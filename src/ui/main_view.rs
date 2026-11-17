//! The signed-in window: navigation rail, chat list, messages and search.

use crate::*;

pub(crate) fn main_view(s: &State, session: &Session, chats: &[Chat]) -> El {
    let t = tok::theme(dark());
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary);

    let me = col().gap(2.0).grow(1.0).min_w(0.0)
        .child(text(session.user.name.as_str()).font(type_::TITLE).color(t.text).truncate())
        .child(note(&session.user.email).truncate());
    let header = row().w_full().gap(10.0).padding(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(user_avatar(s, &session.user.id, &session.user.name, 36.0)).child(me)
        .child(el().key("sign-out").role("button").label("Sign out").focusable(true).px(10.0).py(6.0).radius(6.0)
            .fill(t.control_off).hover(t.hover).cursor(rune::Cursor::Pointer).on_press(sign_out)
            .child(text("Sign out").font(type_::LABEL).color(t.text)));
    let chat_row = |c: &Chat, key: &str| {
        let id = c.id.clone();
        let picked = s.selected.as_ref() == Some(&c.id);
        let title_font = if c.unread { rune::tok::TextStyle::new(13.0, 700.0) } else { type_::BODY };
        row().key(format!("{key}/{}", c.id)).role("button").label(if c.unread { format!("{}, unread messages", c.title) } else { c.title.clone() }).focusable(true)
            .w_full().gap(10.0).px(if s.show_teams { 30.0 } else { 10.0 }).py(8.0).radius(8.0).items_center()
            .fill(if picked { t.press } else { t.card }).hover(t.hover).cursor(rune::Cursor::Pointer)
            .on_press(move |s: &mut State| {
                s.search.clear();
                open_chat(s, id.clone());
            })
            .children((!s.show_teams).then(|| chat_avatar(s, c, &session.user.id, 36.0)))
            .child(col().gap(2.0).grow(1.0).min_w(0.0)
                .child(row().w_full().gap(8.0)
                    .child(text(c.title.as_str()).font(title_font).color(if c.unread { t.text } else { t.text_secondary }).truncate().grow(1.0).min_w(0.0))
                    .children((!c.time.is_empty()).then(|| text(&chat_time(&c.time, chrono::Local::now())).font(if c.unread { title_font } else { type_::LABEL }).color(if c.unread { t.text } else { t.text_secondary }))))
                .child(row().w_full().gap(8.0).items_center()
                    .children((!c.preview.is_empty()).then(|| text(&c.preview).font(if c.unread { type_::TITLE } else { type_::BODY }).color(if c.unread { t.text } else { t.text_secondary }).truncate().grow(1.0).min_w(0.0)))
                    .children(c.unread.then(|| el().size(8.0, 8.0).radius(4.0).fill(tok::accent(0, dark()))))))
    };
    let mut rows: Vec<El> = Vec::new();
    if s.show_teams {
        match &s.teams {
            None => rows.push(el().w_full().center().padding(20.0).motion(rune::Spring::NONE)
                .child(loading_messages().label("Loading teams"))),
            Some(Err(e)) => {
                rows.push(note(e).wrap().padding(12.0));
                rows.push(button("retry-teams", "Retry", false, |s| {
                    s.teams = None;
                    navigate(s, true);
                }));
            }
            Some(Ok(teams)) => {
                rows.push(note("Teams and channels").padding(10.0));
                for team in teams {
                    let id = team.id.clone();
                    let collapsed = s.collapsed_teams.contains(&id);
                    rows.push(row().key(format!("team/{id}")).role("button").label(team.title.as_str())
                        .focusable(true).w_full().gap(8.0).padding(10.0).items_center().hover(t.hover).cursor(rune::Cursor::Pointer)
                        .on_press(move |s: &mut State| {
                            if !s.collapsed_teams.remove(&id) { s.collapsed_teams.insert(id.clone()); }
                        })
                        .child(note(if collapsed { "›" } else { "⌄" }))
                        .child(avatar(&team.title, 24.0))
                        .child(text(team.title.as_str()).font(type_::TITLE).color(t.text).truncate().min_w(0.0)));
                    if !collapsed {
                        if team.channels.is_empty() { rows.push(note("No channels.").px(24.0).py(6.0)); }
                        rows.extend(team.channels.iter().map(|c| chat_row(c, "channel")));
                    }
                }
                if teams.is_empty() { rows.push(note("No teams yet.").padding(12.0)); }
            }
        }
    } else {
        rows.extend(chats.iter().map(|c| chat_row(c, "chat")));
    }
    let visible: Vec<&Chat> = if s.show_teams {
        s.teams.as_ref().and_then(|r| r.as_ref().ok()).into_iter()
            .flatten().flat_map(|team| &team.channels).collect()
    } else { chats.iter().collect() };
    // Results float under the search bar, over the chat list, like Teams.
    let query = s.search.text.trim().to_lowercase();
    let results = (!query.is_empty()).then(|| {
        let hits: Vec<El> = visible.iter().filter(|c| c.title.to_lowercase().contains(&query)).map(|c| chat_row(c, "hit")).collect();
        col().key("results").w(560.0).max_h(480.0).shrink().min_h(0.0).padding(6.0).gap(2.0).radius(8.0).fill(t.card)
            .border(1.0, t.separator)
            .anchored_fit("search-box", Side::Bottom, Align::Start, 4.0)
            .child(note(if s.show_teams { "Channels" } else { "Chats" }).font(type_::LABEL).px(10.0).py(4.0))
            .children(hits.is_empty().then(|| note("No matches.").px(10.0).py(8.0)))
            .child(col().w_full().gap(2.0).children(hits).scroll(ScrollSpec { stick_bottom: false }).shrink().min_h(0.0))
    });
    // Top bar, not the sidebar: search will cover messages too, not just chat names.
    let search = row().w_full().justify_center().px(16.0).py(8.0).fill(t.card).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(col().key("search-box").w_full().max_w(560.0).shrink().px(12.0).py(6.0).radius(8.0).fill(t.bg)
            .child(text_input(&s.search, type_::BODY).key("search").label("Search").color(t.text)
                .placeholder("Search", t.text_secondary)
                .caret(t.text, tok::accent(0, dark()).alpha(0.3))
                .on_edit(|s: &mut State| &mut s.search)
                .on_key(|s: &mut State, e: &rune::KeyEvent, tx| {
                    if e.key != rune::Key::Escape { return false }
                    s.search.clear();
                    tx.blur();
                    true
                })));
    let search = col().key("search-area").w_full().child(search);
    let search = if query.is_empty() { search } else {
        search.modal_tx(true, |s: &mut State, tx| { s.search.clear(); tx.blur(); })
    };
    let list = with_bar("chats", col().w_full().gap(2.0).padding(6.0).children(rows)
        .scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0), &s.chat_bar);
    let sidebar = col().key("sidebar").w(s.panel_width).min_w(0.0).h_full().min_h(0.0)
        .motion(rune::Spring::NONE).clip_rounded(0.0).fill(t.card)
        .child(header)
        .children(s.live_status.as_deref().map(|status| note(status).wrap().padding(12.0)))
        .child(text(if s.show_teams { "Teams" } else { "Chat" }).font(type_::TITLE).color(t.text).padding(12.0))
        .child(list);
    let divider = col().key("panel-resize").w(8.0).h_full().center().fill(t.card)
        .motion(rune::Spring::NONE).cursor(rune::Cursor::ResizeColumn)
        .label("Resize Chat / Teams panel with Left and Right arrows").focusable(true)
        .on_key(|s: &mut State, e: &rune::KeyEvent, _| {
            let delta = match e.key {
                rune::Key::Left => -20.0,
                rune::Key::Right => 20.0,
                _ => return false,
            };
            s.panel_width = panel_width(s.panel_width, delta);
            true
        })
        .on_drag(|s: &mut State, d: &mut Drag, _| {
            if d.phase == Phase::Down { d.data = s.panel_width; }
            s.panel_width = if d.cancelled() { d.data } else { panel_width(d.data, d.dx) };
        })
        .child(el().w(1.0).h_full().fill(t.text_secondary));
    static CHAT_ICON: rune::Icon = rune::Icon {
        name: "nav-chat", view: 24.0,
        body: r#"<path d="M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-6 4V6a2 2 0 0 1 2-2Z M7 9h10 M7 13h7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"#,
    };
    static TEAMS_ICON: rune::Icon = rune::Icon {
        name: "nav-teams", view: 24.0,
        body: r#"<circle cx="9" cy="7" r="3" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="18" cy="8" r="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M3 21v-5a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v5 M16 13h2a3 3 0 0 1 3 3v4" fill="none" stroke="currentColor" stroke-width="1.5"/>"#,
    };
    let rail = col().key("navigation").w(68.0).h_full().gap(8.0).px(4.0).py(12.0)
        .fill(t.bg).border_sides([0.0, 1.0, 0.0, 0.0], t.separator)
        .children([(false, "Chat", &CHAT_ICON), (true, "Teams", &TEAMS_ICON)].into_iter().map(|(teams, label, icon)| {
            col().key(label).role("button").label(label).focusable(true).w_full().center().gap(4.0).py(10.0).radius(6.0)
                .fill(if s.show_teams == teams { t.press } else { t.bg }).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut State| if s.show_teams != teams { navigate(s, teams) })
                .child(rune::icon(icon, 24.0).color(t.text))
                .child(text(label).font(type_::LABEL).color(t.text))
        }))
        .child(el().grow(1.0))
        .child(theme_button(s.theme))
        .child(releases::nav_button(s));

    let pane = col().grow(1.0).shrink().h_full().min_w(0.0).min_h(0.0).fill(t.bg);
    let pane = match (&s.selected, &s.messages) {
        (None, _) => pane.center().child(note(if s.show_teams { "Pick a channel." } else if chats.is_empty() { "No chats yet." } else { "Pick a chat." })),
        (Some(_), None) => {
            pane.key("messages-loading").center().motion(rune::Spring::NONE)
                .child(loading_messages())
        },
        (Some(_), Some(Err(e))) => pane.center().padding(32.0).child(note(e).wrap()),
        (Some(id), Some(Ok(page))) => {
            let msgs = &page.msgs;
            // Fetch older a screenful before the top, so it's usually in before the scroll gets there.
            let edge = if page.older.is_some() { Some(15.min(msgs.len().saturating_sub(1))) } else { None };
            let title = chats.iter().chain(s.teams.as_ref().and_then(|r| r.as_ref().ok()).into_iter().flatten().flat_map(|team| &team.channels)).find(|c| &c.id == id).map_or("Chat", |c| c.title.as_str());
            let bubbles = msgs.iter().enumerate().flat_map(|(i, m)| {
                let prev = i.checked_sub(1).map(|j| &msgs[j]);
                let new_day = prev.is_none_or(|p| day(&p.time) != day(&m.time));
                // Nearby messages from one sender share an avatar, name and timestamp.
                let head = prev.is_none_or(|p| !same_message_group(p, m));
                let time = note(&message_time(&m.time, chrono::Local::now().date_naive())).font(type_::LABEL);
                let bubble = col().max_w(860.0).shrink().min_w(0.0).px(12.0).py(8.0).radius(12.0)
                    .fill(if m.mine { tok::accent(0, dark()).alpha(0.3) } else { t.card })
                    .border(2.0, tok::accent(0, dark()).alpha(if s.reply_highlight_on && s.reply_highlight.as_ref().is_some_and(|(id, _)| id == &m.id) { 1.0 } else { 0.0 }))
                    .children(m.reply.as_ref().map(|reply| {
                        let target = reply.id.clone();
                        // A set width: sized from its content, the quote measured its wrapped text at one width and drew it at another, spilling out of a short reply's bubble.
                        row().w(400.0).shrink().min_w(0.0).gap(10.0).padding(8.0).mb(6.0).radius(4.0).fill(t.bg)
                            .border(1.0, t.separator)
                            .cursor(rune::Cursor::Pointer).hover(t.hover)
                            .label("Go to quoted message").focusable(true)
                            .on_press_tx(move |s: &mut State, tx| {
                                s.reply_highlight = None;
                                s.reply_error = None;
                                s.reply_target = Some(target.clone());
                                reveal_reply(s, tx);
                            })
                            .child(el().w(2.0).radius(1.0).fill(t.text_secondary))
                            .child(col().gap(2.0).grow(1.0).shrink().min_w(0.0)
                                .child(note(&format!("{}  {}", reply.sender, local_time(&reply.time, "%Y-%m-%d %I:%M %p"))).font(type_::LABEL).truncate())
                                .child(text(reply.text.as_str()).font(type_::BODY).color(t.text).max_lines(2)))
                    }))
                    .children((!m.text.is_empty()).then(|| message_text(m)))
                    .children((!m.images.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0).mt(if m.text.is_empty() { 0.0 } else { 8.0 })
                        .children(m.images.iter().map(|url| message_image(s, url)))))
                    .children((!m.files.is_empty()).then(|| col().gap(8.0).mt(if m.text.is_empty() && m.images.is_empty() { 0.0 } else { 8.0 })
                        .children(m.files.iter().map(|f| file_card(s, f)))));
                // Keyed by id, not index: the 50-message window slides on a new message, and index keys would morph each bubble into its neighbour.
                let hover_id = m.id.clone();
                let line = row().key(format!("msg/{}", m.id)).hover_group_shared(format!("msg/{}", m.id)).arrives_whole()
                    .on_hover_change(move |s: &mut State, on, _| {
                        if on { s.hovered = Some(hover_id.clone()) } else if s.hovered.as_ref() == Some(&hover_id) { s.hovered = None }
                    }).w_full().gap(8.0).pt(if head { 12.0 } else { 0.0 });
                #[cfg(feature = "image")]
                let line = if !m.images.is_empty() || m.files.iter().any(|f| f.preview.is_some()) {
                    let id = m.id.clone();
                    line.on_drawn_layout_tx(move |s: &mut State, rect, tx| {
                        s.preview_rows.insert(id.clone(), rect);
                        tx.live_only();
                    })
                } else { line };
                let line = if edge == Some(i) {
                    line.on_visible(|s: &mut State, on, _| if on { load_older(s) })
                } else {
                    line
                };
                // Only a message the server has: an edit needs its id and HTML.
                // Over the bubble's top-right corner while the message is hovered (or focused), as in Teams.
                // The toolbar reaches above the row, so it shares the row's hover: the pointer on it
                // keeps this message's actions, not the one above.
                let shown = s.menu.as_ref() == Some(&m.id) || s.hovered_bar.as_ref().or(s.hovered.as_ref()) == Some(&m.id);
                let bar_id = m.id.clone();
                let actions = message_actions(m, shown)
                    .map(|a| el().hover_group_shared(format!("msg/{}", m.id)).pin(None, Some(-30.0), Some(0.0), None)
                        .on_hover_change(move |s: &mut State, on, _| {
                            if on { s.hovered_bar = Some(bar_id.clone()) } else if s.hovered_bar.as_ref() == Some(&bar_id) { s.hovered_bar = None }
                        })
                        .child(a));
                let bubble = bubble.children(actions);
                let content = col().gap(4.0).shrink().min_w(0.0);
                let content = if m.mine { content.items_end() } else { content }
                    .children(head.then(|| row().gap(8.0).items_center()
                        .child(text(m.sender.as_str()).font(type_::LABEL).color(t.text_secondary))
                        .child(time)))
                    .child(bubble);
                let line = if m.mine {
                    line.justify_end().items_end().child(content)
                } else {
                    line.child(if head { user_avatar(s, &m.sender_id, &m.sender, 32.0) } else { el().w(32.0) })
                        .child(content)
                };
                let divider = new_day.then(|| row().key(format!("day/{}", m.id)).arrives_whole().w_full().justify_center().pt(10.0)
                    .child(note(&message_date(&m.time, chrono::Local::now().date_naive())).font(type_::LABEL)));
                divider.into_iter().chain([line])
            });
            let list = col().w_full().items_center().px(16.0).py(8.0)
                .child(col().w_full().max_w(1040.0).gap(4.0).growth(GrowthSpec::default())
                    .children(bubbles)
                    .children(msgs.is_empty().then(|| note("No messages."))))
                .scroll(ScrollSpec { stick_bottom: true }).scroll_anchor().grow(1.0).min_h(0.0);
            #[cfg(feature = "image")]
            let list = list.drawn_rect_into(&s.preview_viewport);
            pane.child(row().w_full().gap(10.0).px(16.0).py(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                    .child(chats.iter().find(|c| &c.id == id).map_or_else(|| avatar(title, 32.0), |c| chat_avatar(s, c, &session.user.id, 32.0)))
                    .child(text(title).font(type_::TITLE).color(t.text).truncate()))
                // A growth stack, so a sent message and the viewport following it glide in as one.
                .child(with_bar("messages", list, &s.msg_bar).key(format!("history/{id}"))
                    .on_layout_tx(|s: &mut State, _, tx| {
                        if let Some(at) = s.restore_scroll.take() {
                            tx.instant();
                            if at.max[1] - at.offset[1] < 1.0 {
                                tx.scroll_to_bottom_instant("messages");
                            } else {
                                tx.scroll_to_instant("messages", 0.0, at.offset[1]);
                            }
                        }
                    }))
                .child(col().w_full().items_center().px(16.0).py(10.0).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
                    .child(col().w_full().max_w(1040.0).gap(6.0)
                    .children(s.reply_error.as_deref().map(|e| note(e).wrap()))
                    .children(s.send_error.as_deref().map(|e| note(e).wrap()))
                    .children(s.deleting.as_ref().map(|m| row().w_full().gap(12.0).items_center()
                        .child(note(&format!("Delete \"{}\"?", m.text.chars().take(60).collect::<String>())).font(type_::LABEL).truncate().shrink().min_w(0.0))
                        .child(el().key("delete-confirm").role("button").label("Delete message").focusable(true).cursor(rune::Cursor::Pointer)
                            .on_press(delete_message)
                            .child(text("Delete").font(type_::LABEL).color(rune::Color { r: 0.95, g: 0.35, b: 0.35, a: 1.0 })))
                        .child(el().key("delete-cancel").role("button").label("Keep message").focusable(true).cursor(rune::Cursor::Pointer)
                            .on_press(|s: &mut State| s.deleting = None)
                            .child(text("Cancel").font(type_::LABEL).color(tok::accent(0, dark()))))))
                    .children(s.replying.as_ref().map(|m| row().w_full().gap(8.0).items_center().pl(8.0).border_sides([0.0, 0.0, 0.0, 2.0], tok::accent(0, dark()))
                        .child(col().grow(1.0).shrink().min_w(0.0)
                            .child(note(&format!("Replying to {}", m.sender)).font(type_::LABEL))
                            .child(text(m.text.replace('\n', " ")).font(type_::LABEL).color(t.text).truncate()))
                        .child(el().key("reply-cancel").role("button").label("Cancel reply").focusable(true).cursor(rune::Cursor::Pointer)
                            .on_press(|s: &mut State| s.replying = None)
                            .child(text("Cancel").font(type_::LABEL).color(tok::accent(0, dark()))))))
                    .children(s.editing.is_some().then(|| row().w_full().gap(8.0).items_center()
                        .child(note("Editing message").font(type_::LABEL))
                        .child(el().key("edit-cancel").role("button").label("Cancel edit").focusable(true).cursor(rune::Cursor::Pointer)
                            .on_press(|s: &mut State| { s.editing = None; s.draft.clear(); })
                            .child(text("Cancel").font(type_::LABEL).color(tok::accent(0, dark()))))))
                    .child(row().w_full().gap(8.0).items_end()
                        .child(col().grow(1.0).shrink().min_w(0.0).gap(8.0).px(12.0).py(8.0).radius(8.0).fill(t.card)
                            .children((!s.pasted.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0)
                                .children(s.pasted.iter().enumerate().map(|(i, (_, image))| pasted_image(image, i)))))
                            .children((!s.editor).then(|| mention_suggestions(s)).flatten())
                            // One box shows the draft at a time; behind the editor it would echo every key.
                            .child(if s.editor { note("Writing in the editor…") } else {
                                auto_grow(draft_paste(text_input(&s.draft, type_::BODY).key("draft").label("Message").color(t.text)
                                    .placeholder("Type a message", t.text_secondary)
                                    .caret(t.text, tok::accent(0, dark()).alpha(0.3))
                                    .on_edit(|s: &mut State| &mut s.draft)
                                    .on_input_change(mention_changed)
                                    .on_input_key(mention_key)
                                    .on_submit(|s: &mut State, _| if in_code_block(&s.draft.text[..s.draft.range().0]) { s.draft.insert("\n") } else { send(s) })), 1, 6)
                            })
                            .children(composer_preview(s)))
                        .child(button("editor", "Editor", false, |s| s.editor = true))
                        .child(button("send", "Send", true, send)))))
        }
    };
    let body = row().w_full().grow(1.0).min_h(0.0).child(rail).child(sidebar).child(divider).child(pane);
    // Rune's modal scope includes later siblings, so keep the background outside search inert.
    let body = if query.is_empty() { body } else { body.inert() };
    col().w_full().h_full().fill(t.bg).child(search).child(body)
        .children(results)
}

