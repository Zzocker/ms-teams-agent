//! A message's text, its hover toolbar and its "…" menu.

use crate::*;

/// The hover toolbar of a message the server has (its actions need its id): edit for one of
/// mine, quote (reply) for someone else's, and "…" for the rest. It stays shown while its menu is open.
// shortcut: no reactions yet; add their buttons here when the client can send them.
pub(crate) fn message_actions(m: &Msg, shown: bool) -> Option<El> {
    static PENCIL: rune::Icon = rune::Icon { name: "msg-edit", view: 20.0, body: r#"<path d="M13.5 3.5l3 3L7 16H4v-3z M11.5 5.5l3 3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>"# };
    static QUOTE: rune::Icon = rune::Icon { name: "msg-quote", view: 20.0, body: r#"<path d="M4 11.5c0-3 1.5-5 4-5.5 M4 11.5h3.5v3.5H4z M11 11.5c0-3 1.5-5 4-5.5 M11 11.5h3.5v3.5H11z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>"# };
    static MORE: rune::Icon = rune::Icon { name: "msg-more", view: 20.0, body: r#"<circle cx="5" cy="10" r="1.3" fill="currentColor"/><circle cx="10" cy="10" r="1.3" fill="currentColor"/><circle cx="15" cy="10" r="1.3" fill="currentColor"/>"# };
    if m.server_id.is_empty() { return None }
    let t = tok::theme(dark());
    let button = |key: String, label: &'static str, icon: &'static rune::Icon| el().key(key).size(28.0, 28.0).center().radius(6.0)
        .role("button").label(label).focusable(true).cursor(rune::Cursor::Pointer).hover(t.hover)
        .child(rune::icon(icon, 16.0).color(t.text));
    let first = {
        let m = m.clone();
        if m.mine {
            button(format!("edit/{}", m.id), "Edit message", &PENCIL).on_press(move |s: &mut State| start_edit(s, &m))
        } else {
            button(format!("quote/{}", m.id), "Reply", &QUOTE).on_press(move |s: &mut State| start_reply(s, &m))
        }
    };
    let id = m.id.clone();
    let more = button(format!("more/{}", m.id), "More options", &MORE)
        .hover(t.hover)
        .on_press(move |s: &mut State| s.menu = if s.menu.as_ref() == Some(&id) { None } else { Some(id.clone()) });
    Some(row().gap(2.0).padding(3.0).radius(8.0).fill(t.card).border(1.0, t.separator)
        .shadow(2.0, 8.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 })
        // Shown for the hovered message (or its open menu); a hovered group without it is the row
        // above the pointer, which the toolbar of the one below overlaps.
        .group_opacity(if shown { 1.0 } else { 0.0 }, if shown { 1.0 } else { 0.0 })
        .child(first).child(more))
}

/// The open "…" menu, under its button: copy, plus reply, edit and delete for one of mine
/// (someone else's has reply in the toolbar).
pub(crate) fn message_menu(s: &State) -> Option<El> {
    let id = s.menu.as_deref()?;
    let m = s.messages.as_ref()?.as_ref().ok()?.msgs.iter().find(|m| m.id == id)?;
    let t = tok::theme(dark());
    let item = |key: &'static str, label: &'static str| row().key(key).role("menuitem").label(label).focusable(true)
        .w_full().px(12.0).py(6.0).radius(4.0).cursor(rune::Cursor::Pointer).hover(t.hover)
        .child(text(label).font(type_::BODY).color(t.text));
    let (reply, copy, edit, delete) = (m.clone(), m.text.clone(), m.clone(), m.clone());
    Some(col().key("message-menu").role("menu").label("Message options").min_w(160.0).padding(4.0).radius(8.0)
        .fill(t.card).border(1.0, t.separator).shadow(4.0, 16.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.4 })
        .anchored(format!("more/{id}"), Side::Bottom, Align::End, 4.0)
        .modal(true, |s: &mut State| s.menu = None)
        .children(m.mine.then(|| item("menu-reply", "Reply").on_press(move |s: &mut State| start_reply(s, &reply))))
        .child(item("menu-copy", "Copy text").on_press_tx(move |s: &mut State, tx| { tx.copy(copy.clone()); s.menu = None; }))
        .children(m.mine.then(|| item("menu-edit", "Edit").on_press(move |s: &mut State| start_edit(s, &edit))))
        .children(m.mine.then(|| item("menu-delete", "Delete").on_press(move |s: &mut State| { s.deleting = Some(delete.clone()); s.menu = None; }))))
}

/// A message's text, its web links in the accent colour and opened in the browser on a click.
pub(crate) fn message_text(m: &Msg) -> El {
    let t = tok::theme(dark());
    let link = tok::accent(0, dark());
    #[cfg(feature = "markdown")]
    let (text, links, mut spans) = {
        // As the draft's preview showed it.
        let md = markdown::parse(&m.text);
        let links: Vec<_> = m.links.iter().map(|(r, url)| (md.at(r.start)..md.at(r.end), url)).filter(|(r, _)| !r.is_empty()).collect();
        let spans = md_spans(&md);
        (md.text, links, spans)
    };
    #[cfg(not(feature = "markdown"))]
    let (text, links, mut spans) = (m.text.clone(), m.links.iter().map(|(r, url)| (r.clone(), url)).collect::<Vec<_>>(), vec![]);
    spans.extend(links.iter().map(|(r, _)| rune::Span::new(r.start, r.end).color(link)));
    links.iter().fold(rune::rich(text, spans), |el, (r, url)| {
        el.link(rune::Link::new(r.clone(), url.as_str()).hover(rune::LinkPaint::new().underline(link)).help(url.as_str()))
    })
        // Long URLs have no spaces to wrap at.
        .font(type_::BODY).color(t.text).wrap_anywhere().selectable(true)
        .on_link(|_: &mut State, url: &str, tx: &mut Tx| { let _ = tx.open_url(url); })
}

#[cfg(feature = "markdown")]
pub(crate) fn md_spans(md: &markdown::Md) -> Vec<rune::Span> {
    use markdown::Style;
    md.spans.iter().map(|(r, style)| {
        let span = rune::Span::new(r.start, r.end);
        match style {
            Style::Bold | Style::Heading => span.bold(),
            Style::Italic => span.italic(),
            Style::Code => span.mono().bg(tok::theme(dark()).press, 2.0),
            Style::Block => span.mono(),
        }
    }).collect()
}
