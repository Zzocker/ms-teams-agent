//! The signed-in window: navigation rail, chat list, messages and search.

use crate::*;

pub(crate) fn main_view(s: &State, session: &Session, chats: &[Chat]) -> El {
    let t = tok::theme(dark());
    static ATTACH: rune::Icon = rune::Icon { name: "composer-attach", view: 24.0, body: r#"<path d="m8 12 6-6a3 3 0 0 1 4 4l-8 8a5 5 0 0 1-7-7l9-9 M6 14l8-8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static SEND: rune::Icon = rune::Icon { name: "composer-send", view: 24.0, body: r#"<path d="m3 3 18 9-18 9 3-9z M6 12h15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"# };
    let brand = rune::oklch(if dark() { 0.76 } else { 0.49 }, 0.16, 285.0);
    let selected = rune::oklch(if dark() { 0.32 } else { 0.95 }, 0.035, 285.0);
    let sent = rune::oklch(if dark() { 0.38 } else { 0.9 }, 0.065, 285.0);
    let can_send = !s.sender.is_pending() && (!s.draft.text.trim().is_empty() || !s.pasted.is_empty() || !s.attached.is_empty());
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary);

    let chat_row = |c: &Chat, key: &str| {
        let id = c.id.clone();
        let picked = s.selected.as_ref() == Some(&c.id);
        let title_font = if c.unread { rune::tok::TextStyle::new(13.0, 700.0) } else { type_::BODY };
        row().key(format!("{key}/{}", c.id)).role("button").label(if c.unread { format!("{}, unread messages", c.title) } else { c.title.clone() }).focusable(true)
            .w_full().gap(10.0).px(if s.show_teams { 30.0 } else { 10.0 }).py(10.0).radius(6.0).items_center()
            .fill(if picked { selected } else { t.card }).border_sides([0.0, 0.0, 0.0, 3.0], if picked { brand } else { t.card }).hover(t.hover).cursor(rune::Cursor::Pointer)
            .on_press(move |s: &mut State| {
                s.search.clear();
                open_chat(s, id.clone());
            })
            .children((!s.show_teams).then(|| chat_avatar(s, c, &session.user.id, 36.0)))
            .child(col().gap(2.0).grow(1.0).min_w(0.0)
                .child(row().w_full().gap(8.0)
                    .child(text(c.title.as_str()).font(title_font).color(t.text).truncate().grow(1.0).min_w(0.0))
                    .children((!c.time.is_empty()).then(|| text(&chat_time(&c.time, chrono::Local::now())).font(if c.unread { title_font } else { type_::LABEL }).color(if c.unread { t.text } else { t.text_secondary }))))
                .child(row().w_full().gap(8.0).items_center()
                    .children((!c.preview.is_empty()).then(|| text(&c.preview).font(type_::LABEL).color(if c.unread { t.text } else { t.text_secondary }).truncate().grow(1.0).min_w(0.0)))
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
        if chats.is_empty() { rows.push(note("Your recent conversations will appear here.").wrap().padding(16.0)); }
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
            .children(hits.is_empty().then(|| note("No matching names. Try a different name.").px(10.0).py(8.0)))
            .child(col().w_full().gap(2.0).children(hits).scroll(ScrollSpec { stick_bottom: false }).shrink().min_h(0.0))
    });
    // Search stays in the top bar while results follow the current navigation section.
    let search = row().w_full().gap(20.0).items_center().px(20.0).py(10.0).fill(t.card).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(col().w(180.0).min_w(180.0).grow(1.0)
            .child(text("Teams").font(rune::tok::TextStyle::new(18.0, 700.0)).color(t.text)))
        .child(row().key("search-box").items_center().gap(8.0).w_full().max_w(560.0).min_w(0.0).shrink().px(12.0).py(6.0).radius(8.0).fill(t.bg)
            .child(text_input(&s.search, type_::BODY).key("search").grow(1.0).min_w(0.0).label(if s.show_teams { "Search channel names" } else { "Search chat names" }).color(t.text)
                .placeholder(if s.show_teams { "Search channel names" } else { "Search chat names" }, t.text_secondary)
                .caret(t.text, tok::accent(0, dark()).alpha(0.3))
                .on_edit(|s: &mut State| &mut s.search)
                .on_key(|s: &mut State, e: &rune::KeyEvent, tx| {
                    if e.key != rune::Key::Escape { return false }
                    s.search.clear();
                    tx.blur();
                    true
                }))
            .children((!query.is_empty()).then(|| el().key("clear-search").role("button").label("Clear search").focusable(true)
                .px(8.0).py(4.0).radius(4.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press_tx(|s: &mut State, tx| { s.search.clear(); tx.focus("search", false); })
                .child(text("Clear").font(type_::LABEL).color(t.text_secondary)))))
        .child(row().w(180.0).min_w(180.0).grow(1.0).gap(20.0).items_center().justify_end()
            .child(el().key("sign-out").role("button").label("Sign out").focusable(true)
                .px(10.0).py(6.0).radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer).on_press(sign_out)
                .child(text("Sign out").font(type_::LABEL).color(t.text_secondary)))
            .child(user_avatar(s, &session.user.id, &session.user.name, 32.0)));
    let search = col().key("search-area").w_full().child(search);
    let search = if query.is_empty() { search } else {
        search.modal_tx(true, |s: &mut State, tx| { s.search.clear(); tx.blur(); })
    };
    let list = with_bar("chats", col().w_full().gap(2.0).padding(6.0).children(rows)
        .scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0), &s.chat_bar);
    let sidebar = col().key("sidebar").w(s.panel_width).min_w(0.0).h_full().min_h(0.0)
        .motion(rune::Spring::NONE).clip_rounded(0.0).fill(t.card)
        .children(s.live_status.as_deref().map(|status| note(status).wrap().padding(12.0)))
        .child(text(if s.show_teams { "Teams" } else { "Chat" }).font(rune::tok::TextStyle::new(22.0, 700.0)).color(t.text).px(20.0).py(20.0))
        .child(note(if s.show_teams { "Your teams" } else { "Recent chats" }).font(type_::LABEL).px(20.0).py(8.0))
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
            settings::save(s);
            true
        })
        .on_drag(|s: &mut State, d: &mut Drag, _| {
            if d.phase == Phase::Down { d.data = s.panel_width; }
            s.panel_width = if d.cancelled() { d.data } else { panel_width(d.data, d.dx) };
            if d.phase == Phase::Up && !d.cancelled() { settings::save(s); }
        })
        .child(el().w(1.0).h_full().fill(t.separator));
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
                .fill(if s.show_teams == teams { selected } else { t.bg }).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut State| if s.show_teams != teams { navigate(s, teams) })
                .child(rune::icon(icon, 24.0).color(if s.show_teams == teams { brand } else { t.text_secondary }))
                .child(text(label).font(type_::LABEL).color(if s.show_teams == teams { brand } else { t.text_secondary }))
        }))
        .child(el().grow(1.0))
        .child(releases::nav_button(s));

    let pane = col().grow(1.0).shrink().h_full().min_w(0.0).min_h(0.0).fill(t.bg);
    let pane = match (&s.selected, &s.messages) {
        (None, _) => pane.center().padding(24.0).child(col().key("conversation-empty").w_full().max_w(360.0).gap(12.0)
            .child(rune::icon(if s.show_teams { &TEAMS_ICON } else { &CHAT_ICON }, 40.0).color(brand))
            .child(text(if s.show_teams { "Connect with your team" } else { "Your conversations, together" }).font(rune::tok::TextStyle::new(22.0, 600.0)).color(t.text).wrap())
            .child(note(if s.show_teams { "Choose a channel from the sidebar to catch up with your team." } else { "Choose a chat from the sidebar, or search for a conversation by name." }).wrap())),
        (Some(_), None) => {
            pane.key("messages-loading").center().motion(rune::Spring::NONE)
                .gap(12.0).child(loading_messages()).child(note("Loading conversation…"))
        },
        (Some(_), Some(Err(e))) => pane.center().padding(24.0).child(col().key("conversation-error").w_full().max_w(360.0).gap(12.0)
            .child(text("Couldn’t load this conversation").font(type_::TITLE).color(t.text).wrap())
            .child(note(e).wrap())
            .child(button("retry-messages", "Try again", false, |s| {
                if let Some(id) = s.selected.clone() { open_chat(s, id); }
            }))),
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
                let time = message_time(&m.time, chrono::Local::now().date_naive());
                let time = note(&format!("{time}{}", if m.edited { " · Edited" } else { "" }))
                    .key(format!("time/{}", m.id)).font(type_::LABEL).wrap().shrink().min_w(0.0);
                #[cfg(feature = "image")]
                let emojis = teams::animated_emojis(&m.raw);
                #[cfg(not(feature = "image"))]
                let emojis: Vec<(String, String, u16)> = vec![];
                let bubble = col().key(format!("bubble/{}", m.id)).max_w(720.0).shrink().min_w(0.0).px(12.0).py(10.0).radius(6.0)
                    .fill(if !emojis.is_empty() && m.images.is_empty() && m.files.is_empty() { t.bg } else if m.mine { sent } else { t.card })
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
                                .child(note(&format!("{}  {}", if reply.sender_id == session.user.id { &session.user.name } else { &reply.sender }, local_time(&reply.time, "%Y-%m-%d %I:%M %p"))).font(type_::LABEL).truncate())
                                .child(text(reply.text.as_str()).font(type_::BODY).color(t.text).max_lines(2)))
                    }))
                    .children((!m.text.is_empty() && emojis.is_empty()).then(|| message_text(m)))
                    .children((!m.images.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0).mt(if m.text.is_empty() { 0.0 } else { 8.0 })
                        .children(m.images.iter().map(|url| message_image(s, url)))))
                    .children((!m.files.is_empty()).then(|| col().gap(8.0).mt(if m.text.is_empty() && m.images.is_empty() { 0.0 } else { 8.0 })
                        .children(m.files.iter().map(|f| file_card(s, f)))));
                #[cfg(feature = "image")]
                let bubble = bubble.children((!emojis.is_empty()).then(|| row().flex_wrap().gap(4.0)
                    .children(emojis.iter().enumerate().map(|(i, (url, alt, size))| message_emoji(s, url, alt, *size).key(format!("emoji/{}/{i}", m.id))))));
                // Keyed by id, not index: the 50-message window slides on a new message, and index keys would morph each bubble into its neighbour.
                let hover_id = m.id.clone();
                let bubble = bubble.hover_group_shared(format!("msg/{}", m.id))
                    .on_hover_change(move |s: &mut State, on, _| {
                        if on { s.hovered = Some(hover_id.clone()) } else if s.hovered.as_ref() == Some(&hover_id) { s.hovered = None }
                    });
                let line = row().key(format!("msg/{}", m.id)).arrives_whole()
                    .w_full().gap(8.0).pt(if head { 12.0 } else { 0.0 });
                #[cfg(feature = "image")]
                let line = if !m.images.is_empty() || !emojis.is_empty() || m.files.iter().any(|f| f.preview.is_some()) {
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
                // The toolbar reaches above the bubble, so it shares the bubble's hover: the pointer on it
                // keeps this message's actions, not the one above.
                let shown = s.menu.as_ref() == Some(&m.id) || s.hovered_bar.as_ref().or(s.hovered.as_ref()) == Some(&m.id);
                let bar_id = m.id.clone();
                let actions = message_actions(m, shown)
                    .map(|a| el().hover_group_shared(format!("msg/{}", m.id)).pin(None, Some(-30.0), Some(0.0), None)
                        .on_hover_change(move |s: &mut State, on, _| {
                            if on { s.hovered_bar = Some(bar_id.clone()) } else if s.hovered_bar.as_ref() == Some(&bar_id) { s.hovered_bar = None }
                        })
                        .child(a));
                let delivery = message_state::delivery(s, m);
                let bubble = bubble.children(message_reactions(s, m)).children(actions)
                    .children(delivery.filter(|label| *label != "Sending…").map(|label|
                        el().pin(None, None, Some(-28.0), Some(0.0)).child(message_delivery(m, label))));
                let content = col().gap(4.0).shrink().min_w(0.0);
                let content = if m.mine { content.items_end() } else { content }
                    .children((head || m.edited).then(|| row().gap(8.0).items_center().shrink().min_w(0.0)
                        .children((head && !m.mine).then(|| text(m.sender.as_str()).font(type_::LABEL).color(t.text).truncate().shrink().min_w(0.0)))
                        .child(time)))
                    .child(bubble)
                    .children((delivery == Some("Sending…")).then(|| note("Sending…").font(type_::LABEL)))
                    .children((m.deleted && m.mine).then(|| {
                        let m = m.clone();
                        el().key(format!("undo/{}", m.id)).role("button").label("Undo delete").focusable(true)
                            .cursor(rune::Cursor::Pointer).px(8.0).py(4.0).radius(4.0).hover(t.hover)
                            .on_press(move |s: &mut State| message_state::undo(s, &m))
                            .child(note(if s.message_state.restorer.is_pending() { "Restoring…" } else { "Undo" }).font(type_::LABEL))
                    }));
                let line = if m.mine {
                    line.justify_end().items_end().child(content)
                        .child(el().w(20.0))
                } else {
                    line.child(if head { user_avatar(s, &m.sender_id, &m.sender, 32.0) } else { el().w(32.0) })
                        .child(content)
                };
                let divider = new_day.then(|| row().key(format!("day/{}", m.id)).arrives_whole().w_full().gap(12.0).items_center().pt(16.0).pb(8.0)
                    .child(el().h(1.0).grow(1.0).fill(t.separator))
                    .child(note(&message_date(&m.time, chrono::Local::now().date_naive())).font(type_::LABEL))
                    .child(el().h(1.0).grow(1.0).fill(t.separator)));
                divider.into_iter().chain([line])
            });
            let list = col().w_full().items_center().px(28.0).py(12.0)
                .child(col().w_full().max_w(1040.0).gap(4.0).growth(GrowthSpec::default())
                    .children(bubbles)
                    .children(msgs.is_empty().then(|| col().key("messages-empty").w_full().gap(6.0).py(24.0)
                        .child(text("Start the conversation").font(type_::TITLE).color(t.text))
                        .child(note("Send a message below to get things going.").wrap()))))
                .scroll(ScrollSpec { stick_bottom: true }).scroll_anchor().grow(1.0).min_h(0.0);
            #[cfg(feature = "image")]
            let list = list.drawn_rect_into(&s.preview_viewport);
            pane.child(row().key("conversation-header").w_full().gap(12.0).px(24.0).py(16.0).items_center().fill(t.card).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                    .child(chats.iter().find(|c| &c.id == id).map_or_else(|| avatar(title, 32.0), |c| chat_avatar(s, c, &session.user.id, 32.0)))
                    .child(text(title).font(rune::tok::TextStyle::new(18.0, 600.0)).color(t.text).truncate().shrink().min_w(0.0))
                    .child(text(if chats.iter().any(|c| &c.id == id && !c.channel) { "Chat" } else { "Channel" }).font(type_::TITLE).color(brand).px(12.0).py(8.0)
                        .border_sides([0.0, 0.0, 3.0, 0.0], brand)))
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
                .children(message_state::typing_label(s, id).map(|label| note(&label).font(type_::LABEL).px(28.0).py(6.0)))
                .child(col().w_full().items_center().px(28.0).py(20.0)
                    .child(col().w_full().max_w(1040.0).gap(6.0)
                    .children(s.reply_error.as_deref().map(|e| note(e).wrap().role("alert").padding(10.0).radius(6.0).fill(t.card).border_sides([0.0, 0.0, 0.0, 3.0], brand)))
                    .children(s.send_error.as_deref().map(|e| note(e).wrap().role("alert").padding(10.0).radius(6.0).fill(t.card).border_sides([0.0, 0.0, 0.0, 3.0], brand)))
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
                    .child(col().key("composer").w_full().radius(12.0).fill(t.card).border(1.0, t.separator)
                        .child(col().w_full().min_w(0.0).gap(8.0).px(16.0).pt(14.0).pb(10.0)
                            .children((!s.editor).then(|| crate::attachments::draft_files(s)).flatten())
                            .children((!s.pasted.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0)
                                .children(s.pasted.iter().enumerate().map(|(i, (_, image))| pasted_image(image, i)))))
                            .children((!s.editor).then(|| mention_suggestions(s)).flatten())
                            // One box shows the draft at a time; behind the editor it would echo every key.
                            .child(if s.editor { note("Writing in the editor…") } else {
                                auto_grow(draft_paste(text_input(&s.draft, type_::BODY).key("draft").label("Message. Enter to send, Shift+Enter for a new line.").color(t.text)
                                    .placeholder("Type a message", t.text_secondary)
                                    .caret(t.text, tok::accent(0, dark()).alpha(0.3))
                                    .on_edit(|s: &mut State| &mut s.draft)
                                    .on_input_change(mention_changed)
                                    .on_input_key(mention_key)
                                    .on_submit(|s: &mut State, _| if in_code_block(&s.draft.text[..s.draft.range().0]) { s.draft.insert("\n") } else { send(s) })), 1, 6)
                            })
                            .children(composer_preview(s)))
                        .child(row().key("composer-toolbar").w_full().flex_wrap().gap(4.0).items_center().px(8.0).pb(8.0)
                            .child(el().key("editor").role("button").label("Open message editor").focusable(true)
                                .cursor(rune::Cursor::Pointer).px(10.0).py(7.0).radius(6.0).hover(t.hover)
                                .on_press(|s: &mut State| s.editor = true)
                                .child(text("Format").font(type_::LABEL).color(t.text_secondary)))
                            .children(s.editing.is_none().then(|| el().key("attach-file").role("button").label("Attach file from this device").focusable(true)
                                .cursor(rune::Cursor::Pointer).px(10.0).py(7.0).radius(6.0).hover(t.hover)
                                .on_press_tx(crate::attachments::pick_file)
                                .child(row().gap(6.0).items_center()
                                    .child(rune::icon(&ATTACH, 18.0).color(t.text_secondary))
                                    .child(text("Attach file").font(type_::LABEL).color(t.text_secondary)))))
                            .child(el().grow(1.0))
                            .child(el().key("send").role("button").label(if s.sender.is_pending() { "Sending…" } else { "Send" }).focusable(true)
                                .disabled(!can_send).cursor(if can_send { rune::Cursor::Pointer } else { rune::Cursor::Default }).px(12.0).py(7.0).radius(7.0).fill(if can_send { brand } else { t.control_off })
                                .on_press(move |s: &mut State| if can_send { send(s); })
                                .child(row().gap(6.0).items_center()
                                    .child(text(if s.sender.is_pending() { "Sending…" } else { "Send" }).font(type_::LABEL).color(if can_send { t.bg } else { t.text_secondary }))
                                    .child(rune::icon(&SEND, 18.0).color(if can_send { t.bg } else { t.text_secondary }))))))))
        }
    };
    let body = row().w_full().grow(1.0).min_h(0.0).child(rail).child(sidebar).child(divider).child(pane);
    // Rune's modal scope includes later siblings, so keep the background outside search inert.
    let body = if query.is_empty() { body } else { body.inert() };
    col().w_full().h_full().fill(t.bg).child(search).child(body)
        .children(results)
}

