//! Pictures and files: the media store and SharePoint thumbnails.

use super::*;

/// A downloadable file in the media store. Only these URLs may receive a Skype token.
pub fn ams_file(url: &str) -> Option<String> {
    let preview = ams_preview(url)?;
    let original = preview.strip_suffix("views/imgo")?.to_owned() + "views/original";
    (url == original).then_some(original)
}

/// Save through a temporary file so failed downloads never truncate an existing document.
pub fn download_file(s: &Session, url: &str, path: &std::path::Path) -> Result<(), String> {
    let url = ams_file(url).ok_or("Invalid attachment URL.")?;
    let mut res = agent().get(&url).header("Authorization", &format!("skype_token {}", s.skype_token))
        .header("User-Agent", MEDIA_USER_AGENT).call().map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("File download {}", res.status())); }
    save_download(&mut res.body_mut().as_reader(), path)
}

fn save_download(reader: &mut impl std::io::Read, path: &std::path::Path) -> Result<(), String> {
    let temporary = path.with_file_name(format!(".teams-download-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| e.to_string())?;
        std::io::copy(reader, &mut output).map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() { let _ = std::fs::remove_file(temporary); }
    result
}

/// The media store derives the client platform from this and rejects requests it can't parse.
pub(super) const MEDIA_USER_AGENT: &str = "Teams/24295.605.3225.8804/49";

/// Stores a pasted PNG in the media store, readable by `chat_id`'s members, and returns its
/// [`ams_preview`] URL for the message to show.
pub fn upload_image(s: &Session, chat_id: &str, png: &[u8]) -> Result<String, String> {
    let media = &s.media;
    if media.is_empty() { return Err("no media store for this account".into()); }
    let token = format!("skype_token {}", s.skype_token);
    let body = serde_json::json!({"type": "pish/image", "permissions": {chat_id: ["read"]}});
    let mut res = agent().post(&format!("{media}/v1/objects/")).header("Authorization", &token).header("User-Agent", MEDIA_USER_AGENT)
        .send_json(&body).map_err(|e| e.to_string())?;
    let status = res.status();
    let v: Value = res.body_mut().read_json().map_err(|e| format!("media store {status}: {e}"))?;
    let id = v["id"].as_str().filter(|_| status.is_success()).ok_or(format!("media store {status}"))?;
    let res = agent().put(&format!("{media}/v1/objects/{}/content/imgpsh", encode(id))).header("Authorization", &token)
        .header("User-Agent", MEDIA_USER_AGENT).header("Content-Type", "application/octet-stream").send(png).map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("image upload {}", res.status())); }
    ams_preview(&format!("{media}/v1/objects/{id}/views/imgo")).ok_or("unexpected media store address".into())
}

pub(super) const THUMBNAIL: &str = "/driveItem/thumbnails/0/large/content";

/// The SharePoint files API's thumbnail of a SharePoint file or sharing link, or `None` off SharePoint.
pub fn sharepoint_thumbnail(url: &str) -> Option<String> {
    let host = url.strip_prefix("https://")?.split(['/', '?']).next()?;
    sharepoint(host).then(|| format!("https://{host}/_api/v2.1/shares/u!{}{THUMBNAIL}", b64url(url.as_bytes())))
}

/// The host a [`sharepoint_thumbnail`] URL asks, or `None` for any other URL: fetching it sends a
/// SharePoint token, so only these may get one.
#[cfg_attr(not(feature = "image"), allow(dead_code))]
pub fn thumbnail_host(url: &str) -> Option<&str> {
    let (host, path) = url.strip_prefix("https://")?.split_once('/')?;
    let id = path.strip_prefix("_api/v2.1/shares/u!")?.strip_suffix(THUMBNAIL)?;
    let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (sharepoint(host) && id_ok).then_some(host)
}

/// Microsoft's SharePoint Online; a token for another tenant's site isn't issued, so ours can't leak to it.
pub(super) fn sharepoint(host: &str) -> bool {
    host.ends_with(".sharepoint.com") && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// Unpadded base64url, as the SharePoint shares API takes a URL.
pub(super) fn b64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    bytes.chunks(3).flat_map(|c| {
        let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        (0..=c.len()).map(move |i| ABC[(n >> (18 - 6 * i) & 63) as usize] as char)
    }).collect()
}

