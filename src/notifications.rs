//! Apply push updates on the UI thread; deliver native notifications off it.
use crate::*;
use teams::events::Update;

pub fn start(s: &mut State) {
    let (send, receive) = mpsc::sync_channel::<(u64, String, String, String)>(64);
    let attempt = s.attempt.clone();
    let window = s.window.clone();
    std::thread::spawn(move || {
        while let Ok((mine, chat, title, body)) = receive.recv() {
            if attempt.load(SeqCst) != mine { continue; }
            let window = window.clone();
            // Each visible notification waits independently, so later messages can still notify.
            std::thread::spawn(move || {
                let notification = message_notification(&title, &body).show();
                match notification {
                    Ok(notification) => notification.wait_for_action(|action| {
                        if action == "default" && let Some(window) = window {
                            window.update(move |s, _| open_notification(s, mine, &chat));
                        }
                    }),
                    Err(e) => eprintln!("Could not show Teams notification: {e}"),
                }
            });
        }
    });
    s.notifications = Some(send);
}

fn message_notification(title: &str, body: &str) -> notify_rust::Notification {
    let mut notification = notify_rust::Notification::new();
    notification.appname("Teams").icon("ms-teams-agent")
        .summary(title).body(&escape(body)).action("default", "Open chat");
    #[cfg(all(unix, not(target_os = "macos")))]
    notification.sound_name("message-new-instant");
    notification
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn notify(s: &State, chat: &Chat) {
    if let Some(send) = &s.notifications {
        if let Err(error) = send.try_send((s.attempt.load(SeqCst), chat.id.clone(), chat.title.clone(), chat.preview.chars().take(240).collect())) {
            eprintln!("Could not queue Teams notification: {error}");
        }
    }
}

fn notification_current(s: &State, attempt: u64, id: &str) -> bool {
    s.attempt.load(SeqCst) == attempt
        && matches!(&s.screen, Screen::Main { .. }) && conversation(s, id).is_some()
}

fn open_notification(s: &mut State, attempt: u64, id: &str) {
    if !notification_current(s, attempt, id) { return; }
    let channel = conversation(s, id).is_some_and(|c| c.channel);
    if s.show_teams != channel { navigate(s, channel); }
    if channel && let Some(Ok(teams)) = &s.teams {
        for team in teams.iter().filter(|t| t.channels.iter().any(|c| c.id == id)) {
            s.collapsed_teams.remove(&team.id);
        }
    }
    s.search.clear();
    s.viewer = None;
    s.editor = false;
    open_chat(s, id.to_owned());
    if let Some(window) = &s.window { window.open(); }
}

pub fn chat_open(s: &State, id: &str) -> bool {
    s.window_focused && s.window_visible && s.selected.as_deref() == Some(id)
        && s.show_teams == conversation(s, id).is_some_and(|c| c.channel)
}

pub(crate) fn conversation<'a>(s: &'a State, id: &str) -> Option<&'a Chat> {
    let Screen::Main { chats, .. } = &s.screen else { return None };
    s.teams.as_ref().and_then(|r| r.as_ref().ok()).into_iter().flatten()
        .flat_map(|t| &t.channels).chain(&s.pending_channels).chain(chats).find(|c| c.id == id)
}

pub(crate) fn conversation_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Chat> {
    let Screen::Main { chats, .. } = &mut s.screen else { return None };
    s.teams.as_mut().and_then(|r| r.as_mut().ok()).into_iter().flatten()
        .flat_map(|t| &mut t.channels).chain(&mut s.pending_channels).chain(chats).find(|c| c.id == id)
}

fn conversation_rows<'a>(s: &'a mut State, chat: &Chat) -> Option<&'a mut Vec<Chat>> {
    let Screen::Main { chats, .. } = &mut s.screen else { return None };
    let team = s.teams.as_mut().and_then(|r| r.as_mut().ok()).into_iter().flatten()
        .find(|t| t.channels.iter().any(|c| c.id == chat.id));
    if let Some(team) = team {
        chats.retain(|c| c.id != chat.id);
        Some(&mut team.channels)
    } else if chat.channel {
        chats.retain(|c| c.id != chat.id);
        Some(&mut s.pending_channels)
    } else { Some(chats) }
}

