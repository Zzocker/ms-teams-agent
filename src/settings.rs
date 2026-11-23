//! User preferences, kept separately from the sign-in session.
use crate::State;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const DEFAULT_PANEL_WIDTH: f32 = 304.0;
pub type Favorites = BTreeMap<String, BTreeSet<String>>;

pub fn restore(s: &mut State) {
    let Some(dirs) = rune::dirs::AppDirs::new("ms-teams-agent") else {
        eprintln!("Cannot restore settings: no configuration directory");
        return;
    };
    s.settings_file = Some(dirs.config().join("settings.json"));
    match read(s.settings_file.as_deref().unwrap()) {
        Ok((theme, width, favorites)) => {
            s.theme = theme;
            s.panel_width = width;
            s.favorites = favorites;
        },
        Err(error) => eprintln!("Cannot restore settings: {error}"),
    }
}

fn read(path: &Path) -> Result<(Option<bool>, f32, Favorites), String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((None, DEFAULT_PANEL_WIDTH, Favorites::new())),
        Err(error) => return Err(error.to_string()),
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let theme = match value["theme"].as_str() {
        Some("system") => None,
        Some("dark") => Some(true),
        Some("light") => Some(false),
        _ => return Err("Invalid theme preference".into()),
    };
    let width = value["panel_width"].as_f64()
        .filter(|width| width.is_finite() && (0.0..=crate::PANEL_MAX as f64).contains(width))
        .map(|width| width as f32).unwrap_or(DEFAULT_PANEL_WIDTH);
    let favorites = serde_json::from_value(value["favorites"].clone()).unwrap_or_default();
    Ok((theme, width, favorites))
}

pub fn set_theme(s: &mut State, theme: Option<bool>) {
    s.theme = theme;
    save(s);
}

pub fn save(s: &State) {
    let Some(path) = &s.settings_file else { return; };
    if let Err(error) = write(path, s.theme, s.panel_width, &s.favorites) {
        eprintln!("Cannot save settings: {error}");
    }
}

fn write(path: &Path, theme: Option<bool>, width: f32, favorites: &Favorites) -> std::io::Result<()> {
    let theme = match theme { None => "system", Some(true) => "dark", Some(false) => "light" };
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::json!({"theme": theme, "panel_width": width, "favorites": favorites}).to_string())?;
    std::fs::rename(temporary, path)
}

pub fn toggle_favorite(s: &mut State, id: &str) {
    let crate::Screen::Main { session, .. } = &s.screen else { return };
    let favorites = s.favorites.entry(session.user.id.clone()).or_default();
    if !favorites.remove(id) {
        favorites.insert(id.to_owned());
    }
    save(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_preferences(path: &Path) -> Result<(Option<bool>, f32), String> {
        read(path).map(|(theme, width, _)| (theme, width))
    }

    #[test]
    fn sidebar_keyboard_resize_saves_width_without_losing_theme() {
        let directory = std::env::temp_dir().join(format!("teams-settings-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.json");
        let mut s = crate::init();
        crate::demo(&mut s);
        s.settings_file = Some(path.clone());
        set_theme(&mut s, Some(true));
        let mut rt = rune::Runtime::new(s, crate::view, (1000.0, 680.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.focus("panel-resize", false));
        rt.frame(0.0);
        rt.key(rune::KeyEvent { key: rune::Key::Right, mods: Default::default() });
        rt.frame(0.0);
        assert_eq!(rt.state.panel_width, DEFAULT_PANEL_WIDTH + 20.0);
        assert_eq!(read_preferences(&path).unwrap(), (Some(true), DEFAULT_PANEL_WIDTH + 20.0));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn favorites_survive_setting_changes_and_are_account_scoped() {
        let directory = std::env::temp_dir().join(format!("teams-settings-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.json");
        let mut s = crate::init();
        crate::demo(&mut s);
        s.settings_file = Some(path.clone());
        toggle_favorite(&mut s, "a");
        set_theme(&mut s, Some(true));
        let crate::Screen::Main { session, .. } = &mut s.screen else { unreachable!() };
        std::sync::Arc::make_mut(session).user.id = "other".into();
        toggle_favorite(&mut s, "b");
        let (_, _, restored) = read(&path).unwrap();
        assert_eq!(restored["me"], BTreeSet::from(["a".into()]));
        assert_eq!(restored["other"], BTreeSet::from(["b".into()]));
        toggle_favorite(&mut s, "b");
        assert!(read(&path).unwrap().2["other"].is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn theme_survives_restarts_and_invalid_files_do_not_load() {
        let directory = std::env::temp_dir().join(format!("teams-settings-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.json");
        assert_eq!(read_preferences(&path).unwrap(), (None, DEFAULT_PANEL_WIDTH));
        let mut s = crate::init();
        s.settings_file = Some(path.clone());
        for theme in [Some(true), Some(false), None] {
            set_theme(&mut s, theme);
            assert_eq!(s.theme, theme);
            assert_eq!(read_preferences(&path).unwrap(), (theme, DEFAULT_PANEL_WIDTH));
        }
        for width in [0.0, 240.0, crate::PANEL_MAX] {
            s.panel_width = width;
            set_theme(&mut s, Some(true));
            assert_eq!(read_preferences(&path).unwrap(), (Some(true), width));
            set_theme(&mut s, Some(false));
            assert_eq!(read_preferences(&path).unwrap(), (Some(false), width));
        }
        for width in [serde_json::Value::Null, serde_json::json!(-1), serde_json::json!(999), serde_json::json!("bad")] {
            std::fs::write(&path, serde_json::json!({"theme":"dark", "panel_width":width}).to_string()).unwrap();
            assert_eq!(read_preferences(&path).unwrap(), (Some(true), DEFAULT_PANEL_WIDTH));
        }
        std::fs::write(&path, r#"{"theme":"light"}"#).unwrap();
        assert_eq!(read_preferences(&path).unwrap(), (Some(false), DEFAULT_PANEL_WIDTH));
        for invalid in ["not json", r#"{"theme":"unknown"}"#] {
            std::fs::write(&path, invalid).unwrap();
            assert!(read_preferences(&path).is_err());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
