//! The full call view and the controls available while browsing chats.

use crate::calling::{Active, Calling, Phase};
use crate::*;

static PHONE: rune::Icon = rune::Icon {
    name: "call-audio",
    view: 24.0,
    body: r#"<path d="M7 3H4a1 1 0 0 0-1 1c0 9.4 7.6 17 17 17a1 1 0 0 0 1-1v-3l-5-2-2 2a13 13 0 0 1-7-7l2-2-2-5Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
static HANG_UP: rune::Icon = rune::Icon {
    name: "hang-up",
    view: 24.0,
    body: r#"<path d="M3 14v-2c0-1.5 4-4 9-4s9 2.5 9 4v2a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1v-2a11 11 0 0 0-8 0v2a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1Z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"#,
};
static MIC: rune::Icon = rune::Icon {
    name: "microphone",
    view: 24.0,
    body: r#"<path d="M9 6a3 3 0 0 1 6 0v5a3 3 0 0 1-6 0V6ZM6 11a6 6 0 0 0 12 0m-6 6v4m-3 0h6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};
static MIC_OFF: rune::Icon = rune::Icon {
    name: "microphone-muted",
    view: 24.0,
    body: r#"<path d="M9 5a3 3 0 0 1 6 1v5m-6-1v1a3 3 0 0 0 4 2.8M6 11a6 6 0 0 0 9.3 5m2-2.8a6 6 0 0 0 .7-2.2m-6 6v4m-3 0h6M3 3l18 18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};
static CHAT: rune::Icon = rune::Icon {
    name: "call-chat",
    view: 24.0,
    body: r#"<path d="M20 15a2 2 0 0 1-2 2H9l-5 4V6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v9ZM8 9h8m-8 4h5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(crate) fn start_button(s: &State, session: &Session, chat: &Chat) -> Option<El> {
    let peer = Calling::peer(s, session, chat)?;
    let t = tok::theme(dark());
    let contact = Arc::new(calling::Contact {
        chat: chat.id.clone(),
        avatar: if chat.avatar_id.is_empty() {
            peer.into()
        } else {
            chat.avatar_id.clone()
        },
        name: chat.title.clone(),
    });
    let active = s.calling.active.is_some();
    Some(
        el().key("audio-call")
            .role("button")
            .label("Start audio call")
            .help("Start audio call")
            .size(44.0, 44.0)
            .center()
            .radius(7.0)
            .disabled(active)
            .focusable(!active)
            .hover(t.hover)
            .cursor(rune::Cursor::Pointer)
            .on_press_tx(move |s: &mut State, tx| Calling::start_teams(s, contact.clone(), tx))
            .child(
                rune::icon(&PHONE, 20.0)
                    .color(t.text_secondary)
                    .a11y_hidden(),
            ),
    )
}

fn control(
    key: &'static str,
    label: &'static str,
    icon: &'static rune::Icon,
    on_press: fn(&mut State, &mut Tx),
) -> El {
    let t = tok::theme(dark());
    let danger = key == "end-audio-call";
    let foreground = if danger { rune::Color::WHITE } else { t.text };
    let button = el()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .min_h(44.0)
        .px(14.0)
        .center()
        .radius(9.0)
        .fill(if danger {
            rune::oklch(0.46, 0.17, 22.0)
        } else {
            t.control_track
        })
        .hover(if danger {
            rune::oklch(0.52, 0.18, 22.0)
        } else {
            t.hover
        })
        .press(t.press)
        .cursor(rune::Cursor::Pointer)
        .on_press_tx(on_press);
    button.child(
        row()
            .items_center()
            .gap(8.0)
            .child(rune::icon(icon, 18.0).color(foreground).a11y_hidden())
            .child(
                text(label)
                    .font(type_::LABEL)
                    .color(foreground)
                    .a11y_hidden(),
            ),
    )
}

fn mute(call: &Active) -> El {
    let (label, icon) = if call.muted {
        ("Unmute", &MIC_OFF)
    } else {
        ("Mute", &MIC)
    };
    control("mute-audio-call", label, icon, |s, _| {
        s.calling.toggle_mute();
    })
    .help("Toggle microphone (Ctrl+Shift+M or ⌘⇧M)")
    .disabled(matches!(call.phase, Phase::Ending))
}

fn hang_up(call: &Active) -> El {
    let label = match call.phase {
        Phase::Dialing | Phase::Ringing => "Cancel call",
        _ => "End call",
    };
    control("end-audio-call", label, &HANG_UP, |s, _| s.calling.cancel())
        .disabled(matches!(call.phase, Phase::Ending))
}

fn open_chat(s: &mut State, tx: &mut Tx) {
    let contact = s
        .calling
        .active
        .as_ref()
        .map(|call| &call.contact)
        .or_else(|| s.calling.finished.as_ref().map(|call| &call.contact));
    s.calling.expanded = false;
    if let Some(contact) = contact {
        if s.selected.as_ref() != Some(&contact.chat) {
            crate::open_chat(s, contact.chat.clone());
        } else {
            notifications::mark_selected_read(s);
        }
    }
    s.calling.finished = None;
    tx.focus(
        if s.calling.active.is_some() {
            "return-to-call"
        } else {
            "audio-call"
        },
        false,
    );
}

pub(crate) fn compact(s: &State) -> Option<El> {
    if s.calling.expanded {
        return None;
    }
    let t = tok::theme(dark());
    let bar = row()
        .key("call-status")
        .w_full()
        .items_center()
        .gap(8.0)
        .px(12.0)
        .py(5.0)
        .fill(t.card)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.separator);
    if let Some(call) = &s.calling.active {
        let status = match call.duration() {
            Some(duration) if call.muted => format!("Connected · {duration} · Muted"),
            Some(duration) => format!("Connected · {duration}"),
            None => call.status().into(),
        };
        return Some(
            bar.child(user_avatar(s, &call.contact.avatar, &call.contact.name, 32.0).a11y_hidden())
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            text(&call.contact.name)
                                .font(type_::TITLE)
                                .color(t.text)
                                .truncate(),
                        )
                        .child(
                            text(status)
                                .font(type_::LABEL)
                                .color(t.text_secondary)
                                .truncate(),
                        ),
                )
                .child(control(
                    "return-to-call",
                    "Return to call",
                    &PHONE,
                    |s, tx| {
                        s.calling.expanded = true;
                        tx.focus(
                            if s.calling
                                .active
                                .as_ref()
                                .is_some_and(|call| matches!(call.phase, Phase::Ending))
                            {
                                "open-call-chat"
                            } else {
                                "mute-audio-call"
                            },
                            false,
                        );
                    },
                ))
                .child(mute(call))
                .child(hang_up(call)),
        );
    }
    let error = s.calling.finished.as_ref()?.error.as_ref()?;
    Some(
        bar.child(
            text(error)
                .role("alert")
                .font(type_::BODY)
                .color(t.text)
                .grow(1.0)
                .min_w(0.0)
                .wrap(),
        )
        .child(button("dismiss-call-error", "Dismiss", false, |s| {
            s.calling.finished = None
        })),
    )
}