pub(crate) fn set_teams(s: &mut State, result: Result<Vec<teams::Team>, String>) {
    if let Ok(teams) = &result {
        if let Screen::Main { chats, .. } = &mut s.screen {
            chats.retain(|chat| {
                if !teams.iter().flat_map(|t| &t.channels).any(|c| c.id == chat.id) { return true }
                s.pending_channels.push(chat.clone());
                false
            });
        }
    }
    s.teams = Some(result.map(|mut teams| {
        s.pending_channels.retain(|pending| {
            let Some(channel) = teams.iter_mut().flat_map(|t| &mut t.channels).find(|c| c.id == pending.id) else { return true };
            if pending.time >= channel.time {
                channel.preview = pending.preview.clone();
                channel.time = pending.time.clone();
                channel.unread = pending.unread;
            }
            false
        });
        teams
    }));
}

pub fn poll(s: &mut State) -> bool {
    let mut changed = false;
    while let Ok((attempt, update)) = s.updates.1.try_recv() {
        if attempt != s.attempt.load(SeqCst) { continue; }
        changed = true;
        match update {
            Update::Session(fresh) => if let Screen::Main { session, .. } = &mut s.screen { *session = fresh; },
            Update::Status(status) => s.live_status = status,
            Update::Connected(fresh) => reconcile(s, fresh),
            Update::Message { chat, message, server_id, new } => receive(s, chat, message, &server_id, new),
        }
    }
    if let Some((id, through, result)) = s.reader.take() {
        match result {
            Ok(()) => {
                if let Some(chat) = conversation_mut(s, &id).filter(|c| c.time <= through) {
                    chat.unread = false;
                }
            },
            Err(error) => s.live_status = Some(format!("Could not save read status: {error}")),
        }
        changed = true;
    }
    changed
}

/// A reconnect snapshot keeps names/avatars and recovers the latest message missed offline.
fn reconcile(s: &mut State, fresh: Vec<Chat>) {
    if !matches!(s.screen, Screen::Main { .. }) { return; }
    let selected = s.selected.clone();
    let mut notices = Vec::new();
    let mut reload = false;
    for mut chat in fresh {
        s.live_baseline.insert(chat.id.clone(), chat.time.clone());
        chat.channel |= conversation(s, &chat.id).is_some_and(|c| c.channel);
        let Some(chats) = conversation_rows(s, &chat) else { continue };
        if let Some(index) = chats.iter().position(|old| old.id == chat.id) {
            let old = &chats[index];
            if chat.time < old.time { continue; }
            let arrived = chat.time > old.time;
            chat.title = old.title.clone();
            chat.avatar_id = old.avatar_id.clone();
            chat.avatar_members = old.avatar_members.clone();
            if arrived && chat.unread { notices.push(chat.clone()); }
            reload |= arrived && selected.as_ref() == Some(&chat.id);
            chats[index] = chat;
        } else {
            if chat.unread { notices.push(chat.clone()); }
            chats.push(chat);
        }
    }
    if let Screen::Main { chats, .. } = &mut s.screen { chats.sort_by(|a, b| b.time.cmp(&a.time)); }
    for chat in notices {
        if !chat_open(s, &chat.id) { notify(s, &chat); }
    }
    if reload { refresh_selected(s); }
}

fn receive(s: &mut State, mut chat: Chat, message: Option<Msg>, server_id: &str, new: bool) {
    chat.channel |= conversation(s, &chat.id).is_some_and(|c| c.channel);
    let open = chat_open(s, &chat.id);
    let here = s.selected.as_ref() == Some(&chat.id);
    if !matches!(s.screen, Screen::Main { .. }) { return; }
    // A replay or an edit can update a row, but cannot notify or move an old chat to the top.
    let key = (chat.id.clone(), server_id.to_owned());
    let arrived = new && !s.delivered.contains(&key)
        && s.live_baseline.get(&chat.id).is_none_or(|time| chat.time > *time)
        && message.as_ref().is_some_and(|m| !m.mine);
    if new {
        s.delivered.push_back(key);
        if s.delivered.len() > 1024 { s.delivered.pop_front(); }
    }
    let Some(chats) = conversation_rows(s, &chat) else { return };
    let prior = chats.iter().position(|c| c.id == chat.id);
    if let Some(i) = prior {
        let old = &chats[i];
        chat.title = old.title.clone();
        chat.avatar_id = old.avatar_id.clone();
        chat.avatar_members = old.avatar_members.clone();
        chat.unread = old.unread || (arrived && !open);
        if chat.time >= old.time {
            chats[i] = chat.clone();
            if new && !chat.channel { chats.sort_by(|a, b| b.time.cmp(&a.time)); }
        } else if arrived && !open {
            chats[i].unread = true;
        }
    } else {
        chat.unread = arrived && !open;
        chats.push(chat.clone());
        if !chat.channel { chats.sort_by(|a, b| b.time.cmp(&a.time)); }
    }
    if here {
        if s.loader.is_pending() {
            s.loading_updates.retain(|(id, _)| id != server_id);
            s.loading_updates.push((server_id.to_owned(), message.clone()));
        }
        // A deletion has an empty body and therefore no parsed message.
        if let Some(Ok(page)) = &mut s.messages {
            apply_message(page, server_id, message.clone());
        } else if s.messages.is_none() {
            // The initial load still owns the history; retain the pushed row until it lands.
            if let Some(message) = message.clone() { s.messages = Some(Ok(Page { msgs: vec![message], older: None })); }
        }
        if open && new { mark_selected_read(s); }
    }
    if arrived && !open { notify(s, &chat); }
}

