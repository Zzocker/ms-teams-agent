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
    s.mentions.reset();
    s.history = Default::default();
    s.restore_scroll = None;
    s.prefetch.cancel();
    s.prefetch_queue.clear();
    s.prefetch_id = None;
    s.older_loader.cancel();
    s.deleter.cancel();
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
    s.show_teams = false;
    s.collapsed_teams.clear();
    s.loader.cancel();
    s.sender.cancel();
    s.draft.clear();
    s.editing = None;
    s.editor = false;
    (s.replying, s.menu, s.deleting) = (None, None, None);
    s.search.clear();
    s.send_error = None;
    auth::sign_out();
    (s.selected, s.messages) = (None, None);
    s.screen = Screen::Login(None);
}

pub(crate) fn navigate(s: &mut State, teams: bool) {
    s.mentions.reset();
    history::remember_selected(s);
    s.restore_scroll = None;
    #[cfg(feature = "image")]
    s.preview_rows.clear();
    s.reply_highlight = None;
    s.reply_target = None;
    s.reply_error = None;
    s.show_teams = teams;
    s.search.clear();
    s.loader.cancel();
    s.older_loader.cancel();
    s.selected = None;
    s.messages = None;
    s.draft.clear();
    s.editing = None;
    s.editor = false;
    (s.replying, s.menu, s.deleting) = (None, None, None);
    s.send_error = None;
    if teams && s.teams.is_none() && !s.teams_loader.is_pending() {
        let Screen::Main { session, .. } = &s.screen else { return };
        let session = session.clone();
        s.teams_loader.request((), move |_| teams::teams(&session));
    }
}

pub(crate) fn open_chat(s: &mut State, id: String) {
    history::remember_selected(s);
    #[cfg(feature = "image")]
    s.preview_rows.clear();
    s.reply_highlight = None;
    s.reply_target = None;
    s.reply_error = None;
    let Screen::Main { session, .. } = &s.screen else { return };
    let session = session.clone();
    if s.selected.as_ref() != Some(&id) {
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

/// Reveal the reply target, following older pages until it is found or history runs out.
pub(crate) fn reveal_reply(s: &mut State, tx: &mut Tx) -> bool {
    let Some(target) = s.reply_target.as_deref() else { return false };
    let Some(Ok(page)) = &s.messages else { return false };
    if !target.is_empty() && let Some(msg) = page.msgs.iter().find(|m| m.server_id == target || m.id == target) {
        tx.scroll_center_in("messages", format!("msg/{}", msg.id));
        s.reply_highlight = Some((msg.id.clone(), std::time::Instant::now()));
        s.reply_highlight_on = true;
        tx.wake_after(std::time::Duration::from_millis(150));
        s.reply_target = None;
        return true;
    }
    if target.is_empty() || page.older.is_none() {
        s.reply_target = None;
        s.reply_error = Some("The quoted message is unavailable or was deleted.".into());
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
    if (text.trim().is_empty() && s.pasted.is_empty()) || s.sender.is_pending() {
        return;
    }
    let session = session.clone();
    let id = id.clone();
    history::invalidate(s, &id);
    let msg_id = teams::new_msg_id();
    let pasted = std::mem::take(&mut s.pasted);
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
        page.msgs.push(Msg { id: msg_id.clone(), server_id: String::new(), sender: session.user.name.clone(), sender_id: session.user.id.clone(), text: text.clone(), time: teams::now(), mine: true, reply: replying.as_ref().map(|q| teams::Reply { id: q.server_id.clone(), sender: q.sender.clone(), time: q.time.clone(), text: q.text.clone() }), images: local, links, files: vec![], raw: String::new(), mentions: mentions.clone() });
    }
    let draft = s.draft.clone();
    s.draft.clear();
    s.mentions.dismiss();
    s.send_error = None;
    s.sender.request((id, msg_id, text, pasted, editing, replying), move |(id, msg_id, text, pasted, editing, replying)| {
        let sent = (|| {
            let urls = pasted.iter().map(|(png, _)| teams::upload_image(&session, &id, png.png())).collect::<Result<Vec<_>, _>>()?;
            let sized: Vec<_> = urls.iter().cloned().zip(pasted.iter().map(|(png, _)| png.size())).collect();
            match &editing {
                Some(e) => teams::edit(&session, &id, &e.server_id, &e.raw, &text, &sized, &mentions),
                None => teams::send(&session, &id, &msg_id, &text, &sized, replying.as_ref(), &mentions),
            }.map(|()| urls)
        })();
        let r = match sent {
            Ok(urls) => Ok((teams::messages(&session, &id), urls)),
            Err(e) => Err((e, draft, pasted, (editing, replying))),
        };
        (id, msg_id, r)
    });
}

/// The key a sent picture shows under before its upload has a URL.
pub(crate) fn local_image(msg_id: &str, i: usize) -> String {
    format!("local:{msg_id}:{i}")
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
    s.draft = edit_draft(m);
    s.mentions.dismiss();
    (s.editing, s.replying, s.menu) = (Some(m.clone()), None, None);
    s.send_error = None;
}

/// Quotes `m` above the box, so the next send replies to it.
pub(crate) fn start_reply(s: &mut State, m: &Msg) {
    // A reply is a new message: an edit's text isn't its draft.
    if s.editing.take().is_some() { s.draft.clear(); }
    (s.replying, s.menu) = (Some(m.clone()), None);
    s.send_error = None;
}

/// Deletes the message the user confirmed; it leaves the list once Teams has deleted it.
pub(crate) fn delete_message(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id), Some(m)) = (&s.screen, &s.selected, s.deleting.take()) else { return };
    let session = session.clone();
    s.send_error = None;
    s.deleter.request((id.clone(), m), move |(id, m)| {
        let r = teams::delete(&session, &id, &m.server_id);
        (id, m.id, r)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
        navigate(&mut s, false);
        assert!(s.reply_highlight.is_none());
        assert!(s.reply_target.is_none());
        assert!(s.reply_error.is_none());
    }

    #[test]
    fn navigation_separates_chats_and_channels_without_refetching_loaded_teams() {
        let mut s = init();
        demo(&mut s);
        s.teams = Some(Ok(vec![]));
        s.draft = TextEdit::new("old chat draft");
        s.search = TextEdit::new("old search");
        navigate(&mut s, true);
        assert!(s.show_teams);
        assert!(s.selected.is_none());
        assert!(s.messages.is_none());
        assert!(s.search.text.is_empty());
        assert!(s.draft.text.is_empty());
        assert!(!s.teams_loader.is_pending());
        navigate(&mut s, false);
        assert!(!s.show_teams);
        assert!(s.teams.as_ref().unwrap().as_ref().unwrap().is_empty());
    }
}
