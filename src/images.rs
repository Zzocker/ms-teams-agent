//! Pictures in messages (the `image` feature), fetched from Teams' media store (and file previews
//! from SharePoint) and kept in a
//! memory LRU (the least recently shown are dropped past a budget) over a disk LRU, so a
//! restart or an evicted picture reads the file instead of the network. Only previews stay in
//! memory: a full-size picture is held while the viewer shows it, then read from disk on the next
//! click. No credentials are written.
use crate::auth::{Session, agent};
use rune::{Image, ImageCache, Latest, Waker};
use std::{cell::Cell, collections::HashMap, path::{Path, PathBuf}, time::SystemTime};

/// Bytes of files kept on disk; the least recently used go first.
const DISK_BUDGET: u64 = 512 * 1024 * 1024;

/// Decoded preview pixels kept, plus the one full-size picture open. Small, because the disk cache
/// brings an evicted picture back without the network.
const BUDGET: u64 = 64 * 1024 * 1024;

/// Previews are tens of KB and full-size screenshots a few MB; a body this big is neither.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// The full-size view of an [`crate::teams::ams_preview`] URL, for the image viewer.
pub fn full(preview: &str) -> String {
    preview.replace("/views/imgo", "/views/imgpsh_fullsize_anim")
}

#[derive(Default)]
pub struct Images {
    /// By URL; `None` when it couldn't be fetched or decoded.
    // shortcut: failures stay for the session and aren't retried; add a retry time if networks are flaky.
    images: HashMap<String, Entry>,
    /// Bumped on every [`Images::get`], so an entry's stamp says when it was last shown.
    clock: Cell<u64>,
    loader: Latest<Vec<(String, Option<Image>)>>,
}

struct Entry {
    image: Option<Image>,
    used: Cell<u64>,
}

/// Decoded RGBA bytes, plus the third more Rune's mip chain takes; a failure holds none.
// shortcut: an animation's later frames aren't counted (Rune doesn't expose them); count them if GIF-heavy chats still grow.
fn cost(image: &Option<Image>) -> u64 {
    image.as_ref().and_then(Image::size).map_or(0, |[w, h]| u64::from(w) * u64::from(h) * 4 * 4 / 3)
}

impl Images {
    /// `None` while loading, `Some(None)` if it failed. Marks it recently used.
    pub fn get(&self, url: &str) -> Option<Option<&Image>> {
        let entry = self.images.get(url)?;
        self.clock.set(self.clock.get() + 1);
        entry.used.set(self.clock.get());
        Some(entry.image.as_ref())
    }

    /// Drop every full-size picture but `open`'s, the one the viewer shows.
    pub fn keep_full(&mut self, open: Option<&str>) {
        let open = open.map(full);
        self.images.retain(|url, _| !url.contains("/views/imgpsh_fullsize_anim") || open.as_deref() == Some(url.as_str()));
    }

    /// At rest keep only the current viewport and viewer; disk-backed previews can load again.
    pub fn retain(&mut self, urls: &std::collections::HashSet<String>, sending: bool) {
        self.images.retain(|url, _| urls.contains(url) || sending && url.starts_with("local:"));
    }

    pub fn insert(&mut self, url: String, image: Option<Image>) {
        self.clock.set(self.clock.get() + 1);
        self.images.insert(url, Entry { image, used: Cell::new(self.clock.get()) });
        self.evict(BUDGET);
    }

    /// Drop the least recently used pictures until their pixels fit in `budget`.
    fn evict(&mut self, budget: u64) {
        let mut total: u64 = self.images.values().map(|e| cost(&e.image)).sum();
        while total > budget {
            let Some(oldest) = self.images.iter().filter(|(_, e)| e.image.is_some())
                .min_by_key(|(_, e)| e.used.get()).map(|(url, _)| url.clone()) else { break };
            total -= self.images.remove(&oldest).map_or(0, |e| cost(&e.image));
        }
    }

    pub fn poll(&mut self, session: &Session, urls: impl Iterator<Item = String>, wake: Waker) -> bool {
        let mut changed = false;
        if let Some(results) = self.loader.take() {
            for (url, image) in results {
                self.insert(url, image);
            }
            changed = true;
        }
        if !self.loader.is_pending() {
            let mut urls: Vec<_> = urls.filter(|u| !self.images.contains_key(u)).collect();
            urls.dedup();
            urls.truncate(4);
            if !urls.is_empty() {
                let session = session.clone();
                self.loader.set_waker(wake.clone());
                self.loader.request((), move |_| {
                    let cache = ImageCache::new(wake);
                    urls.into_iter().map(|url| { let image = load(&session, &url, &cache); (url, image) }).collect()
                });
            }
        }
        changed
    }
}