pub(crate) fn search_key(s: &mut State, e: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if !matches!(s.screen, Screen::Main { .. }) || s.editor || s.viewer.is_some() || s.menu.is_some() || s.release_panel {
        return false;
    }
    if !rune::Keybind::new("ctrl+alt+e").matches_tx(e, tx) { return false }
    tx.focus("search", false);
    true
}

pub(crate) const PANEL_MAX: f32 = 480.0;

pub(crate) fn panel_width(start: f32, delta: f32) -> f32 {
    (start + delta).clamp(0.0, PANEL_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_shortcut_and_dismissal() {
        let mut s = init();
        demo(&mut s);
        let mut rt = rune::Runtime::new(s, view, (1000.0, 680.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(search_key);
        rt.frame(0.0);
        rt.update(|_, tx| tx.focus("draft", false));
        rt.frame(0.0);
        let draft = rt.state.draft.text.clone();
        rt.key(rune::KeyEvent { key: rune::Key::Command('e'), mods: rune::Mods { ctrl: true, cmd: !cfg!(target_os = "macos"), alt: true, ..Default::default() } });
        rt.frame(0.0);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("search") && e.focused));
        assert_eq!(rt.state.draft.text, draft);
        for ch in "Charles".chars() {
            rt.key(rune::KeyEvent { key: rune::Key::Text(ch.to_string()), mods: Default::default() });
            rt.frame(0.0);
        }
        assert_eq!(rt.state.search.text, "Charles");
        let hits: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("hit/"))).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key.as_deref(), Some("hit/a"));
        assert!(hits[0].rect[2] > 0.0 && hits[0].rect[3] > 0.0, "matching result must be visible: {:?}", hits[0].rect);
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let hit = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("hit/a")).unwrap();
        let picked = rt.element_at(hit.rect[0] + hit.rect[2] / 2.0, hit.rect[1] + hit.rect[3] / 2.0).unwrap();
        assert!(picked.path().starts_with(&hit.path), "results must be above the conversation pane: {}", picked.path());
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("results")));
        rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() });
        rt.frame(0.0);
        assert!(rt.state.search.text.is_empty());
        rt.update(|s, _| s.search.replace_text("no matches"));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let input = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("search")).unwrap().rect;
        rt.pointer_down(input[0] + 10.0, input[1] + 5.0);
        rt.pointer_up(input[0] + 10.0, input[1] + 5.0);
        rt.frame(0.0);
        assert!(!rt.state.search.text.is_empty(), "clicking the search input must keep results open");
        rt.pointer_down(900.0, 650.0);
        rt.pointer_up(900.0, 650.0);
        rt.frame(0.0);
        assert!(rt.state.search.text.is_empty());
    }

    #[test]
    fn panel_resize_limits_and_reopening() {
        assert_eq!(panel_width(300.0, 60.0), 360.0);
        assert_eq!(panel_width(300.0, -500.0), 0.0);
        assert_eq!(panel_width(0.0, 80.0), 80.0);
        assert_eq!(panel_width(300.0, 500.0), 480.0);
        assert_eq!(panel_width(480.0, -40.0), 440.0);
    }
}
