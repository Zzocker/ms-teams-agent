//! What the user's actions do to the state, with the network work they start off the UI thread.

use crate::*;

/// Signs in off the UI thread: from the stored session, else (when `interactive`) with a device
/// code; then loads the chat list.
pub(crate) fn sign_in(s: &mut State, interactive: bool) {
    let attempt = s.attempt.clone();
    let mine = attempt.fetch_add(1, SeqCst) + 1;
    let (send, waker) = (s.events.0.clone(), s.waker.clone());
    s.screen = Screen::Loading;
    std::thread::spawn(move || {
        let emit = |screen| {
            let _ = send.send((mine, screen));
            if let Some(w) = &waker {
                w();
            }
        };
        let session = match auth::resume() {
            // A pressed button signs in fresh when the stored session fails, so an error can't trap the user.
            Ok(None) | Err(_) if interactive => {
                auth::device_login(|user_code, url| emit(Screen::Code { user_code, url }), || attempt.load(SeqCst) == mine).map(Some)
            }
            r => r,
        };
        match session.and_then(|s| s.map(|s| teams::chats(&s).map(|chats| (s, chats))).transpose()) {
            Ok(Some((session, chats))) => {
                let session = Arc::new(session);
                emit(Screen::Main { session: session.clone(), chats: chats.clone() });
                emit(Screen::Main { chats: teams::name_chats(&session, chats), session });
            }
            Ok(None) => emit(Screen::Login(None)),
            Err(e) => emit(Screen::Login(Some(e))),
        }
    });
}

pub(crate) fn sign_out(s: &mut State) {
    #[cfg(feature = "call-audio")]
    { s.calling = Default::default(); }
    s.message_state.clear();
    s.mentions.reset();
    s.history = Default::default();
    s.restore_scroll = None;
    s.prefetch.cancel();
    s.prefetch_queue.clear();
    s.prefetch_id = None;
    s.older_loader.cancel();
    s.deleter.cancel();
    s.reactor.cancel();
    s.avatars = Default::default();
    #[cfg(feature = "image")]
    { s.images = Default::default(); }
    #[cfg(feature = "image")]
    s.preview_rows.clear();
    s.reply_highlight = None;
    s.reply_target = None;
    s.reply_error = None;
    s.attempt.fetch_add(1, SeqCst);
    s.live_attempt = None;
    s.live_status = None;
    s.reader.cancel();
    s.read_in_flight = false;
    s.live_baseline.clear();
    s.delivered.clear();
    s.loading_updates.clear();
    s.teams_loader.cancel();
    s.teams = None;
    s.pending_channels.clear();
    s.conversation_section = ConversationSection::Chat;
    s.ask_me = Default::default();
    s.unread_only = false;
    s.sidebar_hidden = false;
    s.collapsed_teams.clear();
    s.loader.cancel();
    s.sender.cancel();
    s.file_download.cancel();
    s.attached.clear();
    s.pasted.clear();
    s.draft.clear();
    s.editing = None;
    s.editor = false;
    (s.replying, s.menu, s.deleting) = (None, None, None);
    message_search::clear_top(s);
    s.message_search.close();
    s.search_jump = false;
    s.send_error = None;
    auth::sign_out();
    (s.selected, s.messages) = (None, None);
    s.screen = Screen::Login(None);
}

pub(crate) fn load_teams(s: &mut State) {
    if s.waker.is_none() || s.teams.is_some() || s.teams_loader.is_pending() { return; }
    let Screen::Main { session, .. } = &s.screen else { return };
    let session = session.clone();
    s.teams_loader.request((), move |_| teams::teams(&session));
}

/// Reveal a conversation opened outside the current sidebar list.
pub(crate) fn reveal_conversation(s: &mut State, id: &str) {
    s.ask_me.leave();
    if let Some(chat) = notifications::conversation(s, id) {
        let section = if chat.channel { ConversationSection::Teams } else { ConversationSection::Chat };
        if s.conversation_section != section || !chat.unread {
            s.unread_only = false;
        }
        s.conversation_section = section;
    }
    if let Some(Ok(teams)) = &s.teams {
        for team in teams.iter().filter(|team| team.channels.iter().any(|channel| channel.id == id)) {
            s.collapsed_teams.remove(&team.id);
        }
    }
}

