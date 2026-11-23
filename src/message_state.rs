//! Transient message state. Headless scenes never start network work.
use crate::*;
use std::time::{Duration, Instant};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct MessageState {
    typers: HashMap<(String, String), (String, Instant)>,
    sent_typing: Option<(String, Instant)>,
    pub receipts: Latest<(String, Result<HashMap<String, u64>, String>)>,
    pub(crate) seen: HashMap<String, u64>,
    pub(crate) receipt_chat: Option<String>,
    refreshed: Option<Instant>,
    pub restorer: Latest<(String, Result<Page, String>)>,
}

impl MessageState {
    pub fn clear(&mut self) {
        self.receipts.cancel();
        self.restorer.cancel();
        self.typers.clear();
        self.sent_typing = None;
        self.seen.clear();
        self.receipt_chat = None;
        self.refreshed = None;
    }

    pub fn typing(&mut self, chat: String, user: String, name: String, active: bool) {
        let key = (chat, user);
        if active {
            // Bound unsolicited transient events, including conversations not in the sidebar.
            if self.typers.len() >= 128 && !self.typers.contains_key(&key) { return; }
            self.typers.insert(key, (name, Instant::now() + Duration::from_secs(8)));
        } else { self.typers.remove(&key); }
    }
}

pub(crate) fn typing_label(s: &State, chat: &str) -> Option<String> {
    let mut names: Vec<_> = s.message_state.typers.iter()
        .filter(|((id, _), (_, until))| id == chat && *until > Instant::now())
        .map(|(_, (name, _))| name.as_str()).collect();
    names.sort_unstable();
    match names.as_slice() {
        [] => None,
        [name] => Some(format!("{name} is typing…")),
        [a, b] => Some(format!("{a} and {b} are typing…")),
        _ => Some("Several people are typing…".into()),
    }
}

pub(crate) fn send_typing(s: &mut State) {
    if s.waker.is_none() || cfg!(test) || std::env::args().len() != 1 || s.draft.text.trim().is_empty() { return; }
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    if s.message_state.sent_typing.as_ref().is_some_and(|(chat, at)| chat == id && at.elapsed() < Duration::from_secs(4)) { return; }
    s.message_state.sent_typing = Some((id.clone(), Instant::now()));
    let (session, id) = (session.clone(), id.clone());
    std::thread::spawn(move || {
        if let Err(error) = teams::typing(&session, &id) { eprintln!("Typing notification failed: {error}"); }
    });
}

pub(crate) fn load_receipts(s: &mut State, id: &str) {
    if s.waker.is_none() || cfg!(test) || std::env::args().len() != 1 { return; }
    let Screen::Main { session, .. } = &s.screen else { return };
    if s.message_state.receipt_chat.as_deref() != Some(id) {
        s.message_state.seen.clear();
        s.message_state.receipt_chat = Some(id.into());
    }
    s.message_state.refreshed = Some(Instant::now());
    let session = session.clone();
    s.message_state.receipts.request(id.to_owned(), move |id| {
        let result = teams::read_receipts(&session, &id);
        (id, result)
    });
}

pub(crate) fn delivery(s: &State, m: &Msg) -> Option<&'static str> {
    if !m.mine || m.deleted { return None; }
    if m.server_id.is_empty() { return Some("Sending…"); }
    // Show one receipt under the latest outgoing message, as in Teams.
    let latest = s.messages.as_ref()?.as_ref().ok()?.msgs.iter().rev().find(|m| m.mine && !m.deleted)?;
    if latest.id != m.id { return None; }
    let seen = s.message_state.receipt_chat == s.selected
        && m.server_id.parse::<u64>().ok().is_some_and(|id| s.message_state.seen.values().any(|through| *through >= id));
    Some(if seen { "Seen" } else { "Sent" })
}

pub(crate) fn tombstone(m: &mut Msg) {
    m.deleted = true;
    m.edited = false;
    m.text = "This message was deleted.".into();
    m.raw.clear();
    m.images.clear();
    m.files.clear();
    m.links.clear();
    m.mentions.clear();
    m.reactions.clear();
    m.reply = None;
}