pub fn apply_message(page: &mut Page, server_id: &str, message: Option<Msg>) {
    page.msgs.retain(|m| m.server_id != server_id && message.as_ref().is_none_or(|fresh| fresh.id != m.id));
    if let Some(message) = message { page.msgs.push(message); }
    page.msgs.sort_by(|a, b| a.time.cmp(&b.time));
}

pub fn mark_selected_read(s: &mut State) {
    if s.waker.is_none() { return; } // Headless scenes never touch the network.
    let (Screen::Main { session, .. }, Some(id), Some(Ok(page))) = (&s.screen, &s.selected, &s.messages) else { return };
    if !chat_open(s, id) { return; }
    let session = session.clone();
    let (id, page) = (id.clone(), page.clone());
    let through = page.msgs.last().map(|m| m.time.clone()).unwrap_or_default();
    s.reader.request(id, move |id| {
        let result = teams::mark_read(&session, &id, &page);
        (id, through, result)
    });
}

fn refresh_selected(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    let (session, id) = (session.clone(), id.clone());
    let read = chat_open(s, &id);
    s.loader.request(id, move |id| {
        let page = teams::messages(&session, &id);
        let result = match &page { Ok(page) if read => teams::mark_read(&session, &id, page), _ => Ok(()) };
        (id, page, result)
    });
}

pub fn focus(s: &mut State, focused: bool, _: &mut Tx) {
    s.window_focused = focused;
    if focused && let Some(id) = &s.selected {
        let unread = conversation(s, id).is_some_and(|c| c.unread);
        if unread { mark_selected_read(s); }
    }
}

pub fn visibility(s: &mut State, visible: bool, _: &mut Tx) {
    s.window_visible = visible;
    if !visible { s.window_focused = false; }
    if visible && s.window_focused { mark_selected_read(s); }
}