pub(crate) fn open_chat(s: &mut State, id: String) {
    message_state::load_receipts(s, &id);
    if s.message_search.scope.as_ref().is_some_and(|scope| scope != &id) {
        s.message_search.close();
    }
    history::remember_selected(s);
    #[cfg(feature = "image")]
    s.preview_rows.clear();
    s.reply_highlight = None;
    s.reply_target = None;
    s.search_jump = false;
    s.reply_error = None;
    let Screen::Main { session, .. } = &s.screen else { return };
    let session = session.clone();
    if s.selected.as_ref() != Some(&id) {
        s.attached.clear();
        s.pasted.clear();
        s.mentions.reset();
        // A draft belongs to the chat it was written in; it must not follow to another one.
        s.draft.clear();
        s.editing = None;
        s.editor = false;
        (s.replying, s.menu, s.deleting) = (None, None, None);
        s.send_error = None;
    }
    s.older_loader.cancel();
    history::stop_prefetch(s, &id);
    s.prefetch_queue.retain(|queued| queued != &id);
    s.messages = s.history.get(&id).map(Ok);
    s.restore_scroll = if s.selected.as_ref() != Some(&id) { s.history.position(&id) } else { None };
    s.selected = Some(id.clone());
    s.loading_updates.clear();
    notifications::mark_selected_read(s);
    if s.waker.is_none() { return; } // Headless scenes do not fetch history.
    s.loader.request(id, move |id| {
        let r = teams::messages(&session, &id);
        (id, r)
    });
}

/// Reveal a quoted message or a search result, following older pages as needed.
pub(crate) fn reveal_reply(s: &mut State, tx: &mut Tx) -> bool {
    if s.ask_me.open { return false; }
    let Some(target) = s.reply_target.as_deref() else { return false };
    if s.messages.as_ref().is_some_and(Result::is_err) {
        s.reply_target = None;
        s.search_jump = false;
        return true;
    }
    let Some(Ok(page)) = &s.messages else { return false };
    let found = (!target.is_empty()).then(|| page.msgs.iter().find(|msg| msg.server_id == target || msg.id == target)).flatten();
    if let Some(msg) = found.filter(|msg| !s.search_jump || !msg.deleted) {
        tx.scroll_center_in("messages", format!("msg/{}", msg.id));
        s.reply_highlight = Some((msg.id.clone(), std::time::Instant::now()));
        s.reply_highlight_on = true;
        tx.wake_after(std::time::Duration::from_millis(150));
        s.reply_target = None;
        s.search_jump = false;
        return true;
    }
    if found.is_some() || target.is_empty() || page.older.is_none() {
        s.reply_target = None;
        s.reply_error = Some(if std::mem::take(&mut s.search_jump) {
            "This search result is unavailable or was deleted."
        } else {
            "The quoted message is unavailable or was deleted."
        }.into());
        return true;
    }
    load_older(s);
    false
}

/// Fetches the page before the loaded messages, when scrolling reaches the top of them.
pub(crate) fn load_older(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id), Some(Ok(page))) = (&s.screen, &s.selected, &s.messages) else { return };
    let Some(link) = page.older.clone() else { return };
    if s.older_loader.is_pending() {
        return;
    }
    let session = session.clone();
    s.older_loader.request((id.clone(), link), move |(id, link)| {
        let r = teams::older(&session, &link);
        (id, link, r)
    });
}