pub(crate) fn search_key(s: &mut State, e: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if !matches!(s.screen, Screen::Main { .. }) || s.editor || s.viewer.is_some() || s.menu.is_some() || s.settings_panel {
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
    fn message_toolbar_ignores_empty_row_space_but_stays_on_its_buttons() {
        let mut s = init();
        message_states_demo(&mut s);
        let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
        settle(&mut rt);
        for (id, mine) in [("sample-incoming", false), ("sample-outgoing", true)] {
            let elements = rt.elements();
            let rect = |prefix| elements.iter().find(|e| e.key.as_deref() == Some(&format!("{prefix}/{id}"))).unwrap().rect;
            let (line, bubble) = (rect("msg"), rect("bubble"));
            let outside = (if mine { line[0] + 2.0 } else { line[0] + line[2] - 2.0 }, bubble[1] + bubble[3] / 2.0);
            let opacity = |rt: &rune::Runtime<State>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&format!("more/{id}"))).unwrap().opacity;
            rt.pointer_move(outside.0, outside.1);
            settle(&mut rt);
            assert!(opacity(&rt) < 0.01, "blank row space must not reveal actions");
            rt.pointer_move(bubble[0] + bubble[2] / 2.0, bubble[1] + bubble[3] / 2.0);
            settle(&mut rt);
            assert!(opacity(&rt) > 0.99);
            let (x, y) = rt.find(&format!("more/{id}")).unwrap();
            rt.pointer_move(x, y);
            settle(&mut rt);
            assert!(opacity(&rt) > 0.99, "moving onto the toolbar must keep it visible");
            rt.pointer_move(outside.0, outside.1);
            settle(&mut rt);
            assert!(opacity(&rt) < 0.01);
            rt.pointer_move(bubble[0] + bubble[2] / 2.0, bubble[1] + bubble[3] / 2.0);
            settle(&mut rt);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(&mut rt);
            assert_eq!(rt.state.menu.as_deref(), Some(id));
            rt.pointer_move(outside.0, outside.1);
            settle(&mut rt);
            assert!(opacity(&rt) > 0.99, "open menu must keep its toolbar visible");
            rt.update(|s, _| s.menu = None);
            settle(&mut rt);
        }
    }

    #[test]
    fn edited_label_follows_timestamp_even_in_grouped_messages() {
        for width in [640.0, 1200.0] {
            let mut s = init();
            message_states_demo(&mut s);
            let msgs = &mut s.messages.as_mut().unwrap().as_mut().unwrap().msgs;
            let mut grouped = msgs[1].clone();
            grouped.id = "grouped-edit".into();
            grouped.server_id = "102".into();
            grouped.time = "2026-11-17T10:25:00Z".into();
            assert!(same_message_group(&msgs[1], &grouped));
            msgs.push(grouped);
            let mut rt = rune::Runtime::new(s, view, (width, 800.0), rune::Env::new(true, 1.0));
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            let elements = rt.elements();
            for id in ["sample-incoming", "sample-outgoing", "grouped-edit"] {
                let time = elements.iter().find(|e| e.key.as_deref() == Some(&format!("time/{id}"))).unwrap();
                let bubble = elements.iter().find(|e| e.key.as_deref() == Some(&format!("bubble/{id}"))).unwrap();
                assert!(time.text.as_deref().unwrap().ends_with(" · Edited"));
                assert!(time.rect[1] + time.rect[3] <= bubble.rect[1]);
                assert!(time.rect[0] + time.rect[2] <= width);
            }
            assert!(!elements.iter().any(|e| e.text.as_deref() == Some("Edited")));
        }
    }

    #[test]
    fn delivery_icons_stay_beside_outgoing_bubble() {
        for seen in [false, true] {
            for width in [640.0, 1200.0] {
                let mut s = init();
                message_states_demo(&mut s);
                if !seen { s.message_state.seen.clear(); }
                let mut rt = rune::Runtime::new(s, view, (width, 800.0), rune::Env::new(true, 1.0));
                for _ in 0..60 { rt.frame(1.0 / 60.0); }
                let elements = rt.elements();
                let status = elements.iter().find(|e| e.key.as_deref() == Some("delivery/sample-outgoing")).unwrap();
                let bubble = elements.iter().find(|e| e.key.as_deref() == Some("bubble/sample-outgoing")).unwrap();
                assert!((status.rect[0] - bubble.rect[0] - bubble.rect[2] - 8.0).abs() < 1.0);
                assert!(status.rect[0] + status.rect[2] <= width);
                assert!((status.rect[1] + status.rect[3] - bubble.rect[1] - bubble.rect[3]).abs() < 1.0);
                assert_eq!(elements.iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("delivery/"))).count(), 1);
            }
        }
    }

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
    fn conversation_controls_fit_supported_sizes() {
        for dark in [false, true] {
            for (width, height) in [(640.0, 400.0), (1000.0, 680.0), (1280.0, 800.0), (1920.0, 1080.0)] {
                let mut s = init();
                message_states_demo(&mut s);
                let mut rt = rune::Runtime::new(s, view, (width, height), rune::Env::new(dark, 1.0));
                for _ in 0..60 { rt.frame(1.0 / 60.0); }
                let elements = rt.elements();
                let search = elements.iter().find(|e| e.key.as_deref() == Some("search-box")).unwrap();
                assert!((search.rect[0] + search.rect[2] / 2.0 - width / 2.0).abs() < 1.0,
                    "search must be centered at width {width}: {:?}", search.rect);
                for key in ["search", "sign-out", "conversation-header", "composer", "draft", "editor", "attach-file", "send"] {
                    let e = elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
                    let [x, y, w, h] = e.rect;
                    assert!(w > 0.0 && h > 0.0 && x >= 0.0 && y >= 0.0 && x + w <= width + 1.0 && y + h <= height + 1.0,
                        "{key} must fit {width}x{height}: {:?}", e.rect);
                }
                let header = elements.iter().find(|e| e.key.as_deref() == Some("conversation-header")).unwrap();
                let draft = elements.iter().find(|e| e.key.as_deref() == Some("draft")).unwrap();
                assert!(draft.rect[1] > header.rect[1] + header.rect[3]);
            }
        }
    }

    #[test]
    fn conversation_states_offer_guidance_and_retry_preserves_the_draft() {
        for dark in [false, true] {
            for scene in ["conversation-empty", "conversation-error", "conversation-loading", "messages-empty"] {
                let mut s = init();
                SCENES.iter().find(|(name, _)| *name == scene).unwrap().1(&mut s);
                s.draft.replace_text("Keep this draft");
                let mut rt = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(dark, 1.0));
                for _ in 0..60 { rt.frame(1.0 / 60.0); }
                let key = match scene {
                    "conversation-loading" => "message-loader",
                    "conversation-error" => "retry-messages",
                    other => other,
                };
                let elements = rt.elements();
                let e = elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
                let [x, y, w, h] = e.rect;
                assert!(w > 0.0 && h > 0.0 && x >= 0.0 && y >= 0.0 && x + w <= 641.0 && y + h <= 401.0, "{scene}: {:?}", e.rect);
                if scene == "conversation-error" {
                    let (x, y) = rt.find("retry-messages").unwrap();
                    rt.pointer_down(x, y);
                    rt.pointer_up(x, y);
                    rt.frame(0.0);
                    assert!(rt.state.messages.is_none());
                    assert_eq!(rt.state.selected.as_deref(), Some("a"));
                    assert_eq!(rt.state.draft.text, "Keep this draft");
                }
            }
        }
    }

    #[test]
    fn send_availability_and_clear_search_follow_input() {
        let mut s = init();
        demo(&mut s);
        let mut rt = rune::Runtime::new(s, view, (1000.0, 680.0), rune::Env::new(true, 1.0));
        for draft in ["", "   ", "Hello"] {
            rt.update(|s, tx| { s.draft.replace_text(draft); tx.focus("send", false); });
            rt.frame(0.0);
            let send = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("send")).unwrap();
            assert_eq!(send.focused, !draft.trim().is_empty());
        }
        rt.update(|s, _| s.search.replace_text("Charles"));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let (x, y) = rt.find("clear-search").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert!(rt.state.search.text.is_empty());
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("search") && e.focused));
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
