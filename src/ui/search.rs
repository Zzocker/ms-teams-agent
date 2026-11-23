//! Results pane shared by global and in-conversation message search.
use crate::*;
use message_search::Filter;

pub(super) const PANEL_WIDTH: f32 = 328.0;

pub(super) fn close(s: &mut State, tx: &mut Tx) {
    let key = if s.message_search.scope.is_some() { "find-messages" } else { "search" };
    s.message_search.close();
    tx.focus(key, false);
}

fn location(hit: &message_search::Hit, conversations: &[(&Chat, Option<&str>)]) -> String {
    let (title, team) = conversations.iter().find(|(chat, _)| chat.id == hit.conversation)
        .map(|(chat, team)| (chat.title.as_str(), *team)).unwrap_or(("Conversation", None));
    team.map_or_else(|| title.to_owned(), |team| format!("{team} / {title}"))
}

pub(super) fn preview_key(hit: &message_search::Hit) -> String {
    format!("message-preview/{}/{}", hit.conversation, hit.id)
}

pub(super) fn match_spans(ranges: impl IntoIterator<Item = std::ops::Range<usize>>) -> Vec<rune::Span> {
    let (ink, background) = if dark() {
        (rune::oklch(0.94, 0.065, 95.0), rune::oklch(0.40, 0.08, 85.0))
    } else {
        (rune::oklch(0.29, 0.06, 80.0), rune::oklch(0.91, 0.11, 95.0))
    };
    ranges.into_iter().map(|range| {
        rune::Span::new(range.start, range.end).bold().color(ink).bg(background, 2.0)
    }).collect()
}

pub(super) fn preview_row(hit: &message_search::Hit, conversations: &[(&Chat, Option<&str>)]) -> El {
    let t = tok::theme(dark());
    let location = location(hit, conversations);
    let title = if hit.sender.is_empty() || hit.sender == location {
        location
    } else {
        format!("{} · {location}", hit.sender)
    };
    let time = message_time(&hit.time, chrono::Local::now().date_naive());
    let target = hit.clone();
    col().key(preview_key(hit))
        .role("button").label(format!("{title}, {time}. {}", hit.snippet)).focusable(true)
        .w_full().min_w(0.0).min_h(44.0).gap(2.0).px(10.0).py(5.0).radius(6.0)
        .hover(t.hover).cursor(rune::Cursor::Pointer)
        .on_press_tx(move |s: &mut State, tx| {
            message_search::clear_top(s);
            s.message_search.close();
            message_search::jump(s, tx, &target);
            tx.focus("find-messages", false);
        })
        .child(row().w_full().gap(8.0).items_center()
            .child(text(title).font(type_::LABEL).color(t.text).grow(1.0).min_w(0.0).truncate())
            .child(text(time).font(type_::LABEL).color(t.text_secondary)))
        .child(text(&hit.snippet).spans(match_spans(hit.matches.iter().cloned()))
            .font(type_::BODY).color(t.text_secondary).truncate())
}