/// Shows the draft in the selected chat at once and sends it (or, while editing, puts it in place
/// of the edited message's text), then reloads its messages.
pub(crate) fn send(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    let text = s.draft.text.clone();
    // One send at a time: `Latest` would drop the first one's answer.
    if (text.trim().is_empty() && s.pasted.is_empty() && s.attached.is_empty()) || s.sender.is_pending() {
        return;
    }
    if s.editing.is_some() && !s.attached.is_empty() {
        s.send_error = Some("Cancel the edit to send the attached files as a new message.".into());
        return;
    }
    let session = session.clone();
    let id = id.clone();
    history::invalidate(s, &id);
    let msg_id = teams::new_msg_id();
    let pasted = std::mem::take(&mut s.pasted);
    let attached = std::mem::take(&mut s.attached);
    let files = attached.iter().map(attachments::DraftFile::card).collect();
    // The pasted pictures show at once under local keys, until their uploads have URLs.
    let local: Vec<String> = (0..pasted.len()).map(|i| local_image(&msg_id, i)).collect();
    #[cfg(feature = "image")]
    for (key, (_, image)) in local.iter().zip(&pasted) {
        s.images.insert(key.clone(), Some(image.clone()));
    }
    let editing = s.editing.take();
    let replying = s.replying.take();
    let mentions = draft_mentions(&s.draft);
    let links = teams::text_links(&text);
    if let (Some(Ok(page)), Some(e)) = (&mut s.messages, &editing) {
        if let Some(m) = page.msgs.iter_mut().find(|m| m.id == e.id) {
            m.text = text.clone();
            m.links = links;
            m.mentions = mentions.clone();
            m.images.extend(local.iter().cloned());
        }
    } else if let Some(Ok(page)) = &mut s.messages {
        // In this input's commit, so it glides in; the reload's copy shares its id and key.
        page.msgs.push(Msg { id: msg_id.clone(), server_id: String::new(), sender: session.user.name.clone(), sender_id: session.user.id.clone(), text: text.clone(), time: teams::now(), mine: true, reply: replying.as_ref().map(|q| teams::Reply { id: q.server_id.clone(), sender: q.sender.clone(), sender_id: q.sender_id.clone(), time: q.time.clone(), text: q.text.clone() }), thread_root: None, images: local, links, files, raw: String::new(), edited: false, deleted: false, reactions: vec![], mentions: mentions.clone() });
    }
    let draft = s.draft.clone();
    s.draft.clear();
    s.mentions.dismiss();
    s.send_error = None;
    s.sender.request((id, msg_id, text, pasted, attached, editing, replying), move |(id, msg_id, text, pasted, mut attached, editing, replying)| {
        let sent = (|| {
            let urls = pasted.iter().map(|(png, _)| teams::upload_image(&session, &id, png.png())).collect::<Result<Vec<_>, _>>()?;
            let sized: Vec<_> = urls.iter().cloned().zip(pasted.iter().map(|(png, _)| png.size())).collect();
            let files = attached.iter_mut().map(|file| teams::upload_file(&session, &id, file)).collect::<Result<Vec<_>, _>>()?;
            match &editing {
                Some(e) => teams::edit(&session, &id, &e.server_id, &e.raw, &text, &sized, &mentions),
                None => teams::send(&session, &id, &msg_id, &text, &sized, &files, replying.as_ref(), &mentions),
            }.map(|()| urls)
        })();
        let r = match sent {
            Ok(urls) => Ok((teams::messages(&session, &id), urls)),
            Err(e) => Err((e, draft, pasted, attached, (editing, replying))),
        };
        (id, msg_id, r)
    });
}

/// The key a sent picture shows under before its upload has a URL.
pub(crate) fn local_image(msg_id: &str, i: usize) -> String {
    format!("local:{msg_id}:{i}")
}

pub(crate) fn react(s: &mut State, message: &Msg, key: &str) {
    if s.reactor.is_pending() || message.server_id.is_empty() || s.waker.is_none() { return; }
    let (Screen::Main { session, .. }, Some(chat)) = (&s.screen, &s.selected) else { return };
    let remove = message.reactions.iter().any(|r| r.key == key && r.mine(&session.user.id));
    let (session, chat, message, key) = (session.clone(), chat.clone(), message.server_id.clone(), key.to_owned());
    s.send_error = None;
    s.menu = None;
    s.reactor.request((), move |_| {
        let result = teams::react(&session, &chat, &message, &key, remove);
        (chat, message, key, remove, result)
    });
}

pub(crate) fn apply_reaction(page: &mut Page, message: &str, key: &str, me: &str, remove: bool) {
    let Some(m) = page.msgs.iter_mut().find(|m| m.server_id == message) else { return };
    if let Some(reaction) = m.reactions.iter_mut().find(|r| r.key == key) {
        reaction.users.retain(|u| u.rsplit(':').next() != Some(me));
        if !remove { reaction.users.push(format!("8:orgid:{me}")); }
    } else if !remove {
        m.reactions.push(teams::Reaction { key: key.into(), users: vec![format!("8:orgid:{me}")] });
    }
    m.reactions.retain(|r| !r.users.is_empty());
}

