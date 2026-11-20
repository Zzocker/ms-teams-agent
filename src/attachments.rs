//! Device files stay local until Send; uploads and downloads run off the UI thread.
use crate::*;
use std::path::{Path, PathBuf};

pub(crate) const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct DraftFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub modified: std::time::SystemTime,
    /// Keep successful uploads when a later upload or message send fails.
    pub uploaded: Option<serde_json::Value>,
}

impl DraftFile {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let meta = path.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() { return Err("Choose a file, not a folder.".into()); }
        if meta.len() > MAX_FILE_SIZE { return Err("Files must be 100 MiB or smaller.".into()); }
        let name = path.file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).ok_or("Invalid file name.")?.to_owned();
        Ok(Self { path, name, size: meta.len(), modified: meta.modified().map_err(|e| e.to_string())?, uploaded: None })
    }

    pub fn card(&self) -> teams::File {
        teams::File { name: self.name.clone(), kind: extension(&self.name), url: String::new(), preview: None }
    }
}

pub(crate) fn extension(name: &str) -> String {
    Path::new(name).extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub(crate) fn pick_file(s: &mut State, tx: &mut Tx) {
    if s.editing.is_some() || s.selected.is_none() { return; }
    let chat = s.selected.clone();
    let attempt = s.attempt.load(SeqCst);
    tx.open_file(rune::FileDialogOptions { title: "Attach file from this device".into(), ..Default::default() },
        move |s: &mut State, result, _| {
            if s.selected != chat || s.attempt.load(SeqCst) != attempt || s.editing.is_some() { return; }
            match result {
                Ok(Some(path)) => match DraftFile::new(path) {
                    Ok(file) => {
                        if !s.attached.iter().any(|f| f.path == file.path) { s.attached.push(file); }
                        s.send_error = None;
                    }
                    Err(e) => s.send_error = Some(format!("Not attached: {e}")),
                },
                Ok(None) => {},
                Err(e) => s.send_error = Some(format!("Cannot open file picker: {e}")),
            }
        });
}

pub(crate) fn draft_files(s: &State) -> Option<El> {
    if s.attached.is_empty() { return None; }
    let t = tok::theme(dark());
    Some(col().key("draft-files").w_full().gap(4.0).children(s.attached.iter().enumerate().map(|(i, f)| {
        row().key(format!("attached/{i}")).w_full().items_center().gap(8.0).padding(8.0).radius(6.0).fill(t.bg)
            .child(text(format!("📎 {} ({:.1} KiB)", f.name, f.size as f64 / 1024.0)).font(type_::LABEL).color(t.text).truncate().grow(1.0).min_w(0.0))
            .child(el().key(format!("remove-file/{i}")).role("button").label(format!("Remove {}", f.name)).focusable(true)
                .cursor(rune::Cursor::Pointer).padding(6.0).hover(t.hover)
                .on_press(move |s: &mut State| { if i < s.attached.len() { s.attached.remove(i); } })
                .child(text("Remove").font(type_::LABEL).color(t.text)))
    })))
}

/// Media-store downloads require authentication; don't open their raw URL in the browser.
pub(crate) fn open_file(s: &mut State, file: &teams::File, tx: &mut Tx) {
    if file.url.is_empty() { return; }
    if teams::ams_file(&file.url).is_none() {
        if let Err(e) = tx.open_url(&file.url) { s.send_error = Some(format!("Cannot open file: {e}")); }
        return;
    }
    if s.file_download.is_pending() { return; }
    let Screen::Main { session, .. } = &s.screen else { return; };
    let (session, url, attempt) = (session.clone(), file.url.clone(), s.attempt.load(SeqCst));
    let name = Path::new(&file.name).file_name().unwrap_or_default().to_string_lossy().into_owned();
    tx.save_file(rune::FileDialogOptions { title: "Save attachment".into(), file_name: Some(name), ..Default::default() },
        move |s: &mut State, result, _| {
            if s.attempt.load(SeqCst) != attempt || s.file_download.is_pending() { return; }
            match result {
                Ok(Some(path)) => s.file_download.request((), move |_| teams::download_file(&session, &url, &path)),
                Ok(None) => {},
                Err(e) => s.send_error = Some(format!("Cannot save attachment: {e}")),
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("teams-attachment-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("notes & plan.txt"), b"test attachment").unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf { self.0.join("notes & plan.txt") }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn click(rt: &mut rune::Runtime<State>, key: &str) {
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key}")).rect;
        rt.pointer_move(rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
        rt.pointer_down(rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
        rt.pointer_up(rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
        rt.frame(0.0);
    }

    #[test]
    fn picker_selects_removes_cancels_and_ignores_another_chat() {
        let fixture = Fixture::new();
        let mut s = init();
        demo(&mut s);
        let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        click(&mut rt, "attach-file");
        assert_eq!(rt.file_dialog().unwrap().0, rune::FileDialogKind::Open);
        rt.resolve_file_dialog(Some(fixture.file())).unwrap();
        rt.frame(0.0);
        assert_eq!(rt.state.attached.len(), 1);
        assert_eq!(rt.state.attached[0].name, "notes & plan.txt");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("draft-files")));
        if let Ok(path) = std::env::var("ATTACHMENT_EVIDENCE") {
            rune::App::new(AppSpec::new("Device attachments", (1200.0, 800.0), init, view))
                .init(move || rt.state).args(["--snap".into(), path, "--scale".into(), "1".into()]).run();
            return;
        }
        click(&mut rt, "attach-file");
        rt.resolve_file_dialog(Some(fixture.file())).unwrap();
        rt.frame(0.0);
        assert_eq!(rt.state.attached.len(), 1, "same path isn't attached twice");
        click(&mut rt, "remove-file/0");
        assert!(rt.state.attached.is_empty());
        click(&mut rt, "attach-file");
        rt.resolve_file_dialog(None).unwrap();
        rt.frame(0.0);
        assert!(rt.state.send_error.is_none());
        rt.update(|s, _| s.editor = true);
        rt.frame(0.0);
        click(&mut rt, "editor-attach-file");
        rt.resolve_file_dialog(Some(fixture.file())).unwrap();
        rt.frame(0.0);
        assert_eq!(rt.state.attached.len(), 1);
        click(&mut rt, "remove-file/0");
        assert!(rt.state.attached.is_empty());
        rt.update(|s, _| s.editor = false);
        rt.frame(0.0);
        click(&mut rt, "attach-file");
        rt.state.selected = Some("other-chat".into());
        rt.resolve_file_dialog(Some(fixture.file())).unwrap();
        rt.frame(0.0);
        assert!(rt.state.attached.is_empty());
    }

    #[test]
    fn files_are_validated_and_cleared_on_navigation() {
        let fixture = Fixture::new();
        assert!(DraftFile::new(fixture.0.clone()).is_err());
        assert!(DraftFile::new(fixture.0.join("missing")).is_err());
        let large = fixture.0.join("large.bin");
        std::fs::File::create(&large).unwrap().set_len(MAX_FILE_SIZE + 1).unwrap();
        assert!(DraftFile::new(large).unwrap_err().contains("100 MiB"));
        let mut file = DraftFile::new(fixture.file()).unwrap();
        std::fs::write(fixture.file(), b"changed").unwrap();
        assert!(teams::upload_file(&Session::default(), "chat", &mut file).unwrap_err().contains("changed"));
        let mut s = init();
        demo(&mut s);
        s.attached.push(file.clone());
        open_chat(&mut s, "other-chat".into());
        assert!(s.attached.is_empty());
        s.attached.push(file);
        open_chat(&mut s, "a".into());
        assert!(s.attached.is_empty());
    }

    #[test]
    fn attachment_only_send_shows_a_card_and_restores_failed_draft() {
        let fixture = Fixture::new();
        let mut s = init();
        demo(&mut s);
        s.attached.push(DraftFile::new(fixture.file()).unwrap());
        let count = s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len();
        send(&mut s);
        assert!(s.attached.is_empty());
        let page = s.messages.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(page.msgs.len(), count + 1);
        assert!(page.msgs.last().unwrap().text.is_empty());
        assert_eq!(page.msgs.last().unwrap().files[0].name, "notes & plan.txt");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while s.send_error.is_none() && std::time::Instant::now() < deadline {
            poll(&mut s, 0.0, &mut Tx::default());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!s.sender.is_pending());
        assert!(s.send_error.as_deref().unwrap().starts_with("Not sent:"));
        assert_eq!(s.attached.len(), 1);
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len(), count);
    }
}
