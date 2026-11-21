//! The signed-in workspace: search, conversation navigation and message history.

use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConversationSection {
    Chat,
    Teams,
}

const RAIL_WIDTH: f32 = 60.0;

static CHAT_ICON: rune::Icon = rune::Icon {
    name: "nav-chat", view: 24.0,
    body: r#"<path d="M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-6 4V6a2 2 0 0 1 2-2Z M7 9h10 M7 13h7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"#,
};

static TEAMS_ICON: rune::Icon = rune::Icon {
    name: "nav-teams", view: 24.0,
    body: r#"<circle cx="9" cy="7" r="3" fill="none" stroke="currentColor" stroke-width="1.5"/><circle cx="18" cy="8" r="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M3 21v-5a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v5 M16 13h2a3 3 0 0 1 3 3v4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
};

static CHANNEL_ICON: rune::Icon = rune::Icon {
    name: "nav-channel", view: 24.0,
    body: r#"<path d="m10 3-4 18 M18 3l-4 18 M4 9h17 M2 15h17" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#,
};

pub(super) static SEARCH_ICON: rune::Icon = rune::Icon {
    name: "nav-search", view: 24.0,
    body: r#"<circle cx="10.5" cy="10.5" r="6.5" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="m16 16 5 5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};

static SIDEBAR_ICON: rune::Icon = rune::Icon {
    name: "nav-sidebar", view: 24.0,
    body: r#"<rect x="3" y="4" width="18" height="16" rx="2" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M9 4v16" stroke="currentColor" stroke-width="1.7"/>"#,
};

static STAR_ICON: rune::Icon = rune::Icon { name: "favorite", view: 24.0, body: r#"<path d="m12 3 2.8 5.7 6.3.9-4.6 4.5 1.1 6.3-5.6-3-5.6 3 1.1-6.3-4.6-4.5 6.3-.9z" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>"# };
static STAR_FILLED: rune::Icon = rune::Icon {
    name: "favorited", view: 24.0,
    body: r#"<path d="m12 3 2.8 5.7 6.3.9-4.6 4.5 1.1 6.3-5.6-3-5.6 3 1.1-6.3-4.6-4.5 6.3-.9z" fill="currentColor"/>"#,
};

fn select_conversation(s: &mut State, id: &str) {
    message_search::clear_top(s);
    reveal_conversation(s, id);
    if s.selected.as_deref() != Some(id) {
        open_chat(s, id.to_owned());
    }
}

pub(crate) fn main_view(s: &State, session: &Session, chats: &[Chat], cx: &Cx<State>) -> El {
    let t = tok::theme(dark());
    let brand = rune::oklch(if dark() { 0.78 } else { 0.46 }, 0.135, 266.0);
    let selected = rune::oklch(if dark() { 0.34 } else { 0.94 }, 0.035, 266.0);
    let sidebar_surface = rune::oklch(if dark() { 0.255 } else { 0.975 }, 0.009, 266.0);
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary);
    let ask_me_open = s.ask_me.open;

    let show_teams = s.conversation_section == ConversationSection::Teams;
    let matches_filter = |chat: &Chat| chat.channel == show_teams && (!s.unread_only || chat.unread);
    let is_favorite = |id: &str| s.favorites.get(&session.user.id).is_some_and(|ids| ids.contains(id));
    let teams = s.teams.as_ref().and_then(|result| result.as_ref().ok());
    let team_channels: Vec<_> = teams.into_iter().flatten().flat_map(|team|
        team.channels.iter().map(move |channel| (channel, Some(team.title.as_str())))).collect();
    let mut known: std::collections::HashSet<&str> = team_channels.iter().map(|(chat, _)| chat.id.as_str()).collect();
    let other_channels: Vec<_> = s.pending_channels.iter().chain(chats.iter().filter(|chat| chat.channel))
        .filter(|chat| known.insert(chat.id.as_str())).collect();
    let visible_chats: Vec<_> = chats.iter().filter(|chat| !chat.channel && !known.contains(chat.id.as_str())).collect();
    let conversations: Vec<_> = visible_chats.iter().map(|&chat| (chat, None))
        .chain(team_channels.iter().copied()).chain(other_channels.iter().map(|&chat| (chat, None))).collect();
    let active = s.selected.as_ref().and_then(|id| conversations.iter().copied().find(|(chat, _)| &chat.id == id));
    let now = chrono::Local::now();
    let chat_row = |c: &Chat, key: &str, detail: Option<&str>, indent: f32, words: &[&str]| {
        let id = c.id.clone();
        let picked = s.selected.as_ref() == Some(&c.id);
        let title_font = rune::TextStyle::new(13.0, if c.unread { 700.0 } else { 500.0 });
        let caption = detail.unwrap_or(&c.preview);
        let highlight = |node: El, value: &str| {
            if words.is_empty() { return node; }
            node.spans(super::search::match_spans(message_search::matching_ranges(value, words)))
        };
        let label = match (detail, c.unread) {
            (Some(team), true) => format!("{}, {team}, unread messages", c.title),
            (Some(team), false) => format!("{}, {team}", c.title),
            (None, true) => format!("{}, unread messages", c.title),
            (None, false) => c.title.clone(),
        };
        row().key(format!("{key}/{}", c.id)).role("button").label(label).focusable(true)
            .w_full().min_h(44.0).gap(8.0).pl(8.0 + indent).pr(8.0).py(6.0).radius(8.0).items_center()
            .fill(if picked { selected } else { sidebar_surface }).hover(t.hover).cursor(rune::Cursor::Pointer)
            .on_press(move |s: &mut State| select_conversation(s, &id))
            .child(el().w(3.0).h(16.0).radius(2.0).fill(if picked || c.unread { brand } else { sidebar_surface }))
            .child(if c.channel {
                el().size(30.0, 30.0).center().radius(8.0).fill(t.bg)
                    .child(rune::icon(&CHANNEL_ICON, 16.0).color(t.text_secondary))
            } else { chat_avatar(s, c, &session.user.id, 30.0) })
            .child(col().gap(2.0).grow(1.0).min_w(0.0)
                .child(row().w_full().gap(6.0).items_center()
                    .child(highlight(text(c.title.as_str()).font(title_font).color(t.text).truncate()
                        .help_when_truncated(c.title.as_str()).grow(1.0).min_w(0.0), &c.title))
                    .children((!c.time.is_empty()).then(|| note(&chat_time(&c.time, now)).font(type_::LABEL))))
                .children((!caption.is_empty()).then(|| highlight(note(caption).font(type_::LABEL).truncate(), caption))))
    };

    let section = |label: &str| note(label).font(rune::TextStyle::new(12.0, 650.0)).px(12.0).pt(12.0).pb(4.0);
    let favorites: Vec<_> = conversations.iter().filter(|(c, _)| matches_filter(c) && is_favorite(&c.id)).collect();
    let mut rows = Vec::new();
    if !favorites.is_empty() {
        rows.push(section("Favorites"));
        rows.extend(favorites.into_iter().map(|(c, team)| chat_row(c, "favorite", *team, 0.0, &[])));
    }

    let mut has_matches = conversations.iter().any(|(c, _)| matches_filter(c));
    if !show_teams {
        let chats = visible_chats.iter().filter(|c| matches_filter(c) && !is_favorite(&c.id));
        let chat_rows: Vec<_> = chats.map(|c| chat_row(c, "chat", None, 0.0, &[])).collect();
        if !chat_rows.is_empty() {
            rows.push(section("Chats"));
            rows.extend(chat_rows);
        }
    } else {
        let mut channel_rows = Vec::new();
        for team in teams.into_iter().flatten() {
            let channels: Vec<_> = team.channels.iter().filter(|c| matches_filter(c) && !is_favorite(&c.id)).collect();
            if channels.is_empty() && (s.unread_only || !team.channels.is_empty()) { continue; }
            let id = team.id.clone();
            let collapsed = s.collapsed_teams.contains(&id);
            let unread = channels.iter().any(|c| c.unread);
            has_matches = true;
            channel_rows.push(row().key(format!("team/{id}")).role("button").label(if unread {
                    format!("{}, unread channels", team.title)
                } else { team.title.clone() }).expanded(!collapsed)
                .focusable(true).w_full().gap(8.0).px(10.0).py(7.0).items_center().radius(8.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut State| {
                    if !s.collapsed_teams.remove(&id) { s.collapsed_teams.insert(id.clone()); }
                })
                .child(note(if collapsed { "›" } else { "⌄" }).w(12.0))
                .child(avatar(&team.title, 24.0))
                .child(text(team.title.as_str()).font(rune::TextStyle::new(13.0, if unread { 700.0 } else { 550.0 }))
                    .color(t.text).truncate().min_w(0.0)));
            if !collapsed {
                if channels.is_empty() { channel_rows.push(note("No channels").px(32.0).py(6.0)); }
                channel_rows.extend(channels.into_iter().map(|c| chat_row(c, "channel", None, 20.0, &[])));
            }
        }
        channel_rows.extend(other_channels.iter().filter(|c| matches_filter(c) && !is_favorite(&c.id))
            .map(|c| chat_row(c, "channel", None, 0.0, &[])));
        match &s.teams {
            None if s.teams_loader.is_pending() => {
                has_matches = true;
                channel_rows.push(note("Loading channels…").px(12.0).py(6.0));
            }
            Some(Err(e)) => {
                has_matches = true;
                channel_rows.push(note(e).wrap().px(12.0).py(6.0));
                channel_rows.push(button("retry-teams", "Reload channels", false, |s| {
                    s.teams = None;
                    load_teams(s);
                }));
            }
            _ => {}
        }
        if !channel_rows.is_empty() {
            rows.push(section("Teams and channels"));
            rows.extend(channel_rows);
        }
    }
    if !has_matches {
        rows.push(note(match (show_teams, s.unread_only) {
            (false, false) => "No chats yet.",
            (true, false) => "No channels yet.",
            (false, true) => "No unread chats.",
            (true, true) => "No unread channels.",
        }).wrap().padding(12.0));
    }

    let query = s.search.text.trim().to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    let hits: Vec<_> = if query.is_empty() { Vec::new() } else {
        conversations.iter().copied().filter(|(c, team)| {
            let title = c.title.to_lowercase();
            let team = team.unwrap_or_default().to_lowercase();
            words.iter().all(|word| title.contains(word) || team.contains(word))
        }).collect()
    };
    let message_hits = s.search_preview.results(&query);
    let preview_pending = s.search_preview.pending(&query);
    let first_suggestion = hits.first().map(|(chat, _)| format!("hit/{}", chat.id))
        .or_else(|| message_hits.first().map(super::search::preview_key));
    let results = (!hits.is_empty() || !message_hits.is_empty() || preview_pending).then(|| {
        col().key("results").w(540.0).max_h(420.0).shrink().min_h(0.0).padding(6.0).gap(2.0).radius(10.0).fill(sidebar_surface)
            .border(1.0, t.separator)
            .anchored_fit("search-box", Side::Bottom, Align::Start, 4.0)
            .child(col().w_full().gap(2.0).role("list").label("Search suggestions").focus_group()
                .children((!hits.is_empty()).then(|| section("Chats and channels").pt(4.0)))
                .children(hits.iter().map(|(c, team)| chat_row(c, "hit", team.or(Some(if c.channel { "Channel" } else { "Chat" })), 0.0, &words)))
                .children((!message_hits.is_empty() || preview_pending).then(|| section("Recent messages")))
                .children(message_hits.iter().map(|hit| super::search::preview_row(hit, &conversations)))
                .children((preview_pending && message_hits.is_empty()).then(|| {
                    note("Searching messages…").role("status").px(12.0).py(8.0)
                }))
                .scroll(ScrollSpec { stick_bottom: false }).shrink().min_h(0.0))
    });
    let scoped_search_open = s.message_search.open && s.message_search.scope.is_some();
    let global_search_open = s.message_search.open && s.message_search.scope.is_none();
    let search_width = if scoped_search_open { super::search::PANEL_WIDTH } else { 0.0 };
    let available_sidebar_width = cx.layout_with("workspace", move |rect| {
        (rect[2] - RAIL_WIDTH - 350.0 - search_width).clamp(0.0, PANEL_MAX)
    }).unwrap_or(230.0);
    let compact_search = cx.layout_with("workspace", |rect| rect[2] < 700.0).unwrap_or(false);
    let search_occupies_sidebar = scoped_search_open && available_sidebar_width < 172.0;
    let show_sidebar = !s.sidebar_hidden && !search_occupies_sidebar;
    let show_conversations = !show_sidebar || s.panel_width == 0.0;
    let toggle_sidebar = if show_conversations { "Show conversations" } else { "Hide conversations" };
    let search = row().w_full().gap(12.0).items_center().px(16.0).py(8.0).fill(t.card)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(row().w(164.0).min_w(164.0).grow(1.0).gap(8.0).items_center()
            .children((!ask_me_open).then(|| el().key("toggle-sidebar").size(36.0, 36.0).center().radius(7.0)
                .role("button").label(toggle_sidebar).help(toggle_sidebar).focusable(true)
                .hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut State| {
                    if search_occupies_sidebar { s.message_search.close(); }
                    if s.panel_width == 0.0 { s.panel_width = settings::DEFAULT_PANEL_WIDTH; }
                    s.sidebar_hidden = !show_conversations;
                })
                .child(rune::icon(&SIDEBAR_ICON, 20.0).color(t.text_secondary))))
            .child(text("Teams Agent").font(rune::TextStyle::new(16.0, 700.0)).color(t.text)))
        .child(row().key("search-box").items_center().gap(8.0).w_full().max_w(540.0).min_w(0.0).shrink().min_h(36.0)
            .px(10.0).py(5.0).radius(8.0).fill(t.bg).border(1.0, t.separator)
            .child(rune::icon(&SEARCH_ICON, 17.0).color(t.text_secondary))
            .child(text_input(&s.search, type_::BODY).key("search").grow(1.0).min_w(0.0)
                .label("Search messages, chats, and channels").color(t.text)
                .placeholder(if ask_me_open { "Search Teams" } else { "Search messages, chats, and channels" }, t.text_secondary)
                .caret(t.text, brand.alpha(0.3))
                .on_edit(|s: &mut State| &mut s.search)
                .on_input_change(message_search::preview_changed)
                .on_submit(message_search::open_global)
                .on_input_key(move |s: &mut State, e: &rune::KeyEvent, tx| {
                    match e.key {
                        rune::Key::Escape => {
                            message_search::clear_top(s);
                            tx.blur();
                            true
                        }
                        rune::Key::Down if e.mods == rune::Mods::default() && let Some(key) = &first_suggestion => {
                            tx.focus(key.as_str(), true);
                            true
                        }
                        _ => false,
                    }
                }))
            .children((!query.is_empty()).then(|| el().key("clear-search").role("button").label("Clear search").focusable(true)
                .px(6.0).py(4.0).radius(4.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press_tx(|s: &mut State, tx| { message_search::clear_top(s); tx.focus("search", false); })
                .child(note("Clear").font(type_::LABEL)))))
        .child(row().w(164.0).min_w(164.0).grow(1.0).gap(6.0).items_center().justify_end()
            .child(releases::nav_button(s))
            .child(el().key("sign-out").role("button").label("Sign out").focusable(true)
                .px(6.0).py(8.0).radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer).on_press(sign_out)
                .child(note("Sign out").font(type_::LABEL)))
            .child(user_avatar(s, &session.user.id, &session.user.name, 30.0)));
    let search = col().key("search-area").w_full().child(search);
    let search = if query.is_empty() { search } else {
        search.modal_tx(true, |s: &mut State, tx| { message_search::clear_top(s); tx.blur(); })
    };

    let sidebar_width = s.panel_width.min(available_sidebar_width);
    let list = with_bar("chats", col().w_full().gap(2.0).px(6.0).pb(12.0).children(rows)
        .scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0), &s.chat_bar);
    let sidebar = col().key("sidebar").w(sidebar_width).min_w(0.0).h_full().min_h(0.0)
        .motion(rune::Spring::NONE).clip_rounded(0.0).fill(sidebar_surface)
        .children(s.live_status.as_deref().map(|status| note(status).wrap().padding(12.0).role("status")))
        .child(text(if show_teams { "Teams" } else { "Chat" }).font(rune::TextStyle::new(20.0, 700.0)).color(t.text).truncate().px(14.0).pt(14.0).pb(8.0))
        .child(row().key("conversation-filters").role("radiogroup").label("Filter conversations").w_full().flex_wrap().gap(4.0).px(10.0).pb(4.0)
            .children([
                (true, "Unread", "filter-unread"),
                if show_teams { (false, "Channels", "filter-channels") } else { (false, "Chats", "filter-chats") },
            ].into_iter().map(|(unread, label, key)| {
                let on = s.unread_only == unread;
                el().key(key).role("radio").label(label).checked(on).focusable(true)
                    .min_h(32.0).px(8.0).center().radius(6.0)
                    .fill(if on { selected } else { sidebar_surface }).hover(t.hover).cursor(rune::Cursor::Pointer)
                    .on_press_tx(move |s: &mut State, tx| {
                        if s.unread_only != unread {
                            s.unread_only = unread;
                            tx.scroll_to_instant("chats", 0.0, 0.0);
                        }
                    })
                    .child(text(label).font(rune::TextStyle::new(12.0, if on { 650.0 } else { 500.0 }))
                        .color(if on { brand } else { t.text_secondary }))
            })))
        .child(list);
    let divider = col().key("panel-resize").w(8.0).h_full().center().fill(sidebar_surface)
        .motion(rune::Spring::NONE).cursor(rune::Cursor::ResizeColumn)
        .label("Resize conversations panel with Left and Right arrows").focusable(true)
        .on_key(move |s: &mut State, e: &rune::KeyEvent, _| {
            let delta = match e.key {
                rune::Key::Left => -20.0,
                rune::Key::Right => 20.0,
                _ => return false,
            };
            let width = panel_width(sidebar_width, delta).min(available_sidebar_width);
            if width != sidebar_width { s.panel_width = width; }
            settings::save(s);
            true
        })
        .on_drag(move |s: &mut State, d: &mut Drag, _| {
            if d.phase == Phase::Down { d.data = s.panel_width; }
            let start = d.data.min(available_sidebar_width);
            let width = panel_width(start, d.dx).min(available_sidebar_width);
            s.panel_width = if d.cancelled() || width == start { d.data } else { width };
            if d.phase == Phase::Up && !d.cancelled() { settings::save(s); }
        })
        .child(el().w(1.0).h_full().fill(t.separator));

    let is_channel = active.is_some_and(|(chat, _)| chat.channel);
    let header = s.selected.as_ref().map(|id| {
        let favorite_id = id.clone();
        let favorite = is_favorite(id);
        let favorite_label = if favorite { "Remove from Favorites" } else { "Add to Favorites" };
        let (title, context, icon) = match active {
            Some((chat, team)) => (chat.title.as_str(), team.unwrap_or(if chat.channel { "Channel" } else { "Chat" }),
                if chat.channel {
                    el().size(38.0, 38.0).radius(9.0).center().fill(selected)
                        .child(rune::icon(&CHANNEL_ICON, 20.0).color(brand))
                } else { chat_avatar(s, chat, &session.user.id, 38.0) }),
            None => ("Conversation", "", avatar("Conversation", 38.0)),
        };
        let identity = col().min_w(0.0).gap(2.0);
        let identity = if is_channel { identity.w(168.0).shrink() } else { identity.grow(1.0) };
        let header = row().key("conversation-header").w_full().gap(10.0).px(18.0).py(9.0).items_center().fill(t.card)
            .border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
            .child(icon)
            .child(identity
                .child(text(title).key("conversation-title").font(rune::TextStyle::new(16.0, 650.0)).color(t.text).truncate().help_when_truncated(title))
                .children((!context.is_empty()).then(|| note(context).font(type_::LABEL).truncate())))
            .children(is_channel.then(|| col().key("channel-posts-heading").gap(3.0).px(8.0).pt(3.0)
                .child(text("Posts").role("heading").font(type_::TITLE).color(brand))
                .child(el().w_full().h(2.0).radius(1.0).fill(brand))))
            .children(is_channel.then(|| el().grow(1.0).min_w(0.0)))
            .children(active.is_some().then(|| el().key("favorite-conversation").size(38.0, 38.0).center().radius(7.0)
                .role("button").label(favorite_label).help(favorite_label).focusable(true)
                .fill(if favorite { selected } else { t.card }).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut State| settings::toggle_favorite(s, &favorite_id))
                .child(rune::icon(if favorite { &STAR_FILLED } else { &STAR_ICON }, 20.0)
                    .color(if favorite { brand } else { t.text_secondary }))))
            .child(el().key("find-messages").size(38.0, 38.0).center().radius(7.0)
                .role("button").label(if is_channel { "Find in channel" } else { "Find in chat" })
                .help(if cfg!(target_os = "macos") { "Find messages (⌘F)" } else { "Find messages (Ctrl+F)" })
                .focusable(true).hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press_tx(|s: &mut State, tx| { message_search::open_scoped(s, tx); })
                .child(rune::icon(&SEARCH_ICON, 19.0).color(t.text_secondary)));
        #[cfg(feature = "call-audio")]
        let header = header.children(active.filter(|(chat, _)| !chat.channel)
            .and_then(|(chat, _)| super::call::start_button(s, session, chat)));
        header
    });

    let rail = col().key("navigation").role("navigation").label("Workspace").w(RAIL_WIDTH).h_full()
        .gap(6.0).px(4.0).py(8.0).fill(t.card).border_sides([0.0, 1.0, 0.0, 0.0], t.separator)
        .children([
            (None, "Ask Me", "nav-ask-me", &super::ask_me::ASK_ME_ICON),
            (Some(ConversationSection::Chat), "Chat", "nav-chat", &CHAT_ICON),
            (Some(ConversationSection::Teams), "Teams", "nav-teams", &TEAMS_ICON),
        ].into_iter().map(|(section, label, key, icon)| {
            let on = section.map_or(ask_me_open, |section| !ask_me_open && s.conversation_section == section);
            let color = if on { brand } else { t.text_secondary };
            col().key(key).role("button").label(label).checked(on).focusable(true).w_full().min_h(52.0)
                .center().gap(4.0).radius(8.0).fill(if on { selected } else { t.card })
                .hover(t.hover).cursor(rune::Cursor::Pointer)
                .on_press_tx(move |s: &mut State, tx| {
                    let Some(section) = section else {
                        super::ask_me::open(s, tx);
                        return;
                    };
                    let leaving_ask_me = s.ask_me.leave();
                    if s.conversation_section != section {
                        s.conversation_section = section;
                        s.unread_only = false;
                        tx.scroll_to_instant("chats", 0.0, 0.0);
                    }
                    s.sidebar_hidden = false;
                    if s.panel_width == 0.0 { s.panel_width = settings::DEFAULT_PANEL_WIDTH; }
                    if leaving_ask_me { notifications::mark_selected_read(s); }
                })
                .child(rune::icon(icon, 21.0).color(color))
                .child(text(label).font(rune::TextStyle::new(11.0, 600.0)).color(color))
        }));
    let body = row().key("workspace").w_full().grow(1.0).min_h(0.0).child(rail);
    let body = if ask_me_open {
        let (width, maximum) = super::ask_me::sidebar_width(s, available_sidebar_width);
        let compact = cx.layout_with("ask-me", |rect| rect[2] < 500.0).unwrap_or(compact_search);
        let compact_source = cx.layout_with("workspace", move |rect| {
            rect[2] - RAIL_WIDTH - width < super::ask_me::SOURCE_AND_TRANSCRIPT_WIDTH
        }).unwrap_or(compact_search);
        let source = super::ask_me::source_panel(s, compact_source, brand, selected);
        body.child(super::ask_me::sidebar(s, width, sidebar_surface, brand, selected))
            .child(super::ask_me::divider(width, maximum, sidebar_surface))
            .children((!compact_source || source.is_none()).then(|| super::ask_me::view(s, brand, selected, compact)))
            .children(source)
    } else {
        body.children((show_sidebar && sidebar_width > 0.0).then_some(sidebar))
            .children(show_sidebar.then_some(divider))
            .children((!scoped_search_open || !compact_search).then(|| {
                col().grow(1.0).shrink().min_w(0.0).min_h(0.0).h_full().children(header)
                    .child(conversation_pane(s, session, is_channel, brand, selected))
            }))
            .children(scoped_search_open.then(|| {
                super::search::panel(s, &conversations, compact_search, brand)
            }))
    };
    // Rune's modal scope includes later siblings, so keep the background outside search inert.
    let body = if query.is_empty() && !global_search_open { body } else { body.inert() };
    let search = if global_search_open { search.inert() } else { search };
    let layout = col().w_full().h_full().fill(t.bg).child(search);
    #[cfg(feature = "call-audio")]
    let layout = layout.children(super::call::compact(s).map(|status|
        if query.is_empty() && !global_search_open { status } else { status.inert() }));
    layout.child(body)
        .children(results)
        .children(global_search_open.then(|| super::search::panel(s, &conversations, false, brand)))
}