/// The small preview view of a picture in Teams' media store, or `None` for any other URL:
/// fetching it sends the Teams token, so only these hosts may get it.
pub fn ams_preview(url: &str) -> Option<String> {
    let (host, path) = url.strip_prefix("https://")?.split_once('/')?;
    let id = path.strip_prefix("v1/objects/")?.split('/').next()?;
    let host_ok = host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        && (host.ends_with(".asm.skype.com") || host.ends_with(".asyncgw.teams.microsoft.com"));
    let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    (host_ok && id_ok).then(|| format!("https://{host}/v1/objects/{id}/views/imgo"))
}

/// Only public Giphy image hosts are fetched, without an account token.
pub fn giphy_image(url: &str) -> Option<String> {
    let (host, path) = url.strip_prefix("https://")?.split_once('/')?;
    let path = path.split('?').next()?;
    (matches!(host, "media.giphy.com" | "media0.giphy.com" | "media1.giphy.com" | "media2.giphy.com" | "media3.giphy.com" | "media4.giphy.com" | "i.giphy.com")
        && path.starts_with("media/") && (path.ends_with(".gif") || path.ends_with(".webp")))
        .then(|| url.to_owned())
}

/// Public Teams emoji sprites never receive an account token.
#[cfg(any(feature = "image", test))]
pub fn emoji_animation(src: &str) -> Option<String> {
    let path = src.strip_prefix("https://statics.teams.cdn.office.net/")?.split('?').next()?;
    let parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() != 8 || parts[..5] != ["evergreen-assets", "personal-expressions", "v2", "assets", "emoticons"]
        || parts[6] != "default" || parts[5].is_empty()
        || !parts[5].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    let (size, variant) = parts[7].split_once('_')?;
    if !matches!(size, "20" | "40" | "60" | "80" | "100") { return None; }
    let file = match variant {
        "f.png" | "anim_f.png" => "100_anim_f.png",
        "f_s1.png" | "anim_f_s1.png" => "100_anim_f_s1.png",
        "f_s2.png" | "anim_f_s2.png" => "100_anim_f_s2.png",
        "f_s3.png" | "anim_f_s3.png" => "100_anim_f_s3.png",
        "f_s4.png" | "anim_f_s4.png" => "100_anim_f_s4.png",
        "f_s5.png" | "anim_f_s5.png" => "100_anim_f_s5.png",
        "f_s6.png" | "anim_f_s6.png" => "100_anim_f_s6.png",
        _ => return None,
    };
    Some(format!("https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/{}/default/{file}", parts[5]))
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn file_messages_round_trip_and_keep_mentions() {
        let files = vec![serde_json::json!({"id":"0-file-id", "fileName":"Plan <final> & notes.PDF", "fileType":"pdf",
            "objectUrl":"https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-file-id/views/original"})];
        let mut body = serde_json::json!({"content":"", "messagetype":"RichText/Html"});
        apply_files(&mut body, &files);
        apply_mentions(&mut body, "", &[]).unwrap();
        let msgs = parse_messages(&serde_json::json!({"messages":[body]}), "me");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "");
        assert_eq!(msgs[0].files.len(), 1);
        assert_eq!(msgs[0].files[0].name, "Plan <final> & notes.PDF");
        assert_eq!(msgs[0].files[0].kind, "pdf");
        assert!(ams_file(&msgs[0].files[0].url).is_some());
        assert!(msgs[0].images.is_empty());
    }

    #[test]
    fn download_tokens_are_confined_to_exact_media_store_file_urls() {
        let valid = "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-file-id/views/original";
        assert_eq!(ams_file(valid).as_deref(), Some(valid));
        for bad in ["https://evil.example/v1/objects/0-file-id/views/original",
            "https://in-prod.asyncgw.teams.microsoft.com.evil.example/v1/objects/0-file-id/views/original",
            "https://user@in-prod.asyncgw.teams.microsoft.com/v1/objects/0-file-id/views/original",
            "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/../views/original"] {
            assert!(ams_file(bad).is_none(), "{bad}");
        }
        assert!(ams_file(&format!("{valid}?redirect=evil")).is_none());
        assert!(ams_file(&valid.replace("views/original", "content/original")).is_none());
    }

    #[test]
    fn download_failure_preserves_existing_file_and_cleans_temporary_file() {
        let dir = std::env::temp_dir().join(format!("teams-download-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("report.txt");
        std::fs::write(&path, b"original").unwrap();
        struct Broken;
        impl std::io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> { Err(std::io::Error::other("connection lost")) }
        }
        assert!(save_download(&mut Broken, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        save_download(&mut &b"downloaded"[..], &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"downloaded");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
