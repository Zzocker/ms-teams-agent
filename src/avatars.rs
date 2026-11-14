//! Account-scoped, best-effort photo cache. No credentials are written here.
use crate::auth::{Session, agent};
use rune::{Image, ImageCache, Latest, Waker};
use std::{collections::HashMap, path::Path, time::{Duration, Instant, SystemTime}};

const TTL: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct Avatars {
    images: HashMap<String, (Option<Image>, Instant)>,
    loader: Latest<Vec<(String, Option<Image>, bool)>>,
}

impl Avatars {
    pub fn get(&self, id: &str) -> Option<&Image> {
        self.images.get(id)?.0.as_ref()
    }

    pub fn poll(&mut self, session: &Session, ids: impl Iterator<Item = String>, wake: Waker) -> bool {
        let mut changed = false;
        if let Some(results) = self.loader.take() {
            for (id, image, success) in results {
                self.images.insert(id, (image, Instant::now() + if success { TTL } else { RETRY }));
            }
            changed = true;
        }
        if !self.loader.is_pending() {
            let mut ids: Vec<_> = ids.filter(|id| valid_id(id.strip_prefix("28:").unwrap_or(id)) && self.images.get(id).is_none_or(|(_, until)| *until <= Instant::now())).collect();
            ids.sort();
            ids.dedup();
            ids.truncate(8);
            if !ids.is_empty() {
                let session = session.clone();
                self.loader.set_waker(wake.clone());
                self.loader.request((), move |_| {
                    let cache = ImageCache::new(wake);
                    ids.into_iter().map(|id| {
                        let (image, success) = load(&session, &id, &cache);
                        (id, image, success)
                    }).collect()
                });
            }
        }
        changed
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().enumerate().all(|(i, b)| {
        if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() }
    })
}

fn fresh(path: &Path, now: SystemTime) -> bool {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
        .and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age < TTL)
}

fn load(s: &Session, id: &str, cache: &ImageCache) -> (Option<Image>, bool) {
    let path = rune::dirs::AppDirs::new("ms-teams-agent")
        .filter(|_| valid_id(&s.user.id))
        .map(|d| d.cache().join("avatars").join(&s.user.id).join(id.replace(':', "_")));
    let old = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|bytes| {
        let image = cache.load(bytes);
        image.wait().ok().map(|_| image)
    });
    if old.is_some() && path.as_ref().is_some_and(|p| fresh(p, SystemTime::now())) {
        return (old, true);
    }
    let fetch = || -> Option<(Image, Vec<u8>)> {
        let mri = if id.starts_with("28:") { id.to_owned() } else { format!("8:orgid:{id}") };
        let mut res = agent().get(&format!("{}/beta/users/{}/profilepicturev2/{mri}?size=HR96x96", s.middle_tier, s.user.id))
            .header("Authorization", &format!("Bearer {}", s.access)).call().ok()?;
        if !res.status().is_success() { return None; }
        let bytes = res.body_mut().with_config().limit(2 * 1024 * 1024).read_to_vec().ok()?;
        let image = cache.load(bytes.clone());
        image.wait().ok()?;
        Some((image, bytes))
    };
    match fetch() {
        Some((image, bytes)) => {
            if let Some(path) = path {
                // A failed cache write must never prevent displaying a downloaded photo.
                if let Some(parent) = path.parent() && std::fs::create_dir_all(parent).is_ok() {
                    let temp = path.with_extension("tmp");
                    if std::fs::write(&temp, bytes).is_ok() { let _ = std::fs::rename(temp, path); }
                }
            }
            (Some(image), true)
        }
        None => (old, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_cannot_escape_cache_directory() {
        assert!(valid_id("01234567-89ab-cdef-0123-456789abcdef"));
        for id in ["", "../photo", "8:orgid:01234567-89ab-cdef-0123-456789abcdef", "01234567-89ab-cdef-0123-456789abcdeg"] {
            assert!(!valid_id(id));
        }
    }

    #[test]
    fn disk_cache_expires_and_rejects_future_timestamps() {
        let path = std::env::temp_dir().join(format!("teams-avatar-test-{}", std::process::id()));
        std::fs::write(&path, b"photo").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(fresh(&path, modified));
        assert!(!fresh(&path, modified + TTL));
        assert!(!fresh(&path, modified - Duration::from_secs(1)));
        std::fs::remove_file(&path).unwrap();
        assert!(!fresh(&path, modified));
    }
}
