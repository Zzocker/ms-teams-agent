//! The message box and the pop-up editor.

use crate::*;

/// Whether `before_caret` leaves a ``` block open: there Enter types a line, as in Teams, rather than sending.
pub(crate) fn in_code_block(before_caret: &str) -> bool {
    before_caret.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1
}

/// A picture waiting in the draft, with a button to take it out.
pub(crate) fn pasted_image(image: &rune::Image, i: usize) -> El {
    static CLOSE: rune::Icon = rune::Icon { name: "pasted-remove", view: 16.0, body: r#"<path d="M4 4l8 8 M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"# };
    let t = tok::theme(dark());
    col().key(format!("pasted/{i}")).size(150.0, 100.0).radius(6.0).clip_rounded(6.0).fill(t.bg).border(1.0, t.separator)
        .child(el().w_full().h_full().pixels_fit(image.pixels(), rune::SurfaceFit::Fit).label("Pasted image"))
        .child(el().pin(None, Some(4.0), Some(4.0), None).size(22.0, 22.0).center().radius(11.0).fill(t.card)
            .role("button").label("Remove image").focusable(true).cursor(rune::Cursor::Pointer).hover(t.hover)
            .on_press(move |s: &mut State| { if i < s.pasted.len() { s.pasted.remove(i); } })
            .child(rune::icon(&CLOSE, 12.0).color(t.text)))
}

/// A picture pasted into the message box is attached to the draft; text pastes as usual.
#[cfg(feature = "image")]
pub(crate) fn draft_paste(input: El) -> El {
    input.on_input_paste(|s: &mut State, content: &rune::ClipboardContent, _| match content.image() {
        Some(png) => { paste_image(s, png); true }
        None => false,
    })
}

#[cfg(not(feature = "image"))]
pub(crate) fn draft_paste(input: El) -> El {
    input
}

/// The draft as its bubble will show it.
#[cfg(feature = "markdown")]
pub(crate) fn preview_text(draft: &str) -> El {
    let md = markdown::parse(draft);
    let spans = md_spans(&md);
    rune::rich(md.text, spans)
}

#[cfg(not(feature = "markdown"))]
pub(crate) fn preview_text(draft: &str) -> El {
    text(draft)
}

pub(crate) fn composer_preview(s: &State) -> Option<El> {
    #[cfg(feature = "markdown")]
    if !s.editor && !markdown::parse(&s.draft.text).spans.is_empty() {
        let t = tok::theme(dark());
        return Some(col().key("composer-preview").w_full().max_h(160.0).gap(4.0)
            .label("Message preview").scroll(ScrollSpec { stick_bottom: false })
            .child(text("Preview").font(type_::LABEL).color(t.text_secondary))
            .child(preview_text(&s.draft.text).font(type_::BODY).color(t.text).wrap_anywhere()));
    }
    let _ = s;
    None
}

/// The writing side's share after moving the divider `delta` points across `width`.
pub(crate) fn editor_split(start: f32, delta: f32, width: f32) -> f32 {
    if width <= 0.0 { return start }
    (start + delta / width).clamp(0.2, 0.9)
}

/// The draft in a pop-up editor: the raw text on the left and, when on, how it will read on the
/// right, split by a divider that drags. Escape or a click outside closes it; the draft stays.
pub(crate) fn draft_editor(s: &State) -> Option<El> {
    if !s.editor { return None }
    let t = tok::theme(dark());
    let input = draft_paste(text_input(&s.draft, type_::BODY).key("editor-draft").label("Message").color(t.text)
        .placeholder("Type a message", t.text_secondary)
        .caret(t.text, tok::accent(0, dark()).alpha(0.3))
        .autofocus()
        .on_edit(|s: &mut State| &mut s.draft)
        .on_input_change(mention_changed)
        .on_input_key(mention_key)
        // Enter is a new line here; Send sends.
        .on_submit(|s: &mut State, _| s.draft.insert("\n")));
    let write = col().key("editor-write").h_full().min_w(0.0).padding(16.0).children(mention_suggestions(s)).child(auto_grow(input, 1, 10_000));
    let body = row().w_full().grow(1.0).min_h(0.0).on_layout(|s: &mut State, [_, _, w, _]| s.editor_w = w);
    let body = if s.editor_preview {
        let divider = col().key("editor-resize").w(8.0).h_full().center().cursor(rune::Cursor::ResizeColumn)
            .label("Resize editor and preview with Left and Right arrows").focusable(true)
            .on_key(|s: &mut State, e: &rune::KeyEvent, _| {
                let delta = match e.key {
                    rune::Key::Left => -20.0,
                    rune::Key::Right => 20.0,
                    _ => return false,
                };
                s.editor_split = editor_split(s.editor_split, delta, s.editor_w);
                true
            })
            .on_drag(|s: &mut State, d: &mut Drag, _| {
                if d.phase == Phase::Down { d.data = s.editor_split; }
                s.editor_split = if d.cancelled() { d.data } else { editor_split(d.data, d.dx, s.editor_w) };
            })
            .child(el().w(1.0).h_full().fill(t.separator));
        // Both sides grow from zero in the split's ratio, so it holds before the width is known.
        body.child(write.w(0.0).grow(s.editor_split))
            .child(divider)
            .child(col().key("editor-preview").w(0.0).grow(1.0 - s.editor_split).min_w(160.0).h_full().padding(16.0).scroll(ScrollSpec { stick_bottom: false })
                .child(preview_text(&s.draft.text).font(type_::BODY).color(t.text).wrap_anywhere().selectable(true)))
    } else {
        body.child(write.grow(1.0))
    };
    // Centred and capped, so the chat stays in sight around it on a large window.
    Some(col().w_full().h_full().center().padding(64.0).fill(t.bg.alpha(0.7))
        .child(col().key("draft-editor").label("Editor").w_full().max_w(1000.0).h_full().max_h(700.0).min_h(0.0).radius(12.0).fill(t.card).border(1.0, t.separator)
            .modal(true, |s: &mut State| s.editor = false)
            .child(row().w_full().gap(8.0).px(16.0).py(10.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                .child(text("Editor").font(type_::TITLE).color(t.text).grow(1.0))
                .child(el().key("editor-preview-toggle").role("button").label("Preview").focusable(true).px(12.0).py(6.0).radius(6.0)
                    .fill(if s.editor_preview { t.press } else { t.control_off }).cursor(rune::Cursor::Pointer)
                    .on_press(|s: &mut State| s.editor_preview = !s.editor_preview)
                    .child(text(if s.editor_preview { "Preview on" } else { "Preview off" }).font(type_::LABEL).color(t.text)))
                .child(button("editor-close", "Close", false, |s| s.editor = false)))
            .child(body)
            .children(crate::attachments::draft_files(s))
            .child(row().w_full().justify_end().px(16.0).py(10.0).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
                .children(s.editing.is_none().then(|| el().key("editor-attach-file").role("button").label("Attach file from this device").focusable(true)
                    .cursor(rune::Cursor::Pointer).padding(8.0).on_press_tx(crate::attachments::pick_file)
                    .child(text("📎 Attach file").font(type_::LABEL).color(t.text))))
                .child(button("editor-send", "Send", true, |s| { s.editor = false; send(s) })))))
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[cfg(feature = "markdown")]
    #[test]
    fn normal_composer_previews_code_while_typing() {
        let mut s = init();
        demo(&mut s);
        let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| { tx.focus("draft", false); });
        rt.frame(0.0);
        for ch in "Test data: run `cargo test` before sending.".chars() {
            rt.key(rune::KeyEvent { key: rune::Key::Text(ch.to_string()), mods: Default::default() });
            rt.frame(0.0);
        }
        let preview = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("composer-preview")).unwrap();
        assert!(preview.rect[2] > 0.0 && preview.rect[3] > 0.0);
        assert_eq!(rt.state.draft.text, "Test data: run `cargo test` before sending.");
        assert!(!rt.state.editor);
        if let Ok(path) = std::env::var("COMPOSER_PREVIEW_EVIDENCE") {
            rune::App::new(AppSpec::new("Composer preview regression", (1200.0, 800.0), init, view))
                .init(move || rt.state).args(["--snap".into(), path, "--scale".into(), "1".into()]).run();
            return;
        }
        rt.update(|s, _| s.editor = true);
        rt.frame(0.0);
        assert!(super::composer_preview(&rt.state).is_none());
        rt.update(|s, _| { s.editor = false; s.draft.clear(); });
        rt.frame(0.0);
        assert!(super::composer_preview(&rt.state).is_none());
    }

    #[test]
    fn editor_split_moves_by_share_of_width_and_stays_in_bounds() {
        assert_eq!(super::editor_split(0.75, -100.0, 1000.0), 0.65);
        assert_eq!(super::editor_split(0.75, 500.0, 1000.0), 0.9);
        assert_eq!(super::editor_split(0.75, -900.0, 1000.0), 0.2);
        assert_eq!(super::editor_split(0.75, 50.0, 0.0), 0.75);
    }

    #[test]
    fn enter_types_a_line_inside_an_open_code_block() {
        assert!(!super::in_code_block("plain text"));
        assert!(super::in_code_block("text\n```markdown\n# Heading"));
        assert!(super::in_code_block("```"));
        assert!(!super::in_code_block("```sh\nls\n```"));
    }
}