struct ChannelPost {
    root: usize,
    replies: Vec<usize>,
}

/// Resolve reply chains against the loaded history. Missing parents and cycles keep their
/// messages visible as separate posts until a valid parent is available.
fn channel_posts(msgs: &[Msg]) -> Vec<ChannelPost> {
    let mut indices = std::collections::HashMap::with_capacity(msgs.len() * 2);
    for (i, msg) in msgs.iter().enumerate() {
        for id in [&msg.id, &msg.server_id].into_iter().filter(|id| !id.is_empty()) {
            indices.entry(id.as_str()).or_insert(i);
        }
    }
    let mut children = vec![Vec::new(); msgs.len()];
    let mut roots = vec![None; msgs.len()];
    let mut pending = Vec::new();
    for (i, msg) in msgs.iter().enumerate() {
        let parent_id = msg.thread_root.as_deref().or_else(|| msg.reply.as_ref().map(|reply| reply.id.as_str()));
        if let Some(&parent) = parent_id.and_then(|id| indices.get(id)) && parent != i
        {
            children[parent].push(i);
        } else {
            roots[i] = Some(i);
            pending.push(i);
        }
    }
    while let Some(i) = pending.pop() {
        for &child in &children[i] {
            roots[child] = roots[i];
            pending.push(child);
        }
    }
    let mut replies = vec![Vec::new(); msgs.len()];
    for (i, root) in roots.iter().enumerate() {
        if let Some(root) = *root && root != i {
            replies[root].push(i);
        }
    }
    replies.into_iter().enumerate().filter_map(|(i, replies)| {
        (roots[i].unwrap_or(i) == i).then_some(ChannelPost { root: i, replies })
    }).collect()
}