fn load(s: &Session, url: &str, cache: &ImageCache) -> Option<Image> {
    // The URL carries no secret but the request does: each token goes only to the hosts it's for.
    let auth = if let Some(host) = crate::teams::thumbnail_host(url) {
        format!("Bearer {}", crate::auth::sharepoint_access(host).ok()?)
    } else if crate::teams::ams_preview(url).is_some_and(|p| p == url || full(&p) == url) {
        format!("skype_token {}", s.skype_token)
    } else {
        return None;
    };
    // SharePoint thumbnails change with their file, so `file_name` keeps them off disk.
    let path = rune::dirs::AppDirs::new("ms-teams-agent").filter(|_| crate::avatars::valid_id(&s.user.id))
        .zip(file_name(url)).map(|(d, name)| d.cache().join("images").join(&s.user.id).join(name));
    // Teams media objects never change, so a cached file is always current.
    if let Some(p) = &path && let Ok(bytes) = std::fs::read(p) {
        let image = cache.load(bytes);
        if image.wait().is_ok() {
            // Its modified time is its last use, for trim.
            let _ = std::fs::File::options().write(true).open(p).and_then(|f| f.set_modified(SystemTime::now()));
            return Some(image);
        }
    }
    let mut res = agent().get(url).header("Authorization", &auth).call().ok()?;
    if !res.status().is_success() { return None; }
    let bytes = res.body_mut().with_config().limit(MAX_BYTES).read_to_vec().ok()?;
    // Rune decodes off this thread, caps dimensions and allocation, and turns a codec panic into an error.
    let image = cache.load(bytes.clone());
    image.wait().ok()?;
    // A failed cache write must never prevent displaying a downloaded picture.
    if let Some(path) = path && let Some(dir) = path.parent() && std::fs::create_dir_all(dir).is_ok() {
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() && std::fs::rename(temp, &path).is_ok() {
            trim(dir, DISK_BUDGET);
        }
    }
    Some(image)
}

/// `{object id}.{view}`, or `None` if either part could step outside the cache directory.
fn file_name(url: &str) -> Option<String> {
    let (id, view) = url.split_once("/v1/objects/")?.1.split_once("/views/")?;
    let safe = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (safe(id) && safe(view)).then(|| format!("{id}.{view}"))
}

