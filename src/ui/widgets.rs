//! Small shared pieces: buttons, avatars, the loader, the theme button.

use crate::*;

pub(crate) fn button(key: &'static str, label: &'static str, primary: bool, on_press: fn(&mut State)) -> El {
    let t = tok::theme(dark());
    el().key(key).role("button").label(label).focusable(true).px(16.0).py(8.0).radius(8.0)
        .fill(if primary { tok::accent(0, dark()) } else { t.control_off }).cursor(rune::Cursor::Pointer)
        .on_press(on_press)
        .child(text(label).font(type_::TITLE).color(t.text))
}

/// A circle with the name's initials, coloured by the name so a person keeps their colour.
pub(crate) fn avatar(name: &str, size: f32) -> El {
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect();
    let hue = name.bytes().map(usize::from).sum();
    el().size(size, size).radius(size / 2.0).center().fill(tok::accent(hue, dark()))
        .child(text(initials.as_str()).font(type_::LABEL).color(tok::theme(dark()).bg))
}

pub(crate) fn chat_avatar_id<'a>(chat: &'a Chat, me: &'a str) -> &'a str {
    if !chat.avatar_id.is_empty() { &chat.avatar_id }
    else if chat.id == "48:notes" { me }
    else { teams::peer(&chat.id, me).unwrap_or("") }
}

pub(crate) fn chat_avatar(s: &State, chat: &Chat, me: &str, size: f32) -> El {
    let members = &chat.avatar_members;
    if members.len() < 2 {
        return user_avatar(s, chat_avatar_id(chat, me), &chat.title, size);
    }
    let tile = |member: &auth::User, w: f32, h: f32| {
        match s.avatars.get(&member.id) {
            Some(image) => el().size(w, h).pixels_fit(image.pixels(), rune::SurfaceFit::Fill),
            None => avatar(&member.name.chars().take(1).collect::<String>(), w.max(h)).size(w, h).radius(0.0).clip_rounded(0.0),
        }
    };
    let half = size / 2.0;
    let right = if members.len() >= 3 {
        col().size(half, size)
            .child(tile(&members[1], half, half))
            .child(tile(&members[2], half, half))
    } else { tile(&members[1], half, size) };
    row().size(size, size).clip_rounded(half).label(chat.title.as_str())
        .child(tile(&members[0], half, size)).child(right)
}

pub(crate) fn user_avatar(s: &State, id: &str, name: &str, size: f32) -> El {
    match s.avatars.get(id) {
        Some(image) => el().size(size, size).clip_rounded(size / 2.0).pixels(image.pixels()).label(name),
        None => avatar(name, size),
    }
}

pub(crate) fn loading_messages() -> El {
    static RING: rune::Icon = rune::Icon {
        name: "message-loader",
        view: 16.0,
        body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2" fill="none" opacity="0.2"/><path d="M8 2A6 6 0 0 1 13.258 10.89" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none"/>"#,
    };
    rune::icon(&RING, 24.0).key("message-loader")
        .role("progressbar").label("Loading messages")
        .color(tok::theme(dark()).text_secondary).looping(rune::LoopSpec::rotation(0.9))
}

/// The rail's theme button: each press steps System → Dark → Light → System.
pub(crate) fn theme_button(theme: Option<bool>) -> El {
    static SYSTEM: rune::Icon = rune::Icon {
        name: "theme-system", view: 24.0,
        body: r#"<rect x="3" y="4" width="18" height="12" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 20h8 M12 16v4" fill="none" stroke="currentColor" stroke-width="1.5"/>"#,
    };
    static DARK_ICON: rune::Icon = rune::Icon {
        name: "theme-dark", view: 24.0,
        body: r#"<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"#,
    };
    static LIGHT_ICON: rune::Icon = rune::Icon {
        name: "theme-light", view: 24.0,
        body: r#"<circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 2v2 M12 20v2 M2 12h2 M20 12h2 M4.9 4.9l1.4 1.4 M17.7 17.7l1.4 1.4 M4.9 19.1l1.4-1.4 M17.7 6.3l1.4-1.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
    };
    let t = tok::theme(dark());
    let (icon, label) = match theme {
        None => (&SYSTEM, "System"),
        Some(true) => (&DARK_ICON, "Dark"),
        Some(false) => (&LIGHT_ICON, "Light"),
    };
    col().key("theme").role("button").label(format!("Theme: {label}")).focusable(true).w_full().center().gap(4.0).py(10.0).radius(6.0)
        .hover(t.hover).cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut State| s.theme = next_theme(s.theme))
        .child(rune::icon(icon, 24.0).color(t.text))
        .child(text(label).font(type_::LABEL).color(t.text))
}

pub(crate) fn next_theme(theme: Option<bool>) -> Option<bool> {
    match theme {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_cycles_system_dark_light() {
        assert_eq!(next_theme(None), Some(true));
        assert_eq!(next_theme(Some(true)), Some(false));
        assert_eq!(next_theme(Some(false)), None);
    }

    #[test]
    fn self_chat_uses_own_photo() {
        let chat = |id: &str| Chat { id: id.into(), channel: false, avatar_id: String::new(), avatar_members: Vec::new(), title: String::new(), preview: String::new(), time: String::new(), unread: false };
        let mut bot = chat("19:me_bot@unq.gbl.spaces");
        bot.avatar_id = "28:bot".into();
        assert_eq!(chat_avatar_id(&bot, "me"), "28:bot");
        assert_eq!(chat_avatar_id(&chat("48:notes"), "me"), "me");
        assert_eq!(chat_avatar_id(&chat("19:me_other@unq.gbl.spaces"), "me"), "other");
        assert_eq!(chat_avatar_id(&chat("19:group@thread.v2"), "me"), "");
    }
}