pub fn tray(handle: rune::AppHandle<State>) -> Result<rune::Tray, rune::TrayError> {
    let icon = image::load_from_memory(include_bytes!("../assets/icons/icon.png"))
        .map_err(|e| rune::TrayError::Backend(e.to_string()))?.resize(32, 32, image::imageops::FilterType::Triangle).to_rgba8();
    let spec = rune::TraySpec::new("ms-teams-agent", "Teams", rune::TrayIcon::rgba(icon.width(), icon.height(), icon.into_raw()))
        .icon_name("ms-teams-agent").tooltip("Teams — running in background")
        .menu(vec![rune::TrayItem::action("open", "Open Teams"), rune::TrayItem::action("quit", "Quit Teams")]);
    rune::Tray::new(spec, move |event| match event {
        rune::TrayEvent::Activate { token: Some(token) } => handle.activate(token),
        rune::TrayEvent::Activate { .. } => handle.open(),
        rune::TrayEvent::Menu(id) if id == "open" => handle.open(),
        rune::TrayEvent::Menu(id) if id == "quit" => handle.quit(),
        _ => {},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a desktop notification service; shows a brief test notification"]
    fn native_notification() {
        message_notification("Teams notifications", "Notification sound test.")
            .timeout(2000).show().expect("desktop notification service");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn message_notification_requests_desktop_sound() {
        let notification = message_notification("New message", "<hello> & welcome");
        assert!(notification.hints.contains(&notify_rust::Hint::SoundName("message-new-instant".into())));
        assert!(!notification.hints.contains(&notify_rust::Hint::SuppressSound(true)));
        assert_eq!(notification.body, "&lt;hello&gt; &amp; welcome");
        assert_eq!(notification.actions, ["default", "Open chat"]);
    }
    fn signed_in() -> State {
        let mut s = init();
        s.screen = Screen::Main { session: Arc::new(Session::default()), chats: vec![] };
        s
    }
    fn incoming(id: &str, time: &str) -> (Chat, Msg) {
        let resource = serde_json::json!({"id":time,"clientmessageid":time,"messagetype":"Text","from":"8:orgid:other","content":"hello","composetime":time});
        let chat = teams::parse_chats(&serde_json::json!({"conversations":[{"id":id,"lastMessage":resource}]}), &auth::User::default()).pop().unwrap();
        let msg = teams::parse_messages(&serde_json::json!({"messages":[resource]}), "me").pop().unwrap();
        (chat, msg)
    }

    fn channel(time: &str) -> (Chat, Msg) {
        let (mut chat, message) = incoming("19:fixture@thread.v2", time);
        chat.id = "19:channel@thread.tacv2".into();
        chat.channel = true;
        (chat, message)
    }

    fn team() -> teams::Team {
        let (mut chat, _) = channel("");
        chat.title = "General".into();
        teams::Team { id: "team".into(), title: "Engineering".into(), channels: vec![chat] }
    }

    #[test]
    fn channel_updates_wait_for_metadata_and_never_create_chat_rows() {
        let mut s = signed_in();
        let (send, notices) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        let (chat, msg) = channel("1");
        receive(&mut s, chat.clone(), Some(msg.clone()), "1", true);
        receive(&mut s, chat.clone(), Some(msg), "1", true);
        assert_eq!(s.pending_channels.len(), 1);
        assert!(s.pending_channels[0].unread);
        assert_eq!(notices.try_iter().count(), 1);
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert!(chats.is_empty());
        s.collapsed_teams.insert("team".into());
        set_teams(&mut s, Ok(vec![team()]));
        assert!(s.pending_channels.is_empty());
        let loaded = conversation(&s, &chat.id).unwrap();
        assert_eq!(loaded.title, "General");
        assert_eq!(loaded.time, "1");
        assert!(loaded.unread);
        let (mut newer, msg) = channel("2");
        // Loaded membership must win even when an event lacks channel metadata.
        newer.channel = false;
        receive(&mut s, newer, Some(msg), "2", true);
        assert_eq!(conversation(&s, &chat.id).unwrap().time, "2");
        assert!(s.collapsed_teams.contains("team"));
        let (direct, msg) = incoming("19:new@thread.v2", "3");
        receive(&mut s, direct, Some(msg), "3", true);
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].id, "19:new@thread.v2");
    }

    #[test]
    fn channel_reconnect_and_open_channel_updates_stay_in_teams() {
        let mut s = signed_in();
        set_teams(&mut s, Ok(vec![team()]));
        let (send, notices) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        let (mut chat, msg) = channel("1");
        chat.unread = true;
        reconcile(&mut s, vec![chat.clone()]);
        receive(&mut s, chat.clone(), Some(msg), "1", true);
        assert_eq!(notices.try_iter().count(), 1);
        assert_eq!(conversation(&s, &chat.id).unwrap().title, "General");
        s.selected = Some(chat.id.clone());
        s.show_teams = true;
        s.window_focused = true;
        s.window_visible = true;
        s.messages = Some(Ok(Page { msgs: vec![], older: None }));
        let (chat, msg) = channel("2");
        for _ in 0..2 { receive(&mut s, chat.clone(), Some(msg.clone()), "2", true); }
        assert!(notices.try_recv().is_err());
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len(), 1);
        let (chat, mut edit) = channel("3");
        edit.server_id = "2".into();
        receive(&mut s, chat, Some(edit), "2", false);
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len(), 1);
        assert!(notices.try_recv().is_err());
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert!(chats.is_empty());
    }

    #[test]
    fn channel_notification_selects_teams_and_expands_its_team() {
        let mut s = signed_in();
        set_teams(&mut s, Ok(vec![team()]));
        s.collapsed_teams.insert("team".into());
        let id = s.teams.as_ref().unwrap().as_ref().unwrap()[0].channels[0].id.clone();
        assert!(notification_current(&s, 0, &id));
        open_notification(&mut s, 0, &id);
        assert!(s.show_teams);
        assert_eq!(s.selected.as_deref(), Some(id.as_str()));
        assert!(!s.collapsed_teams.contains("team"));
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert!(chats.is_empty());
    }

    #[test]
    fn channel_snapshot_before_metadata_and_legacy_rows_merge_into_teams() {
        let mut s = signed_in();
        let (mut chat, _) = channel("2");
        chat.unread = true;
        reconcile(&mut s, vec![chat.clone()]);
        assert_eq!(s.pending_channels.len(), 1);
        let (mut legacy, _) = incoming("opaque-channel", "1");
        legacy.unread = true;
        if let Screen::Main { chats, .. } = &mut s.screen { chats.push(legacy.clone()); }
        let mut metadata = team();
        legacy.channel = true;
        legacy.title = "Legacy channel".into();
        legacy.time.clear();
        metadata.channels.push(legacy);
        set_teams(&mut s, Ok(vec![metadata]));
        assert!(s.pending_channels.is_empty());
        assert_eq!(conversation(&s, &chat.id).unwrap().time, "2");
        let legacy = conversation(&s, "opaque-channel").unwrap();
        assert_eq!(legacy.title, "Legacy channel");
        assert!(legacy.channel && legacy.unread);
        assert_eq!(legacy.time, "1");
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert!(chats.is_empty());
    }
    #[test]
    fn incoming_chat_moves_first_and_notifies_once() {
        let mut s = signed_in();
        let (send, receive_notice) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        for (id, time) in [("19:a", "1"), ("19:b", "2"), ("19:a", "3"), ("19:a", "3")] {
            let (chat, msg) = incoming(id, time);
            receive(&mut s, chat, Some(msg), time, true);
        }
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert_eq!(chats.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["19:a", "19:b"]);
        assert!(chats[0].unread);
        let notices = receive_notice.try_iter().collect::<Vec<_>>();
        assert_eq!(notices.len(), 3);
        assert_eq!(notices[2].1, "19:a", "click action must retain the destination chat");
    }

    #[test]
    fn notification_clicks_require_the_same_signed_in_session_and_an_existing_chat() {
        let mut s = signed_in();
        let (chat, _) = incoming("19:a", "1");
        if let Screen::Main { chats, .. } = &mut s.screen { chats.push(chat); }
        assert!(notification_current(&s, 0, "19:a"));
        assert!(!notification_current(&s, 0, "19:missing"));
        s.attempt.fetch_add(1, SeqCst);
        assert!(!notification_current(&s, 0, "19:a"));
        s.screen = Screen::Login(None);
        assert!(!notification_current(&s, 1, "19:a"));
    }
    #[test]
    fn visible_open_chat_is_quiet_but_background_chat_notifies() {
        let mut s = signed_in();
        let (send, notices) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        s.selected = Some("19:a".into());
        s.window_focused = true;
        s.window_visible = true;
        // No read-marker worker is needed for this state-only test.
        let (chat, msg) = incoming("19:a", "1");
        receive(&mut s, chat, Some(msg), "1", true);
        assert!(notices.try_recv().is_err());
        s.window_focused = false;
        let (chat, msg) = incoming("19:a", "2");
        receive(&mut s, chat, Some(msg), "2", true);
        assert!(notices.try_recv().is_ok());
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len(), 2);
    }
    #[test]
    fn edits_self_messages_and_stale_sessions_do_not_notify() {
        let mut s = signed_in();
        let (send, notices) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        let (chat, mut msg) = incoming("19:a", "1");
        msg.mine = true;
        receive(&mut s, chat, Some(msg), "1", true);
        let (chat, msg) = incoming("19:a", "2");
        receive(&mut s, chat, Some(msg), "2", false);
        s.updates.0.send((99, Update::Status(Some("old account".into())))).unwrap();
        assert!(!poll(&mut s));
        assert!(s.live_status.is_none());
        assert!(notices.try_recv().is_err());
        assert_eq!(escape("<b>A&B</b>"), "&lt;b&gt;A&amp;B&lt;/b&gt;");
    }

    #[test]
    fn reconnect_recovers_unread_without_notifying_again_for_queued_replays() {
        let mut s = signed_in();
        let (send, notices) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        let (mut old, _) = incoming("19:a", "1");
        old.title = "Resolved name".into();
        if let Screen::Main { chats, .. } = &mut s.screen { chats.push(old); }
        let (mut fresh, msg) = incoming("19:a", "2");
        fresh.unread = true;
        reconcile(&mut s, vec![fresh.clone()]);
        receive(&mut s, fresh, Some(msg), "2", true);
        assert_eq!(notices.try_iter().count(), 1);
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert_eq!(chats[0].title, "Resolved name");
        assert!(chats[0].unread);
    }

    #[test]
    fn same_time_and_out_of_order_messages_notify_without_reordering_backwards() {
        let mut s = signed_in();
        let (send, notices) = mpsc::sync_channel(8);
        s.notifications = Some(send);
        for (id, time) in [("one", "3"), ("two", "3"), ("three", "2")] {
            let (chat, mut msg) = incoming("19:a", time);
            msg.id = id.into();
            msg.server_id = id.into();
            receive(&mut s, chat, Some(msg), id, true);
        }
        assert_eq!(notices.try_iter().count(), 3);
        let Screen::Main { chats, .. } = &s.screen else { panic!() };
        assert_eq!(chats[0].time, "3");
    }
}
