//! Explicit, anonymous update checks; installation stays with the user.
use crate::*;
use semver::Version;

const RELEASES: &str = "https://github.com/Zzocker/ms-teams-agent/releases";
const API: &str = "https://api.github.com/repos/Zzocker/ms-teams-agent/releases";

pub struct Release {
    pub(crate) version: Version,
    pub(crate) url: String,
}

pub enum Status {
    Idle,
    Current,
    Available(Release),
    Failed,
}

fn newest(value: &serde_json::Value, current: &str) -> Result<Option<Release>, String> {
    let current = Version::parse(current).map_err(|e| e.to_string())?;
    let entries = value.as_array().ok_or("Unexpected release response")?;
    Ok(entries.iter().filter(|r| r["draft"] == false && r["assets"].as_array().is_some_and(|a| !a.is_empty()))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
            // Construct the destination ourselves; never open a URL supplied by the response.
            Some(Release { version, url: format!("{RELEASES}/tag/{tag}") })
        })
        .filter(|r| r.version.cmp_precedence(&current).is_gt())
        .max_by(|a, b| a.version.cmp_precedence(&b.version)))
}

pub fn check(s: &mut State) {
    if s.release_loader.is_pending() || s.waker.is_none() { return; }
    s.release_open_error = false;
    s.release_loader.request((), |_| {
        let result = (|| {
            // shortcut: inspect the newest 100 releases, paginate if that stops covering current versions.
            let mut response = ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(15))).build().new_agent()
                .get(&format!("{API}?per_page=100"))
                .header("User-Agent", concat!("ms-teams-agent/", env!("CARGO_PKG_VERSION")))
                .header("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?;
            let value = response.body_mut().read_json().map_err(|e| e.to_string())?;
            newest(&value, env!("CARGO_PKG_VERSION"))
        })();
        result.map_err(|e: String| format!("Update check failed: {e}"))
    });
}

pub fn poll(s: &mut State) -> bool {
    let Some(result) = s.release_loader.take() else { return false; };
    apply_result(s, result);
    true
}

fn apply_result(s: &mut State, result: Result<Option<Release>, String>) {
    s.release_status = match result {
        Ok(release) => {
            s.release_checked_at = Some(chrono::Local::now());
            match release {
                Some(release) => Status::Available(release),
                None => Status::Current,
            }
        }
        Err(error) => {
            eprintln!("{error}");
            Status::Failed
        }
    };
}

fn open(s: &mut State) {
    s.settings_panel = !s.settings_panel;
}

fn open_download(s: &mut State, tx: &mut Tx) {
    if let Status::Available(release) = &s.release_status {
        s.release_open_error = tx.open_url(&release.url).is_err();
    }
}

fn open_releases(s: &mut State, tx: &mut Tx) {
    s.release_open_error = tx.open_url(RELEASES).is_err();
}

pub fn nav_button(s: &State) -> El {
    static SETTINGS_ICON: rune::Icon = rune::Icon {
        name: "nav-settings", view: 24.0,
        body: r#"<circle cx="12" cy="12" r="7" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 2v3 M12 19v3 M2 12h3 M19 12h3 M5 5l2 2 M17 17l2 2 M5 19l2-2 M17 7l2-2" fill="none" stroke="currentColor" stroke-width="2"/>"#,
    };
    let t = tok::theme(dark());
    let pending = s.release_loader.is_pending();
    col().key("settings-control").role("button").label("Settings").focusable(true)
        .w_full().center().gap(4.0).py(10.0).radius(6.0).hover(t.hover)
        .fill(if s.settings_panel { t.press } else { t.bg }).cursor(rune::Cursor::Pointer)
        .on_press(open)
        .child(rune::icon(&SETTINGS_ICON, 24.0).color(t.text))
        .child(text("Settings").font(type_::LABEL).color(t.text))
        .children((!pending && matches!(s.release_status, Status::Available(_))).then(||
            el().size(6.0, 6.0).radius(3.0).fill(tok::accent(0, dark()))
                .pin(None, Some(6.0), Some(10.0), None)))
}

#[cfg(target_os = "linux")]
const INSTALL: &str = "Download the Linux archive for your distribution and extract it. Quit Teams from the tray, then run sh install.sh inside the extracted folder and reopen Teams.";
#[cfg(target_os = "windows")]
const INSTALL: &str = "Download and extract the Windows ZIP. Quit Teams from the tray, replace the executable in your installation folder, then reopen Teams.";
#[cfg(target_os = "macos")]
const INSTALL: &str = "Download and extract the Apple Silicon archive. Quit Teams from the application menu, replace Teams.app in Applications, then reopen it.";
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const INSTALL: &str = "Download the package for your system. Quit Teams, replace your installed application, then reopen it.";