pub(crate) fn screen(s: &State) -> Option<El> {
    if !s.calling.expanded {
        return None;
    }
    let call = s.calling.active.as_ref();
    let finished = s.calling.finished.as_ref();
    let contact = call
        .map(|call| &call.contact)
        .or_else(|| finished.map(|call| &call.contact))?;
    let name = contact.name.as_str();
    let error = finished.and_then(|call| call.error.as_deref());
    let status = if let Some(call) = call {
        call.status()
    } else if error.is_some() {
        "Call failed"
    } else {
        "Call ended"
    };
    let t = tok::theme(dark());
    let tint = if dark() {
        rune::oklch(0.26, 0.028, 260.0)
    } else {
        rune::oklch(0.965, 0.018, 260.0)
    };
    let status_dot = if call.is_some_and(|call| matches!(call.phase, Phase::Connected(_))) {
        if dark() {
            rune::oklch(0.77, 0.13, 155.0)
        } else {
            rune::oklch(0.46, 0.12, 155.0)
        }
    } else {
        t.text_secondary
    };

    let stage = col()
        .key("call-participant")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .gap(14.0)
        .padding(16.0)
        .radius(18.0)
        .gradient(tint, t.card)
        .border(1.0, t.card_border)
        .child(
            el().size(104.0, 104.0)
                .radius(52.0)
                .center()
                .fill(t.card)
                .border(1.0, t.card_border)
                .child(user_avatar(s, &contact.avatar, name, 86.0).a11y_hidden()),
        )
        .child(
            text(name)
                .font(rune::TextStyle::new(25.0, 650.0))
                .color(t.text)
                .max_w(480.0)
                .truncate(),
        )
        .child(
            row()
                .center()
                .gap(8.0)
                .px(12.0)
                .py(5.0)
                .radius(20.0)
                .fill(t.control_track)
                .child(
                    el().size(7.0, 7.0)
                        .radius(3.5)
                        .fill(status_dot)
                        .a11y_hidden(),
                )
                .child(text(status).role("status").font(type_::LABEL).color(t.text))
                .children(call.and_then(Active::duration).map(|duration| {
                    text(duration)
                        .key("call-duration")
                        .font(type_::LABEL)
                        .color(t.text_secondary)
                })),
        )
        .children(error.map(|error| {
            text(error)
                .role("alert")
                .font(type_::BODY)
                .color(t.text)
                .max_w(440.0)
                .wrap()
        }));

    let controls = row()
        .key("call-controls")
        .role("group")
        .label("Call controls")
        .center()
        .gap(8.0);
    let controls = if let Some(call) = call {
        controls
            .child(control("open-call-chat", "Chat", &CHAT, open_chat))
            .child(mute(call))
            .child(hang_up(call))
    } else {
        controls.child(control("close-call", "Back to chat", &CHAT, open_chat))
    };
    Some(
        col()
            .key("call-screen")
            .label("Audio call")
            .w_full()
            .h_full()
            .fill(t.bg)
            .gap(12.0)
            .padding(16.0)
            .child(
                row()
                    .items_center()
                    .gap(8.0)
                    .child(
                        rune::icon(&PHONE, 17.0)
                            .color(t.text_secondary)
                            .a11y_hidden(),
                    )
                    .child(text("Audio call").font(type_::TITLE).color(t.text)),
            )
            .child(stage)
            .child(controls),
    )
}