pub(crate) fn undo(s: &mut State, m: &Msg) {
    if !m.mine || !m.deleted || s.message_state.restorer.is_pending() || s.waker.is_none() || (!cfg!(test) && std::env::args().len() != 1) { return; }
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    let (session, message) = (session.clone(), m.server_id.clone());
    s.send_error = None;
    s.message_state.restorer.request(id.clone(), move |id| {
        let result = teams::restore(&session, &id, &message).and_then(|_| teams::restored_page(&session, &id, &message));
        (id, result)
    });
}

pub(crate) fn poll(s: &mut State, tx: &mut Tx) -> bool {
    let mut changed = false;
    let now = Instant::now();
    s.message_state.typers.retain(|_, (_, until)| {
        if *until > now { tx.wake_after(until.duration_since(now)); true } else { changed = true; false }
    });
    if let Some((id, result)) = s.message_state.receipts.take() {
        if s.selected.as_ref() == Some(&id) {
            // A disabled or unavailable receipt must not leave an old Seen indicator visible.
            s.message_state.seen = result.unwrap_or_default();
        }
        changed = true;
    }
    if let Some((id, result)) = s.message_state.restorer.take() {
        history::invalidate(s, &id);
        if result.is_ok() { insights::invalidate(s, &id); }
        match result {
            Ok(fresh) if s.selected.as_ref() == Some(&id) => {
                s.loader.cancel();
                s.messages = Some(Ok(match s.messages.take() {
                    Some(Ok(page)) => teams::merge(page, fresh), _ => fresh,
                }));
            }
            Ok(_) => {},
            Err(e) => s.send_error = Some(format!("Could not restore message: {e}")),
        }
        changed = true;
    }
    // Only read positions are refreshed, and only while the conversation is visible.
    if s.window_visible && s.window_focused && let Some(id) = s.selected.clone() {
        if !s.message_state.receipts.is_pending() && s.message_state.refreshed.is_none_or(|at| at.elapsed() >= Duration::from_secs(15)) {
            load_receipts(s, &id);
        }
        if !s.message_state.receipts.is_pending() && let Some(at) = s.message_state.refreshed { tx.wake_after(Duration::from_secs(15).saturating_sub(at.elapsed())); }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_is_scoped_expires_and_clears() {
        let mut s = init();
        demo(&mut s);
        s.message_state.typing("a".into(), "one".into(), "Charles".into(), true);
        s.message_state.typing("b".into(), "two".into(), "Mary".into(), true);
        assert_eq!(typing_label(&s, "a").as_deref(), Some("Charles is typing…"));
        s.message_state.typing("a".into(), "three".into(), "Grace".into(), true);
        assert_eq!(typing_label(&s, "a").as_deref(), Some("Charles and Grace are typing…"));
        s.message_state.typing("a".into(), "three".into(), String::new(), false);
        s.message_state.typers.get_mut(&("a".into(), "one".into())).unwrap().1 = Instant::now();
        assert!(poll(&mut s, &mut Tx::default()));
        assert!(typing_label(&s, "a").is_none());
        s.message_state.clear();
        assert!(typing_label(&s, "b").is_none());
    }

    #[test]
    fn seen_requires_a_confirmed_horizon_for_this_chat() {
        let mut s = init();
        message_states_demo(&mut s);
        let m = s.messages.as_ref().unwrap().as_ref().unwrap().msgs[1].clone();
        assert_eq!(delivery(&s, &m), Some("Seen"));
        s.message_state.seen.insert("charles".into(), 100);
        assert_eq!(delivery(&s, &m), Some("Sent"));
        s.message_state.seen.insert("charles".into(), 101);
        s.selected = Some("b".into());
        assert_eq!(delivery(&s, &m), Some("Sent"));
        let mut pending = m.clone();
        pending.server_id.clear();
        assert_eq!(delivery(&s, &pending), Some("Sending…"));
        pending.mine = false;
        assert_eq!(delivery(&s, &pending), None);
        let mut deleted = m;
        tombstone(&mut deleted);
        assert_eq!(delivery(&s, &deleted), None);
        assert!(deleted.raw.is_empty() && deleted.images.is_empty() && deleted.mentions.is_empty());
    }

    #[test]
    fn deleted_messages_have_undo_but_no_edit_or_reaction_actions() {
        let mut s = init();
        message_states_demo(&mut s);
        let m = &mut s.messages.as_mut().unwrap().as_mut().unwrap().msgs[1];
        tombstone(m);
        let id = m.id.clone();
        s.menu = Some(id.clone());
        assert!(message_menu(&s).is_none());
        let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(rt.find(&format!("undo/{id}")).is_some());
        assert!(rt.find(&format!("edit/{id}")).is_none());
        assert!(rt.find(&format!("more/{id}")).is_none());
    }
}

#[cfg(test)]
#[test]
fn undo_click_restores_original_and_preserves_stub_on_failure() {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let restored = serde_json::json!({"messages":[{
            "id":"101", "clientmessageid":"sample-outgoing", "from":"8:orgid:me", "imdisplayname":"Ada Lovelace",
            "composetime":"2026-11-17T10:24:00Z", "messagetype":"Text", "content":"Original restored text"
        }]}).to_string();
        let mut requests = Vec::new();
        for (i, stream) in listener.incoming().take(7).enumerate() {
            let mut stream = stream.unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request = line.trim().to_owned();
            let mut length = 0;
            loop {
                line.clear(); reader.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                if let Some((key, value)) = line.split_once(':') && key.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            let mut bytes = vec![0; length]; reader.read_exact(&mut bytes).unwrap();
            requests.push((request, if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() }));
            let (status, body) = match i {
                1 => ("403 Forbidden", "{}".into()),
                3 => ("200 OK", serde_json::json!({"messages":[],"_metadata":{"backwardLink":format!("http://{address}/older")}}).to_string()),
                4 => ("200 OK", restored.clone()),
                5 => ("200 OK", "{\"consumptionhorizons\":[{\"id\":\"8:orgid:other\",\"consumptionhorizon\":\"101;1;c\"}]}".into()),
                _ => ("204 No Content", String::new()),
            };
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    let mut s = init();
    message_states_demo(&mut s);
    if let Screen::Main { session, .. } = &mut s.screen { Arc::make_mut(session).chat_service = format!("http://{address}"); }
    s.waker = Some(Arc::new(|| {}));
    s.deleting = Some(s.messages.as_ref().unwrap().as_ref().unwrap().msgs[1].clone());
    delete_message(&mut s);
    let finish = |s: &mut State| {
        let start = Instant::now();
        loop {
            crate::poll(s, 0.0, &mut Tx::default());
            if !s.deleter.is_pending() && !s.message_state.restorer.is_pending() { break; }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        crate::poll(s, 0.0, &mut Tx::default());
    };
    finish(&mut s);
    assert!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs[1].deleted);
    let mut rt = rune::Runtime::new(s, view, (1200.0, 800.0), rune::Env::new(true, 1.0));
    let click = |rt: &mut rune::Runtime<State>| {
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        let (x, y) = rt.find("undo/sample-outgoing").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    };
    click(&mut rt); finish(&mut rt.state);
    assert!(rt.state.messages.as_ref().unwrap().as_ref().unwrap().msgs[1].deleted);
    assert!(rt.state.send_error.as_ref().unwrap().contains("Could not restore"));
    click(&mut rt); finish(&mut rt.state);
    let page = rt.state.messages.as_ref().unwrap().as_ref().unwrap();
    assert!(!page.msgs[1].deleted);
    assert_eq!(page.msgs[1].text, "Original restored text");
    let Screen::Main { session, .. } = &rt.state.screen else { panic!() };
    assert_eq!(teams::read_receipts(session, "a").unwrap().get("other"), Some(&101));
    teams::typing(session, "a").unwrap();
    let requests = server.join().unwrap();
    assert!(requests[0].0.contains("DELETE /v1/users/ME/conversations/a/messages/101?behavior=softDelete"));
    for request in &requests[1..3] {
        assert!(request.0.contains("DELETE /v1/users/ME/conversations/a/messages/101/properties?name=deletetime"));
        assert_eq!(request.1, serde_json::json!({"deletetime":null}));
    }
    assert!(requests[4].0.starts_with("GET /older "));
    assert!(requests[5].0.contains("/v1/threads/a/consumptionhorizons"));
    assert_eq!(requests[6].1["messagetype"], "Control/Typing");
    assert_eq!(requests[6].1["contenttype"], "Application/Message");
}