pub fn panel(s: &State) -> Option<El> {
    if !s.settings_panel { return None; }
    let t = tok::theme(dark());
    let pending = s.release_loader.is_pending();
    let (heading, explanation) = if pending {
        ("Checking for updates…".to_owned(), "Looking for a newer version. You can keep chatting while we check.")
    } else {
        match &s.release_status {
            Status::Idle => ("Keep Teams up to date".into(), "Check for new features and fixes, including preview releases."),
            Status::Current => ("You're up to date".into(), "You have the latest available version, including previews."),
            Status::Available(release) => (format!("Version {} is available", release.version), "A newer version is ready. Download it and install it when you're ready."),
            Status::Failed => ("Couldn't check for updates".into(), "Check your connection or try again in a few minutes. You can also browse releases in your browser."),
        }
    };
    let note = |value: &str| text(value).font(type_::BODY).color(t.text_secondary).wrap();
    let link = |key: &'static str, label: &'static str| el().key(key).role("button").label(label).focusable(true)
        .px(12.0).py(8.0).radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer)
        .child(text(label).font(type_::LABEL).color(t.text));
    let action = link("check-updates", match s.release_status {
        Status::Failed => "Try again", Status::Idle => "Check for updates", _ => "Check again",
    }).fill(t.control_off).on_press(check);
    Some(col().w_full().h_full().center().padding(20.0).fill(t.bg.alpha(0.5)).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(col().key("settings-panel").role("dialog").label("Settings").w_full().max_w(900.0).h_full().max_h(680.0).min_h(0.0)
        .radius(12.0).fill(t.card).border(1.0, t.separator)
        .shadow(4.0, 16.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 })
        .modal(true, |s: &mut State| s.settings_panel = false)
        .child(row().w_full().items_center().px(24.0).py(16.0).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
            .child(text("Settings").font(type_::TITLE).color(t.text).grow(1.0))
            .child(link("close-settings", "Close").on_press(|s: &mut State| s.settings_panel = false)))
        .child(row().w_full().grow(1.0).min_h(0.0)
            .child(col().key("settings-navigation").w(160.0).h_full().padding(12.0).gap(6.0).fill(t.bg)
                .border_sides([0.0, 1.0, 0.0, 0.0], t.separator)
                .children([(false, "settings-appearance", "Appearance"), (true, "settings-updates", "Updates")].into_iter().map(|(updates, key, label)|
                    link(key, label).w_full().fill(if s.settings_updates == updates { t.press } else { t.bg })
                        .on_press(move |s: &mut State| s.settings_updates = updates))))
            .child(if !s.settings_updates {
                col().key("appearance-settings").grow(1.0).min_w(0.0).h_full().padding(20.0).gap(16.0)
                    .child(text("Appearance").font(type_::TITLE).color(t.text))
                    .child(col().w_full().padding(16.0).gap(8.0).radius(8.0).border(1.0, t.separator)
                        .child(text("Theme").font(type_::TITLE).color(t.text))
                        .child(note("Choose System, Dark, or Light."))
                        .child(theme_button(s.theme)))
            } else {
                col().grow(1.0).min_w(0.0).h_full().min_h(0.0).padding(20.0).gap(12.0)
                    .child(col().key("update-details").w_full().gap(12.0).padding(16.0).radius(8.0).border(1.0, t.separator)
                        .child(text("Updates").font(type_::TITLE).color(t.text))
                        .child(col().w_full().gap(6.0)
                            .child(text(&heading).font(type_::TITLE).color(t.text).wrap())
                            .child(note(explanation)))
                        .child(note(concat!("Installed version: ", env!("CARGO_PKG_VERSION"))))
                        .children(s.release_checked_at.map(|time| note(&format!("Last successful check: {}", time.format("%b %-d at %-I:%M %p")))))
                        .children((!pending && matches!(s.release_status, Status::Available(_))).then(||
                            col().gap(8.0).padding(12.0).radius(8.0).fill(t.bg)
                                .child(text("How to update").font(type_::LABEL).color(t.text))
                                .child(note(INSTALL))
                                .child(note("Your sign-in and settings will be kept."))))
                        .children(s.release_open_error.then(|| col().gap(4.0)
                            .child(note("Couldn't open your browser. Copy this address to browse releases:"))
                            .child(text(RELEASES).font(type_::LABEL).color(t.text).wrap().selectable(true))))
                        .scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0))
                    .child(row().w_full().gap(6.0).items_center()
                        .child(link("browse-releases", "All releases").on_press_tx(open_releases))
                        .child(el().grow(1.0))
                        .children((!pending).then(|| if matches!(s.release_status, Status::Available(_)) {
                            link("download-update", "Download update").fill(tok::accent(0, dark())).on_press_tx(open_download)
                        } else { action })))
                    .child(text("Checks include preview releases. Updates are installed manually.").font(type_::LABEL).color(t.text_secondary).wrap())
            }))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn results_show_successful_check_time_and_recover_after_failure() {
        let mut s = init();
        apply_result(&mut s, Ok(None));
        assert!(matches!(s.release_status, Status::Current));
        let checked = s.release_checked_at;
        assert!(checked.is_some());
        apply_result(&mut s, Err("offline".into()));
        assert!(matches!(s.release_status, Status::Failed));
        assert_eq!(s.release_checked_at, checked);
        let release = Release { version: Version::new(0, 1, 1), url: RELEASES.into() };
        apply_result(&mut s, Ok(Some(release)));
        assert!(matches!(s.release_status, Status::Available(_)));
    }

    #[test]
    fn settings_panel_and_actions_fit_the_smallest_window() {
        let mut s = init();
        demo(&mut s);
        s.release_status = Status::Available(Release { version: Version::new(0, 1, 1), url: RELEASES.into() });
        let mut rt = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("theme")));
        let control = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("settings-control")).unwrap().rect;
        rt.pointer_down(control[0] + control[2] / 2.0, control[1] + control[3] / 2.0);
        rt.pointer_up(control[0] + control[2] / 2.0, control[1] + control[3] / 2.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(rt.state.settings_panel);
        let elements = rt.elements();
        let theme = elements.iter().find(|e| e.key.as_deref() == Some("theme")).unwrap();
        let updates = elements.iter().find(|e| e.key.as_deref() == Some("settings-control")).unwrap();
        assert!(updates.rect[0] < 68.0, "settings belongs in the rail");
        let theme_rect = theme.rect;
        let previous_theme = rt.state.theme;
        rt.pointer_down(theme_rect[0] + 10.0, theme_rect[1] + 10.0);
        rt.pointer_up(theme_rect[0] + 10.0, theme_rect[1] + 10.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.theme, next_theme(previous_theme));
        assert!(rt.state.settings_panel, "theme changes keep settings open");
        let panel = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("settings-panel")).unwrap().rect;
        assert!((panel[0] + panel[2] / 2.0 - 320.0).abs() < 1.0);
        assert!((panel[1] + panel[3] / 2.0 - 200.0).abs() < 1.0);
        let updates_tab = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("settings-updates")).unwrap().rect;
        rt.pointer_down(updates_tab[0] + 10.0, updates_tab[1] + 10.0);
        rt.pointer_up(updates_tab[0] + 10.0, updates_tab[1] + 10.0);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(rt.state.settings_updates);
        for key in ["settings-panel", "download-update", "close-settings"] {
            let elements = rt.elements();
            let element = elements.iter().find(|e| e.key.as_deref() == Some(key)).expect(key);
            let [x, y, w, h] = element.rect;
            assert!(w > 0.0 && h > 0.0 && x >= 0.0 && y >= 0.0 && x + w <= 640.5 && y + h <= 400.5,
                "{key} must stay inside the window: {:?}", element.rect);
        }
        rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.settings_panel, "Escape dismisses the panel");
        assert!(matches!(rt.state.release_status, Status::Available(_)), "dismissal keeps the update available");
    }

    #[test]
    fn settings_stays_centered_at_large_window_sizes() {
        for (width, height) in [(1000.0, 680.0), (1920.0, 1080.0)] {
            let mut s = init();
            demo(&mut s);
            s.settings_panel = true;
            let mut rt = rune::Runtime::new(s, view, (width, height), rune::Env::new(true, 1.0));
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            let panel = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("settings-panel")).unwrap().rect;
            assert!((panel[0] + panel[2] / 2.0 - width / 2.0).abs() < 1.0);
            assert!((panel[1] + panel[3] / 2.0 - height / 2.0).abs() < 1.0);
        }
    }

    #[test]
    fn compares_numbers_and_prereleases_using_semver_precedence() {
        let entries = json!([
            {"tag_name":"v0.1.9","draft":false,"assets":[{}]},
            {"tag_name":"v0.1.10-beta.1","draft":false,"assets":[{}]},
            {"tag_name":"v0.1.10","draft":false,"assets":[{}]}
        ]);
        assert_eq!(newest(&entries, "0.1.9").unwrap().unwrap().version.to_string(), "0.1.10");
        let metadata = json!([{"tag_name":"v0.1.0+build.2","draft":false,"assets":[{}]}]);
        assert!(newest(&metadata, "0.1.0").unwrap().is_none());
    }
    #[test]
    fn selects_newest_including_previews_without_offering_downgrades() {
        let entries = json!([
            {"tag_name":"v0.1.2","draft":true,"assets":[{}]},
            {"tag_name":"v0.1.1-beta.1","draft":false,"prerelease":true,"assets":[{}]},
            {"tag_name":"v0.1.0","draft":false,"assets":[{}]},
            {"tag_name":"v0.9.0","draft":false,"assets":[]},
            {"tag_name":"bad","draft":false,"assets":[{}]}
        ]);
        let release = newest(&entries, "0.1.0").unwrap().unwrap();
        assert_eq!(release.version.to_string(), "0.1.1-beta.1");
        assert_eq!(release.url, format!("{RELEASES}/tag/v0.1.1-beta.1"));
        assert!(newest(&entries, "0.1.1").unwrap().is_none());
        assert!(newest(&json!({"message":"rate limited"}), "0.1.0").is_err());
        assert!(newest(&json!([]), "0.1.0").unwrap().is_none());
    }
}