/// Delete the least recently used files in `dir` until they fit in `budget` bytes.
fn trim(dir: &Path, budget: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries.flatten().filter_map(|e| {
        let m = e.metadata().ok()?;
        m.is_file().then_some(())?;
        Some((m.modified().ok()?, m.len(), e.path()))
    }).collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= budget { break; }
        if std::fs::remove_file(&path).is_ok() { total -= len; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_cleanup_keeps_visible_previews_viewer_and_unsent_images() {
        let mut s = crate::init();
        crate::demo(&mut s);
        let cache = ImageCache::new(std::sync::Arc::new(|| {}));
        let image = cache.rgba(4, 4, vec![127; 64]);
        for url in ["visible", "offscreen", "local:pending:0", "https://example.com/views/imgo", "https://example.com/views/imgpsh_fullsize_anim"] {
            s.images.insert(url.into(), Some(image.clone()));
        }
        let message = &mut s.messages.as_mut().unwrap().as_mut().unwrap().msgs[0];
        message.images = vec!["visible".into()];
        s.preview_rows.insert(message.id.clone(), [0.0, 0.0, 500.0, 200.0]);
        s.preview_viewport.set([0.0, 0.0, 500.0, 500.0]);
        s.viewer = Some("https://example.com/views/imgo".into());
        s.images.retain(&["visible".to_owned()].into(), true);
        assert!(s.images.get("local:pending:0").is_some());
        for url in ["https://example.com/views/imgo", "https://example.com/views/imgpsh_fullsize_anim", "offscreen"] {
            s.images.insert(url.into(), Some(image.clone()));
        }
        assert!(!crate::rest(&mut s, &mut crate::Tx::default()));
        assert!(s.images.get("visible").is_some());
        assert!(s.images.get("https://example.com/views/imgpsh_fullsize_anim").is_some());
        assert!(s.images.get("https://example.com/views/imgo").is_some());
        assert!(s.images.get("offscreen").is_none());
        assert!(s.images.get("local:pending:0").is_none());
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    #[ignore = "Linux/glibc RSS experiment; run alone with --ignored --test-threads=1"]
    fn idle_cleanup_returns_decoded_image_pages_to_linux() {
        let rss = || -> u64 {
            std::fs::read_to_string("/proc/self/status").unwrap().lines().find_map(|l| l.strip_prefix("VmRSS:")
                .and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok())).unwrap()
        };
        let mut s = crate::init();
        crate::demo(&mut s);
        let cache = ImageCache::new(std::sync::Arc::new(|| {}));
        let baseline = rss();
        for i in 0..4096u32 {
            let mut pixels = vec![127u8; 64 * 64 * 4];
            pixels[..4].copy_from_slice(&i.to_le_bytes());
            let image = cache.rgba(64, 64, pixels);
            image.wait().unwrap();
            s.images.insert(format!("image{i}"), Some(image));
        }
        let loaded = rss();
        let cached = s.images.images.len();
        assert!(s.images.images.values().map(|e| cost(&e.image)).sum::<u64>() <= BUDGET);
        // Keep one visible image while the other cached previews become unused.
        let message = &mut s.messages.as_mut().unwrap().as_mut().unwrap().msgs[0];
        message.images = vec!["image4095".into()];
        s.preview_rows.insert(message.id.clone(), [0.0, 0.0, 500.0, 200.0]);
        s.preview_viewport.set([0.0, 0.0, 500.0, 500.0]);
        assert!(!crate::rest(&mut s, &mut crate::Tx::default()));
        let rested = rss();
        assert_eq!(s.images.images.len(), 1);
        assert!(s.images.get("image4095").flatten().is_some());
        assert!(rested + 16 * 1024 < loaded, "RSS should fall by at least 16 MiB: {loaded} -> {rested} KiB");
        let result = serde_json::json!({"baseline_kib":baseline,"before_rest_kib":loaded,"after_rest_kib":rested,
            "cached_images_before":cached,"cached_images_after":1,"generated_images":4096,"visible_image_preserved":true});
        eprintln!("IDLE_MEMORY_PROOF {result}");
        if let Ok(path) = std::env::var("IDLE_MEMORY_EVIDENCE") {
            std::fs::write(format!("{path}.json"), serde_json::to_string_pretty(&result).unwrap()).unwrap();
            let report = format!("Synthetic idle-memory regression (Linux/glibc)\n\n4096 decoded test images, no Teams account or network\nBefore idle cleanup: {:.1} MiB RSS / {cached} cached previews\nAfter idle cleanup: {:.1} MiB RSS / 1 visible preview retained\nReleased: {:.1} MiB RSS\n\nPASS: visible image preserved; unused previews released", loaded as f64 / 1024.0, rested as f64 / 1024.0, (loaded - rested) as f64 / 1024.0);
            let page = s.messages.as_mut().unwrap().as_mut().unwrap();
            page.msgs.truncate(1); page.msgs[0].text = report;
            rune::App::new(crate::AppSpec::new("Idle memory regression", (1200.0, 800.0), crate::init, crate::view))
                .init(move || s).args(["--snap".into(), path, "--scale".into(), "1".into()]).run();
        }
    }


    #[test]
    fn evicts_least_recently_shown_first() {
        let cache = ImageCache::new(std::sync::Arc::new(|| {}));
        let image = || Some(cache.rgba(4, 4, vec![0u8; 64])); // 64 bytes decoded (85 with mips), decoded inline
        let mut images = Images::default();
        for url in ["a", "b", "c"] {
            images.insert(url.into(), image());
        }
        images.insert("failed".into(), None);
        images.get("a"); // now b is the oldest picture
        images.evict(170);
        assert!(images.get("b").is_none());
        assert!(images.get("a").is_some() && images.get("c").is_some());
        assert!(matches!(images.get("failed"), Some(None)), "failures hold no pixels and aren't evicted");
    }

    #[test]
    fn full_size_pictures_leave_memory_when_the_viewer_moves_on() {
        let cache = ImageCache::new(std::sync::Arc::new(|| {}));
        let mut images = Images::default();
        let (a, b) = ("https://h/v1/objects/0-a/views/imgo", "https://h/v1/objects/0-b/views/imgo");
        for url in [a.to_owned(), full(a), b.to_owned(), full(b)] {
            images.insert(url, Some(cache.rgba(1, 1, vec![0u8; 4])));
        }
        images.keep_full(Some(b));
        assert!(images.get(&full(a)).is_none() && images.get(&full(b)).is_some());
        images.keep_full(None);
        assert!(images.get(&full(b)).is_none());
        assert!(images.get(a).is_some() && images.get(b).is_some(), "previews stay");
    }

    #[test]
    fn cache_file_names_cannot_escape_the_directory() {
        assert_eq!(file_name("https://in-api.asm.skype.com/v1/objects/0-cin-d3-41c6/views/imgo").as_deref(), Some("0-cin-d3-41c6.imgo"));
        assert_eq!(file_name(&full("https://h/v1/objects/0-a/views/imgo")).as_deref(), Some("0-a.imgpsh_fullsize_anim"));
        for bad in ["https://h/v1/objects/../views/imgo", "https://h/v1/objects/0-a/views/../../x", "https://h/v1/objects//views/imgo", "https://h/x"] {
            assert_eq!(file_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn disk_trim_deletes_least_recently_used_files() {
        let dir = std::env::temp_dir().join(format!("teams-images-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for (name, age) in [("old", 30), ("mid", 20), ("new", 10)] {
            let path = dir.join(name);
            std::fs::write(&path, [0u8; 10]).unwrap();
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(now - std::time::Duration::from_secs(age)).unwrap();
        }
        trim(&dir, 20);
        let mut left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["mid", "new"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