pub(super) fn panel(s: &State, conversations: &[(&Chat, Option<&str>)], compact: bool, brand: rune::Color) -> El {
    let search = &s.message_search;
    let global = search.scope.is_none();
    let dismiss_on_jump = global || compact;
    let t = tok::theme(dark());
    let label = match &search.scope {
        Some(id) if notifications::conversation(s, id).is_some_and(|chat| chat.channel) => "Find in channel",
        Some(_) => "Find in chat",
        None => "Message search results",
    };
    let search_action = |key, title, action: fn(&mut State)| el().key(key).role("button")
        .label(title).focusable(true).px(10.0).py(8.0).radius(6.0).fill(t.control_off)
        .hover(t.hover).cursor(rune::Cursor::Pointer).on_press(action)
        .child(text(title).font(type_::LABEL).color(t.text));
    let hits: Vec<_> = search.hits.iter().filter(|hit| search.filter.includes(hit.channel)).collect();
    let channels_unavailable = global && s.teams.as_ref().is_some_and(Result::is_err);
    let status = if search.query.is_empty() {
        None
    } else if search.busy() {
        Some(if hits.is_empty() { "Searching message history…".to_owned() }
            else { format!("{} matches · Searching more history…", hits.len()) })
    } else {
        Some(match hits.len() {
            0 if search.has_more() || search.has_failures() || channels_unavailable => "No matches in the history searched.".into(),
            0 => "No matching messages.".into(),
            1 => "1 match".into(),
            count => format!("{count} matches"),
        })
    };
    let panel = col().label(label).h_full().min_h(0.0).min_w(0.0).gap(10.0).padding(12.0).fill(t.card);
    let panel = if global {
        panel.key("message-search-dialog").role("dialog").w_full().max_w(720.0).max_h(560.0)
            .padding(16.0).radius(12.0).border(1.0, t.separator).modal_tx(true, close)
    } else {
        let panel = panel.key("message-search-panel").role("region").border_sides([0.0, 0.0, 0.0, 1.0], t.separator);
        if compact { panel.grow(1.0) } else { panel.w(PANEL_WIDTH).min_w(PANEL_WIDTH) }
    };
    let panel = panel
        .child(row().w_full().gap(8.0).items_center()
            .child(text(label).role("heading").font(type_::TITLE).color(t.text).grow(1.0))
            .child(el().key("close-message-search").role("button").label("Close message search").focusable(true)
                .px(9.0).py(7.0).radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press_tx(close).child(text("Close").font(type_::LABEL).color(t.text_secondary))))
        .child(row().w_full().gap(6.0).items_center()
            .child(text_input(&search.input, type_::BODY).key("message-search-input").grow(1.0).min_w(0.0)
                .label("Message keywords").placeholder("Message keywords", t.text_secondary)
                .color(t.text).caret(t.text, brand.alpha(0.3)).fill(t.bg).radius(7.0)
                .border(1.0, t.separator).px(9.0).py(8.0)
                .on_edit(|s: &mut State| &mut s.message_search.input)
                .on_input_change(message_search::scoped_input_changed)
                .on_submit(|s, _| message_search::submit(s))
                .on_input_key(|s, event, tx| {
                    if event.key != rune::Key::Escape { return false; }
                    close(s, tx);
                    true
                }))
            .child(search_action("submit-message-search", "Search", message_search::submit)
                .disabled(search.input.text.trim().is_empty())))
        .children(global.then(|| {
            row().gap(6.0).children([
                (Filter::All, "All", "message-filter-all"),
                (Filter::Chats, "Chats", "message-filter-chats"),
                (Filter::Channels, "Channels", "message-filter-channels"),
            ].into_iter().map(|(filter, title, key)| {
                let on = search.filter == filter;
                el().key(key).role("button").label(title).checked(on).focusable(true).px(9.0).py(6.0)
                    .radius(6.0).fill(if on { t.control_off } else { t.card }).hover(t.hover)
                    .cursor(rune::Cursor::Pointer).on_press(move |s: &mut State| s.message_search.filter = filter)
                    .child(text(title).font(type_::LABEL).color(if on { brand } else { t.text_secondary }))
            }))
        }))
        .children(status.map(|status| text(status).key("message-search-status").role("status")
            .font(type_::LABEL).color(t.text_secondary).wrap()))
        .child(col().key("message-search-results").w_full().gap(6.0).grow(1.0).shrink().min_h(0.0)
            .children(hits.into_iter().map(|hit| {
                let location = location(hit, conversations);
                let time = message_time(&hit.time, chrono::Local::now().date_naive());
                let sender = if hit.sender.is_empty() { "Unknown sender" } else { &hit.sender };
                let target = hit.clone();
                col().key(format!("message-hit/{}/{}", hit.conversation, hit.id))
                    .role("button").label(format!("{sender}, {location}, {time}. {}", hit.snippet)).focusable(true)
                    .w_full().min_w(0.0).gap(4.0).padding(10.0).radius(8.0).fill(t.bg)
                    .hover(t.hover).cursor(rune::Cursor::Pointer)
                    .on_press_tx(move |s: &mut State, tx| {
                        if dismiss_on_jump { s.message_search.close(); }
                        message_search::jump(s, tx, &target);
                        if dismiss_on_jump { tx.focus("find-messages", false); }
                    })
                    .child(text(location).font(type_::LABEL).color(brand).truncate())
                    .child(row().w_full().gap(6.0).items_center()
                        .child(text(sender).font(type_::LABEL).color(t.text).grow(1.0).min_w(0.0).truncate())
                        .child(text(time).font(type_::LABEL).color(t.text_secondary)))
                    .child(text(&hit.snippet).spans(match_spans(hit.matches.iter().cloned()))
                        .font(type_::BODY).color(t.text).wrap().max_lines(3))
            }))
            .children(channels_unavailable.then(|| text("Some channels could not be loaded.")
                .font(type_::LABEL).color(t.text_secondary).wrap()))
            .children((!search.busy() && search.has_more()).then(|| {
                search_action("search-more-history", "Search more history", message_search::more)
            }))
            .children((!search.busy() && search.has_failures()).then(|| {
                search_action("retry-message-search", "Retry unavailable history", message_search::retry)
            }))
            .scroll(ScrollSpec { stick_bottom: false }));
    if global {
        col().w_full().h_full().center().padding(20.0).fill(t.bg.alpha(0.6))
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).child(panel)
    } else {
        panel
    }
}