fn conversation_pane(s: &State, session: &Session, is_channel: bool, brand: rune::Color, selected: rune::Color) -> El {
    static ATTACH: rune::Icon = rune::Icon { name: "composer-attach", view: 24.0, body: r#"<path d="m8 12 6-6a3 3 0 0 1 4 4l-8 8a5 5 0 0 1-7-7l9-9 M6 14l8-8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static SEND: rune::Icon = rune::Icon { name: "composer-send", view: 24.0, body: r#"<path d="m3 3 18 9-18 9 3-9z M6 12h15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"# };
    let t = tok::theme(dark());
    let sent = rune::oklch(if dark() { 0.38 } else { 0.92 }, 0.055, 266.0);
    let pane_width = if is_channel { 760.0 } else { 1040.0 };
    let message_column = || col().w_full().max_w(pane_width);
    let row_gap = 8.0;
    let avatar_width = 32.0;
    let receipt_width = 20.0;
    let (content_left, content_right) = if is_channel { (0.0, 0.0) } else {
        (avatar_width + row_gap, receipt_width + row_gap)
    };
    let send_label = compose_action_label(s, is_channel);
    let can_send = !s.sender.is_pending() && (!s.draft.text.trim().is_empty() || !s.pasted.is_empty() || !s.attached.is_empty());
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary);
    let pane = col().grow(1.0).shrink().w_full().min_w(0.0).min_h(0.0).fill(t.bg);
    match (&s.selected, &s.messages) {
        (None, _) => pane.center().padding(24.0).child(col().key("conversation-empty").w_full().max_w(340.0).gap(10.0)
            .child(el().size(48.0, 48.0).radius(12.0).center().fill(selected)
                .child(rune::icon(&CHAT_ICON, 26.0).color(brand)))
            .child(text("Choose a conversation").font(rune::TextStyle::new(22.0, 650.0)).color(t.text).wrap())
            .child(note("Open a chat or channel from the sidebar, or search by name.").wrap())),
        (Some(_), None) => {
            pane.key("messages-loading").center().motion(rune::Spring::NONE)
                .gap(12.0).child(loading_messages()).child(note(if is_channel { "Loading posts…" } else { "Loading conversation…" }))
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
            let today = chrono::Local::now().date_naive();
            let render_message = |i: usize, thread_root: Option<&Msg>| {
                let m = &msgs[i];
                let head = !is_channel && i.checked_sub(1).is_none_or(|j| !same_message_group(&msgs[j], m));
                let show_quote = m.reply.as_ref().filter(|quote| {
                    thread_root.is_none_or(|root| quote.id != root.id && quote.id != root.server_id)
                });
                let nested = thread_root.is_some();
                let quote_target = show_quote.map(|quote| quote.id.as_str())
                    .or_else(|| if is_channel && !nested && !m.deleted && m.reply.is_none() { m.thread_root.as_deref() } else { None });
                let time = message_time(&m.time, today);
                let time = note(&format!("{time}{}", if m.edited { " · Edited" } else { "" }))
                    .key(format!("time/{}", m.id)).font(type_::LABEL).wrap().shrink().min_w(0.0);
                #[cfg(feature = "image")]
                let emojis = teams::animated_emojis(&m.raw);
                #[cfg(not(feature = "image"))]
                let emojis: Vec<(String, String, u16)> = vec![];
                let highlighted = s.reply_highlight_on && s.reply_highlight.as_ref().is_some_and(|(id, _)| id == &m.id);
                let bubble = col().key(format!("bubble/{}", m.id)).max_w(720.0).shrink().min_w(0.0);
                let bubble = if is_channel { bubble.w_full() } else {
                    bubble.px(12.0).py(10.0).radius(6.0)
                        .fill(if !emojis.is_empty() && m.images.is_empty() && m.files.is_empty() { t.bg } else if m.mine { sent } else { t.card })
                        .border(2.0, tok::accent(0, dark()).alpha(if highlighted { 1.0 } else { 0.0 }))
                };
                let bubble = bubble
                    .children(quote_target.map(|target| {
                        let target = target.to_owned();
                        // A set width: sized from its content, the quote measured its wrapped text at one width and drew it at another, spilling out of a short reply's bubble.
                        row().w(400.0).shrink().min_w(0.0).gap(10.0).padding(8.0).mb(6.0).radius(4.0).fill(t.bg)
                            .border(1.0, t.separator)
                            .cursor(rune::Cursor::Pointer).hover(t.hover)
                            .label(if show_quote.is_some() { "Go to quoted message" } else { "Go to original post" }).focusable(true)
                            .on_press_tx(move |s: &mut State, tx| {
                                s.reply_highlight = None;
                                s.reply_error = None;
                                s.search_jump = false;
                                s.reply_target = Some(target.clone());
                                reveal_reply(s, tx);
                            })
                            .child(el().w(2.0).radius(1.0).fill(t.text_secondary))
                            .child(if let Some(reply) = show_quote {
                                col().gap(2.0).grow(1.0).shrink().min_w(0.0)
                                    .child(note(&format!("{}  {}", if reply.sender_id == session.user.id { &session.user.name } else { &reply.sender }, local_time(&reply.time, "%Y-%m-%d %I:%M %p"))).font(type_::LABEL).truncate())
                                    .child(text(reply.text.as_str()).font(type_::BODY).color(t.text).max_lines(2))
                            } else {
                                text("Go to original post").font(type_::LABEL).color(brand).wrap().shrink().min_w(0.0)
                            })
                    }))
                    .children((!m.text.is_empty() && emojis.is_empty()).then(|| message_text(m)))
                    .children((!m.images.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0).mt(if m.text.is_empty() { 0.0 } else { 8.0 })
                        .children(m.images.iter().map(|url| message_image(s, url)))))
                    .children((!m.files.is_empty()).then(|| col().gap(8.0).mt(if m.text.is_empty() && m.images.is_empty() { 0.0 } else { 8.0 })
                        .children(m.files.iter().map(|f| file_card(s, f)))));
                #[cfg(feature = "image")]
                let bubble = bubble.children((!emojis.is_empty()).then(|| row().flex_wrap().gap(4.0)
                    .children(emojis.iter().enumerate().map(|(i, (url, alt, size))| message_emoji(s, url, alt, *size).key(format!("emoji/{}/{i}", m.id))))));
                // Stable keys keep a moving history window from morphing one message into another.
                let line = row().key(format!("msg/{}", m.id)).arrives_whole()
                    .w_full().gap(row_gap).pt(if head { 12.0 } else { 0.0 });
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
                let hover_group = format!("msg/{}", m.id);
                let hover_surface = |surface: El| {
                    let id = m.id.clone();
                    surface.hover_group_shared(hover_group.clone())
                        .on_hover_change(move |s: &mut State, on, _| {
                            if on { s.hovered = Some(id.clone()) } else if s.hovered.as_ref() == Some(&id) { s.hovered = None }
                        })
                };
                // The toolbar shares the message's hover so the pointer can reach its actions.
                let shown = s.menu.as_ref() == Some(&m.id) || s.hovered_bar.as_ref().or(s.hovered.as_ref()) == Some(&m.id);
                let bar_id = m.id.clone();
                let actions = message_actions(m, shown, !is_channel)
                    .map(|a| el().hover_group_shared(hover_group.clone())
                        .pin(None, Some(if is_channel { 8.0 } else { -30.0 }), Some(if is_channel { 10.0 } else { 0.0 }), None)
                        .on_hover_change(move |s: &mut State, on, _| {
                            if on { s.hovered_bar = Some(bar_id.clone()) } else if s.hovered_bar.as_ref() == Some(&bar_id) { s.hovered_bar = None }
                        })
                        .child(a));
                let delivery = message_state::delivery(s, m);
                let body = bubble.children(message_reactions(s, m));
                let status = (delivery == Some("Sending…")).then(|| note(if is_channel { "Posting…" } else { "Sending…" }).font(type_::LABEL));
                let undo = (m.deleted && m.mine).then(|| {
                    let m = m.clone();
                    el().key(format!("undo/{}", m.id)).role("button").label("Undo delete").focusable(true)
                        .cursor(rune::Cursor::Pointer).px(8.0).py(4.0).radius(4.0).hover(t.hover)
                        .on_press(move |s: &mut State| message_state::undo(s, &m))
                        .child(note(if s.message_state.restorer.is_pending() { "Restoring…" } else { "Undo" }).font(type_::LABEL))
                });
                let line = if is_channel {
                    let (author, author_id) = if m.mine { (&session.user.name, &session.user.id) } else { (&m.sender, &m.sender_id) };
                    let has_actions = actions.is_some();
                    let content = col().w_full().gap(if nested { 6.0 } else { 10.0 })
                        .padding(if nested { 10.0 } else { 14.0 }).radius(8.0)
                        .border(1.0, brand.alpha(if highlighted { 1.0 } else { 0.0 }))
                        .child(row().w_full().gap(10.0).items_center()
                            .pr(if has_actions { 68.0 } else { 0.0 })
                            .child(user_avatar(s, author_id, author, if nested { 26.0 } else { 32.0 }))
                            .child(col().grow(1.0).min_w(0.0).gap(2.0)
                                .child(text(author.as_str()).font(type_::TITLE).color(t.text).truncate().help_when_truncated(author.as_str()))
                                .child(time)))
                        .child(body)
                        .children(status)
                        .children(undo)
                        .children(nested.then(|| channel_quote_button(m, brand, false)).flatten())
                        .children(actions);
                    line.child(hover_surface(content))
                } else {
                    let body = hover_surface(body).children(actions)
                        .children(delivery.filter(|label| *label != "Sending…").map(|label|
                            el().pin(None, None, Some(-content_right), Some(0.0)).child(message_delivery(m, label))));
                    let content = col().gap(4.0).shrink().min_w(0.0);
                    let content = if m.mine { content.items_end() } else { content }
                        .children((head || m.edited).then(|| row().gap(8.0).items_center().shrink().min_w(0.0)
                            .children((head && !m.mine).then(|| text(m.sender.as_str()).font(type_::LABEL).color(t.text).truncate().shrink().min_w(0.0)))
                            .child(time)))
                        .child(body)
                        .children(status)
                        .children(undo);
                    if m.mine {
                        line.pl(content_left).justify_end().items_end().child(content).child(el().w(receipt_width))
                    } else {
                        line.pr(content_right)
                            .child(if head { user_avatar(s, &m.sender_id, &m.sender, avatar_width) } else { el().w(avatar_width) })
                            .child(content)
                    }
                };
                line
            };
            let bubbles: Vec<El> = if is_channel {
                channel_posts(msgs).into_iter().map(|post| {
                    let root = &msgs[post.root];
                    col().key(format!("post/{}", root.id)).role("article").w_full().radius(10.0)
                        .fill(t.card).border(1.0, t.separator)
                        .child(render_message(post.root, None))
                        .children((!post.replies.is_empty()).then(|| col().key(format!("post-replies/{}", root.id))
                            .role("group").label("Replies").w_full().px(14.0).pb(8.0)
                            .child(col().w_full().gap(2.0).pl(8.0)
                                .border_sides([0.0, 0.0, 0.0, 2.0], t.separator)
                                .children(post.replies.iter().map(|&i| render_message(i, Some(root)))))))
                        .children(channel_quote_button(root, brand, true))
                }).collect()
            } else {
                msgs.iter().enumerate().flat_map(|(i, m)| {
                    let new_day = i.checked_sub(1).is_none_or(|j| day(&msgs[j].time) != day(&m.time));
                    let divider = new_day.then(|| row().key(format!("day/{}", m.id)).arrives_whole().w_full().gap(12.0).items_center()
                        .pl(content_left).pr(content_right).pt(16.0).pb(8.0)
                        .child(el().h(1.0).grow(1.0).fill(t.separator))
                        .child(note(&message_date(&m.time, today)).font(type_::LABEL))
                        .child(el().h(1.0).grow(1.0).fill(t.separator)));
                    divider.into_iter().chain([render_message(i, None)])
                }).collect()
            };
            let list = col().w_full().items_center().px(22.0).py(12.0)
                .child(message_column().gap(if is_channel { 10.0 } else { 4.0 }).growth(GrowthSpec::default())
                    .children(bubbles)
                    .children(msgs.is_empty().then(|| col().key("messages-empty").w_full().gap(6.0).py(24.0)
                        .child(text(if is_channel { "No posts yet" } else { "Start the conversation" }).font(type_::TITLE).color(t.text))
                        .child(note(if is_channel { "Write the first post below." } else { "Send a message below to get things going." }).wrap()))))
                .scroll(ScrollSpec { stick_bottom: true }).scroll_anchor().grow(1.0).min_h(0.0);
            #[cfg(feature = "image")]
            let list = list.drawn_rect_into(&s.preview_viewport);
            pane
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
                .children(message_state::typing_label(s, id).map(|label| note(&label).font(type_::LABEL).px(22.0).py(6.0)))
                .child(col().w_full().items_center().px(22.0).py(14.0)
                        .child(message_column().pl(content_left).pr(content_right).gap(6.0)
                    .children(s.reply_error.as_deref().map(|e| note(e).wrap().role("alert").padding(10.0).radius(6.0).fill(t.card).border_sides([0.0, 0.0, 0.0, 3.0], brand)))
                    .children((s.search_jump && s.reply_target.is_some()).then(|| {
                        note("Locating message in history…").role("status").wrap().padding(10.0)
                    }))
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
                                auto_grow(draft_paste(text_input(&s.draft, type_::BODY).key("draft").label(if is_channel { "Channel post. Enter to submit, Shift+Enter for a new line." } else { "Message. Enter to send, Shift+Enter for a new line." }).color(t.text)
                                    .placeholder(compose_placeholder(s, is_channel), t.text_secondary)
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
                            .children((!is_channel && s.editing.is_none()).then(|| el().key("attach-file").role("button").label("Attach file from this device").focusable(true)
                                .cursor(rune::Cursor::Pointer).px(10.0).py(7.0).radius(6.0).hover(t.hover)
                                .on_press_tx(crate::attachments::pick_file)
                                .child(row().gap(6.0).items_center()
                                    .child(rune::icon(&ATTACH, 18.0).color(t.text_secondary))
                                    .child(text("Attach file").font(type_::LABEL).color(t.text_secondary)))))
                            .child(el().grow(1.0))
                            .child(el().key("send").role("button").label(send_label).focusable(true)
                                .disabled(!can_send).cursor(if can_send { rune::Cursor::Pointer } else { rune::Cursor::Default }).px(12.0).py(7.0).radius(7.0).fill(if can_send { brand } else { t.control_off })
                                .on_press(move |s: &mut State| if can_send { send(s); })
                                .child(row().gap(6.0).items_center()
                                    .child(text(send_label).font(type_::LABEL).color(if can_send { t.bg } else { t.text_secondary }))
                                    .child(rune::icon(&SEND, 18.0).color(if can_send { t.bg } else { t.text_secondary }))))))))
        }
    }
}