/// Attach a pasted picture to the draft, decoding its thumbnail off the UI thread.
#[cfg(feature = "image")]
pub(crate) fn paste_image(s: &mut State, png: &rune::ClipboardImage) {
    let Some(waker) = s.waker.clone() else { return };
    let image = rune::ImageCache::new(waker).load(png.png().to_vec());
    s.pasted.push((png.clone(), image));
}

/// Puts `m`'s text in the box to replace it when sent.
pub(crate) fn start_edit(s: &mut State, m: &Msg) {
    s.attached.clear();
    s.pasted.clear();
    s.draft = edit_draft(m);
    s.mentions.dismiss();
    (s.editing, s.replying, s.menu) = (Some(m.clone()), None, None);
    s.send_error = None;
}

/// Quotes `m` above the box, so the next send replies to it.
pub(crate) fn start_reply(s: &mut State, m: &Msg) {
    let mut m = m.clone();
    if m.mine && let Screen::Main { session, .. } = &s.screen {
        m.sender = if session.user.name.is_empty() { "You".into() } else { session.user.name.clone() };
    }
    // A reply is a new message: an edit's text isn't its draft.
    if s.editing.take().is_some() { s.draft.clear(); }
    (s.replying, s.menu) = (Some(m), None);
    s.send_error = None;
}

/// Soft-deletes the confirmed message, keeping an Undo placeholder.
pub(crate) fn delete_message(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id), Some(m)) = (&s.screen, &s.selected, s.deleting.take()) else { return };
    let session = session.clone();
    s.send_error = None;
    s.deleter.request((id.clone(), m), move |(id, m)| {
        let r = teams::delete(&session, &id, &m.server_id);
        (id, m, r)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_quote_uses_account_name_and_preserves_sender_identity() {
        let mut s = init();
        message_states_demo(&mut s);
        let mut original = s.messages.as_ref().unwrap().as_ref().unwrap().msgs[1].clone();
        original.sender = "Display Name".into();
        start_reply(&mut s, &original);
        let Screen::Main { session, .. } = &s.screen else { panic!() };
        assert_eq!(s.replying.as_ref().unwrap().sender, session.user.name);
        // Existing server quotes can also contain the placeholder.
        let raw = r#"<blockquote itemtype="http://schema.skype.com/Reply" itemid="101"><strong itemprop="mri" itemid="8:orgid:me">Display Name</strong><p itemprop="preview">Original</p></blockquote><p>A reply</p>"#;
        let mut page = teams::parse_messages(&serde_json::json!({"messages":[{
            "id":"102", "messagetype":"RichText/Html", "from":"8:orgid:me", "content":raw
        }]}), "me");
        assert_eq!(page[0].reply.as_ref().unwrap().sender_id, original.sender_id);
        s.messages.as_mut().unwrap().as_mut().unwrap().msgs = std::mem::take(&mut page);
        let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(!rt.elements().iter().any(|e| e.text.as_deref().is_some_and(|t| t.contains("Display Name"))));
        start_reply(&mut rt.state, &Msg { mine: false, sender: "Other Person".into(), ..original });
        assert_eq!(rt.state.replying.as_ref().unwrap().sender, "Other Person");
    }

    #[test]
    fn reply_navigation_resolves_server_ids_and_handles_missing_messages() {
        let mut s = init();
        demo(&mut s);
        let mut tx = Tx::default();
        if let Some(Ok(page)) = &mut s.messages {
            page.msgs[0].server_id = "server-original".into();
        }
        s.reply_target = Some("server-original".into());
        assert!(reveal_reply(&mut s, &mut tx));
        assert!(s.reply_target.is_none());
        assert!(s.reply_error.is_none());
        assert_eq!(s.reply_highlight.as_ref().unwrap().0, "2026-11-14T10:20:00Z");
        assert!(s.reply_highlight_on);
        s.reply_target = Some("deleted".into());
        assert!(reveal_reply(&mut s, &mut tx));
        assert!(s.reply_target.is_none());
        assert!(s.reply_error.is_some());
        s.reply_target = Some("old-message".into());
        open_chat(&mut s, "other".into());
        assert!(s.reply_highlight.is_none());
        assert!(s.reply_target.is_none());
        assert!(s.reply_error.is_none());
    }
}