pub(crate) fn search_key(s: &mut State, e: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if !matches!(s.screen, Screen::Main { .. }) || s.editor || s.viewer.is_some() || s.menu.is_some() || s.settings_panel {
        return false;
    }
    if s.message_search.open && s.message_search.scope.is_none() { return false; }
    if rune::Keybind::new("mod+f").matches_tx(e, tx) {
        return message_search::open_scoped(s, tx);
    }
    if e.key == rune::Key::Escape && s.message_search.open && s.search.text.is_empty() {
        super::search::close(s, tx);
        return true;
    }
    if e.key == rune::Key::Escape && s.search.text.is_empty() && super::ask_me::close_source(s, tx) {
        return true;
    }
    if !rune::Keybind::new("ctrl+e").matches_tx(e, tx)
        && !rune::Keybind::new("ctrl+alt+e").matches_tx(e, tx) { return false }
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
    fn top_search_previews_messages_while_typing_and_opens_a_selection() {
        let mut s = init();
        demo(&mut s);
        if let Screen::Main { chats, .. } = &mut s.screen {
            chats[1].title = "Gears design".into();
        }
        let mut rt = rune::Runtime::new(s, view, (1000.0, 680.0), rune::Env::new(true, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
        let type_text = |rt: &mut rune::Runtime<State>, value: &str| {
            for ch in value.chars() {
                rt.key(rune::KeyEvent { key: rune::Key::Text(ch.to_string()), mods: Default::default() });
                rt.frame(0.0);
            }
        };
        settle(&mut rt);
        rt.update(|_, tx| tx.focus("search", false));
        settle(&mut rt);
        let preview = "message-preview/a/2026-11-14T10:31:00Z";
        type_text(&mut rt, "column");
        settle(&mut rt);
        assert!(rt.find(preview).is_some());
        assert!(!rt.elements().iter().any(|e| matches!(
            e.text.as_deref(), Some("Chats and channels" | "No matching chats or channels.")
        )));
        rt.update(|s, _| message_search::clear_top(s));
        settle(&mut rt);
        type_text(&mut rt, "gears");
        settle(&mut rt);
        let (_, chat_y) = rt.find("hit/b").unwrap();
        let (_, preview_y) = rt.find(preview).unwrap();
        let heading = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Recent messages")).unwrap();
        assert!(chat_y < heading.rect[1] && heading.rect[1] < preview_y);
        assert!(!rt.state.message_search.open);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("search") && e.focused));

        type_text(&mut rt, "zz");
        settle(&mut rt);
        assert!(rt.find("results").is_none());
        for _ in 0..2 {
            rt.key(rune::KeyEvent { key: rune::Key::Backspace, mods: Default::default() });
            rt.frame(0.0);
        }
        settle(&mut rt);
        rt.key(rune::KeyEvent { key: rune::Key::Down, mods: Default::default() });
        settle(&mut rt);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("hit/b") && e.focused));
        rt.key(rune::KeyEvent { key: rune::Key::Down, mods: Default::default() });
        settle(&mut rt);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some(preview) && e.focused));
        rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
        settle(&mut rt);
        assert!(rt.state.search.text.is_empty());
        assert!(rt.state.search_preview.results("gears").is_empty());
        assert_eq!(rt.state.reply_highlight.as_ref().unwrap().0, "2026-11-14T10:31:00Z");
    }

    #[test]
    fn message_search_opens_globally_and_in_the_current_conversation() {
        for (width, height) in [(640.0, 400.0), (1200.0, 680.0)] {
            let mut s = init();
            demo(&mut s);
            let other_chat_history = s.messages.as_ref().unwrap().as_ref().unwrap().clone();
            s.history.insert("b".into(), other_chat_history);
            let mut rt = rune::Runtime::new(s, view, (width, height), rune::Env::new(true, 1.0));
            rt.on_key = Some(search_key);
            let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
            let enter = rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() };
            settle(&mut rt);
            rt.update(|s, tx| {
                s.search.replace_text("column gears");
                tx.focus("search", false);
            });
            settle(&mut rt);
            rt.key(enter.clone());
            settle(&mut rt);
            assert!(rt.state.message_search.scope.is_none());
            let elements = rt.elements();
            let dialog = elements.iter().find(|e| e.key.as_deref() == Some("message-search-dialog")).unwrap();
            assert_eq!(dialog.role, "dialog");
            let [x, y, w, h] = dialog.rect;
            assert!(x > 0.0 && y > 0.0 && x + w < width && y + h < height);
            assert!(elements.iter().any(|e| e.key.as_deref() == Some("message-search-input") && e.focused));
            assert!(rt.find("message-search-panel").is_none());
            assert!(rt.find("message-hit/b/2026-11-14T10:31:00Z").is_some());
            let key = "message-hit/a/2026-11-14T10:31:00Z";
            let (x, y) = rt.find(key).expect("keyword search should find the matching message");
            let picked = rt.element_at(x, y).map(|element| element.path().to_string());
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(&mut rt);
            assert_eq!(rt.state.reply_highlight.as_ref().map(|highlight| highlight.0.as_str()), Some("2026-11-14T10:31:00Z"),
                "width={width}, hit at ({x}, {y}), pointer picked {picked:?}");
            assert!(rt.find("message-search-dialog").is_none());

            rt.key(rune::KeyEvent { key: rune::Key::Command('f'), mods: rune::Mods {
                ctrl: !cfg!(target_os = "macos"), logo: cfg!(target_os = "macos"), cmd: true, ..Default::default()
            } });
            settle(&mut rt);
            assert_eq!(rt.state.message_search.scope.as_deref(), Some("a"));
            assert!(rt.find("message-search-panel").is_some());
            assert!(rt.find("message-search-dialog").is_none());
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("message-search-input") && e.focused));
            for ch in "BERNOULLI".chars() {
                rt.key(rune::KeyEvent { key: rune::Key::Text(ch.to_string()), mods: Default::default() });
                rt.frame(0.0);
            }
            settle(&mut rt);
            let scoped_hit = "message-hit/a/2026-11-14T10:24:00Z";
            assert!(rt.find(scoped_hit).is_some());
            assert!(rt.find("message-hit/b/2026-11-14T10:24:00Z").is_none());
            assert!(rt.find(key).is_none());
            for _ in "BERNOULLI".chars() {
                rt.key(rune::KeyEvent { key: rune::Key::Backspace, mods: Default::default() });
                rt.frame(0.0);
            }
            settle(&mut rt);
            assert!(rt.find(scoped_hit).is_none());
            assert!(rt.find("message-search-status").is_none());
            rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() });
            settle(&mut rt);
            assert!(!rt.state.message_search.open, "Escape should close search at width {width}");
            assert!(rt.find("find-messages").is_some());
        }
    }

    #[test]
    fn channel_search_results_open_nested_replies_in_context() {
        let mut s = init();
        channel_posts_demo(&mut s);
        history::remember_selected(&mut s);
        demo(&mut s);
        s.conversation_section = ConversationSection::Chat;
        s.search.replace_text("review");
        message_search::open_global(&mut s, &mut Tx::default());
        let mut rt = rune::Runtime::new(s, view, (1200.0, 680.0), rune::Env::new(false, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
        let click = |rt: &mut rune::Runtime<State>, key| {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(rt);
        };
        settle(&mut rt);
        click(&mut rt, "message-filter-chats");
        assert!(rt.find("message-hit/backend/102").is_none());
        click(&mut rt, "message-filter-channels");
        click(&mut rt, "message-hit/backend/102");
        assert_eq!(rt.state.selected.as_deref(), Some("backend"));
        assert_eq!(rt.state.conversation_section, ConversationSection::Teams);
        assert_eq!(rt.state.reply_highlight.as_ref().unwrap().0, "channel-review");
        assert!(rt.find("post-replies/channel-update").is_some());
        assert!(rt.find("msg/channel-review").is_some());
    }

    #[test]
    #[cfg(feature = "call-audio")]
    fn audio_call_control_is_available_for_people_and_hidden_for_bots() {
        let mut s = init();
        message_states_demo(&mut s);
        let me = "10000000-0000-4000-8000-000000000001";
        let peer = "10000000-0000-4000-8000-000000000002";
        let direct = format!("19:{me}_{peer}@unq.gbl.spaces");
        let Screen::Main { session, chats } = &mut s.screen else { unreachable!() };
        Arc::make_mut(session).user.id = me.into();
        chats[0].id = direct.clone();
        s.selected = Some(direct);
        let mut rt = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(false, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let button = rt.elements().into_iter().find(|el| el.key.as_deref() == Some("audio-call")).unwrap();
        assert_eq!(button.rect[2..], [44.0, 44.0]);
        assert!(button.rect[0] + button.rect[2] <= 640.0);

        let Screen::Main { chats, .. } = &mut rt.state.screen else { unreachable!() };
        chats[0].avatar_id = format!("28:{peer}");
        rt.invalidate();
        rt.frame(0.0);
        assert!(rt.find("audio-call").is_none(), "the bot avatar identifies the chat");

        let Screen::Main { chats, .. } = &mut rt.state.screen else { unreachable!() };
        chats[0].avatar_id.clear();
        if let Some(Ok(page)) = &mut rt.state.messages {
            page.msgs[0].sender_id = format!("28:{peer}");
        }
        rt.invalidate();
        rt.frame(0.0);
        assert!(rt.find("audio-call").is_none(), "a matching bot sender in history identifies the chat");

        rt.state.selected = Some("48:notes".into());
        rt.invalidate();
        rt.frame(0.0);
        assert!(rt.find("audio-call").is_none());
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
                for key in ["search", "sign-out", "nav-chat", "nav-teams", "filter-unread", "filter-chats", "conversation-header", "composer", "draft", "editor", "attach-file", "send"] {
                    let e = elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
                    let [x, y, w, h] = e.rect;
                    assert!(w > 0.0 && h > 0.0 && x >= 0.0 && y >= 0.0 && x + w <= width + 1.0 && y + h <= height + 1.0,
                        "{key} must fit {width}x{height}: {:?}", e.rect);
                }
                let header = elements.iter().find(|e| e.key.as_deref() == Some("conversation-header")).unwrap();
                let draft = elements.iter().find(|e| e.key.as_deref() == Some("draft")).unwrap();
                assert!(draft.rect[1] > header.rect[1] + header.rect[3]);
                let rect = |key: &str| elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
                let composer = rect("composer");
                let incoming = rect("bubble/sample-incoming");
                let outgoing = rect("bubble/sample-outgoing");
                assert!((incoming[0] - composer[0]).abs() < 1.0, "incoming message aligns with the composer at width {width}");
                assert!((outgoing[0] + outgoing[2] - composer[0] - composer[2]).abs() < 1.0,
                    "outgoing message aligns with the composer at width {width}");
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
    fn channel_posts_align_authors_and_expose_quote_replies() {
        let mut s = init();
        channel_posts_demo(&mut s);
        let mut rt = rune::Runtime::new(s, view, (1100.0, 760.0), rune::Env::new(false, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
        settle(&mut rt);

        let elements = rt.elements();
        let get = |key: &str| elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
        assert!(rt.find("channel-posts-heading").is_some());
        assert!(get("conversation-title").rect[2] >= 100.0);
        assert!(rt.find("attach-file").is_none());
        assert_eq!(get("send").label.as_deref(), Some("Post"));
        let incoming = get("post/channel-opening").rect;
        let outgoing = get("post/channel-update").rect;
        let composer = get("composer").rect;
        assert!((incoming[0] - outgoing[0]).abs() < 1.0);
        assert!((incoming[2] - outgoing[2]).abs() < 1.0);
        assert!((incoming[0] - composer[0]).abs() < 1.0);
        assert!((incoming[2] - composer[2]).abs() < 1.0);
        assert!(get("time/channel-update").rect[1] < get("bubble/channel-update").rect[1]);
        let replies = get("post-replies/channel-update");
        for id in ["channel-review", "channel-confirm"] {
            assert!(get(&format!("msg/{id}")).path.starts_with(&replies.path));
            assert!(rt.find(&format!("post/{id}")).is_none(), "replies should render inside the root card");
        }
        assert!(get("msg/channel-review").rect[0] > outgoing[0]);
        assert!(get("msg/channel-confirm").rect[1] > get("msg/channel-review").rect[1]);

        let (x, y) = rt.find("post-quote/channel-review").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        assert_eq!(rt.state.replying.as_ref().unwrap().id, "channel-review");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("draft") && e.focused));
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("send") && e.label.as_deref() == Some("Reply")));

        rt.update(|s, _| s.editor = true);
        settle(&mut rt);
        assert!(rt.find("editor-attach-file").is_none());
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("editor-send") && e.label.as_deref() == Some("Reply")));

        let mut s = init();
        channel_posts_demo(&mut s);
        let mut compact = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(true, 1.0));
        settle(&mut compact);
        let elements = compact.elements();
        for key in ["conversation-title", "channel-posts-heading", "favorite-conversation", "draft", "send"] {
            let [x, y, w, h] = elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
            assert!(w > 0.0 && h > 0.0 && x + w <= 641.0 && y + h <= 401.0, "{key} exceeds the compact window");
        }
        let title = elements.iter().find(|e| e.key.as_deref() == Some("conversation-title")).unwrap();
        assert!(title.rect[2] >= 80.0);
    }

    #[test]
    fn channel_replies_regroup_when_history_loads_the_root() {
        let mut s = init();
        channel_posts_demo(&mut s);
        let messages = &mut s.messages.as_mut().unwrap().as_mut().unwrap().msgs;
        let root = messages.remove(1);
        assert_eq!(messages[1].thread_root.as_deref(), Some(root.server_id.as_str()));
        assert!(messages[1].reply.is_none(), "native replies use the thread ID without quote markup");
        let mut rt = rune::Runtime::new(s, view, (1100.0, 760.0), rune::Env::new(false, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
        settle(&mut rt);
        assert!(rt.find("post/channel-review").is_some());
        assert!(rt.elements().iter().any(|e| e.label.as_deref() == Some("Go to original post")));
        rt.update(move |s, _| s.messages.as_mut().unwrap().as_mut().unwrap().msgs.insert(1, root));
        settle(&mut rt);
        assert!(rt.find("post/channel-review").is_none());
        assert!(rt.find("post-replies/channel-update").is_some());
        assert!(rt.find("msg/channel-review").is_some());
    }

    #[test]
    fn rail_views_scope_unread_filters_and_preserve_the_open_draft() {
        let mut s = init();
        workspace_demo(&mut s);
        s.draft.replace_text("Still writing");
        s.teams.as_mut().unwrap().as_mut().unwrap()[0].channels[0].unread = true;
        let mut rt = rune::Runtime::new(s, view, (1000.0, 680.0), rune::Env::new(false, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| for _ in 0..60 { rt.frame(1.0 / 60.0); };
        let click = |rt: &mut rune::Runtime<State>, key| {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(rt);
        };
        settle(&mut rt);
        assert!(rt.find("nav-chat").is_some());
        assert!(rt.find("nav-teams").is_some());
        assert!(rt.find("filter-chats").is_some());
        assert!(rt.find("filter-channels").is_none());
        assert!(rt.find("favorite/a").is_some());
        assert!(rt.find("chat/b").is_some());
        assert!(rt.find("channel/backend").is_none());

        click(&mut rt, "favorite-conversation");
        assert!(rt.find("favorite/a").is_none());
        assert!(rt.find("chat/a").is_some());
        click(&mut rt, "favorite-conversation");
        assert!(rt.find("favorite/a").is_some());

        click(&mut rt, "filter-unread");
        assert!(rt.state.unread_only);
        assert!(rt.find("chat/b").is_some());
        assert!(rt.find("favorite/a").is_none());
        assert!(rt.find("channel/backend").is_none());

        click(&mut rt, "nav-teams");
        assert_eq!(rt.state.conversation_section, ConversationSection::Teams);
        assert!(!rt.state.unread_only);
        assert!(rt.find("filter-chats").is_none());
        assert!(rt.find("filter-channels").is_some());
        assert!(rt.find("channel/backend").is_some());
        assert!(rt.find("channel/mobile").is_some());
        assert!(rt.find("chat/b").is_none());
        assert!(rt.find("favorite/a").is_none());

        click(&mut rt, "filter-unread");
        assert!(rt.state.unread_only);
        assert!(rt.find("channel/backend").is_some());
        assert!(rt.find("channel/mobile").is_none());
        assert!(rt.find("chat/b").is_none());
        click(&mut rt, "filter-channels");
        assert!(rt.find("channel/mobile").is_some());
        click(&mut rt, "toggle-sidebar");
        assert!(rt.find("sidebar").is_none());
        assert!(rt.find("nav-teams").is_some());
        click(&mut rt, "nav-teams");
        assert!(rt.find("channel/backend").is_some());
        assert_eq!(rt.state.selected.as_deref(), Some("a"));
        assert_eq!(rt.state.draft.text, "Still writing");

        click(&mut rt, "nav-chat");
        rt.update(|s, tx| {
            s.search.replace_text("trential general");
            tx.focus("search", false);
        });
        settle(&mut rt);
        assert!(rt.find("hit/general").is_some());
        click(&mut rt, "hit/general");
        assert_eq!(rt.state.selected.as_deref(), Some("general"));
        assert_eq!(rt.state.conversation_section, ConversationSection::Teams);
        assert!(rt.find("channel/general").is_some());
        assert!(rt.state.search.text.is_empty());
        assert!(rt.state.draft.text.is_empty());
        assert!(rt.find("conversation-header").is_some());
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
