//! Member suggestions shared by the compact and expanded message boxes.
use crate::*;

#[derive(Default)]
pub(crate) struct MentionPicker {
    chat: Option<String>,
    people: Option<Result<Vec<auth::User>, String>>,
    loader: Latest<(String, Result<Vec<auth::User>, String>)>,
    picked: usize,
    dismissed: bool,
}

impl MentionPicker {
    pub(crate) fn reset(&mut self) {
        self.loader.cancel();
        self.chat = None;
        self.people = None;
        self.picked = 0;
        self.dismissed = false;
    }
    pub(crate) fn dismiss(&mut self) { self.dismissed = true; }
    pub(crate) fn setup(&mut self, waker: Waker) { self.loader.set_waker(waker); }
    pub(crate) fn poll(&mut self, selected: &Option<String>) -> bool {
        if let Some((id, result)) = self.loader.take() {
            if selected.as_ref() == Some(&id) && self.chat.as_ref() == Some(&id) { self.people = Some(result); }
            return true;
        }
        false
    }
}

fn query(draft: &TextEdit) -> Option<(std::ops::Range<usize>, &str)> {
    let (start, end) = draft.range();
    if start != end { return None; }
    let before = &draft.text[..start];
    let at = before.rfind('@')?;
    if before[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '@'))
        || draft.pills().any(|(r, _)| r.start <= at && at < r.end)
        || in_code_block(before) { return None; }
    let term = &before[at + 1..];
    if term.len() > 64 || term.contains(['\n', '\r', '@', '`']) { return None; }
    #[cfg(feature = "markdown")]
    if pulldown_cmark::Parser::new(&draft.text).into_offset_iter().any(|(event, range)|
        matches!(event, pulldown_cmark::Event::Code(_) | pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_))) && range.contains(&at)) { return None; }
    Some((at..end, term))
}

fn candidates(s: &State) -> Vec<auth::User> {
    if s.mentions.dismissed { return vec![]; }
    let Some((_, query)) = query(&s.draft) else { return vec![]; };
    let Some(Ok(people)) = &s.mentions.people else { return vec![]; };
    let term = query.to_lowercase();
    people.iter().filter(|u| u.name.to_lowercase().contains(&term) || u.email.to_lowercase().contains(&term)).take(8).cloned().collect()
}

pub(crate) fn mention_changed(s: &mut State, _: &mut Tx) {
    message_state::send_typing(s);
    s.mentions.dismissed = false;
    s.mentions.picked = 0;
    if query(&s.draft).is_none() { return; }
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return; };
    if s.mentions.chat.as_ref() == Some(id) { return; }
    s.mentions.reset();
    s.mentions.chat = Some(id.clone());
    let session = session.clone();
    s.mentions.loader.request(id.clone(), move |id| {
        let people = teams::participants(&session, &id);
        (id, people)
    });
}

fn choose(s: &mut State, user: &auth::User, tx: &mut Tx) {
    if let Some((range, _)) = query(&s.draft) {
        s.draft.replace_with_pill(range, &user.id, &format!("@{}", user.name));
        s.draft.insert(" ");
        s.mentions.dismissed = true;
        tx.focus(if s.editor { "editor-draft" } else { "draft" }, false);
    }
}

pub(crate) fn mention_key(s: &mut State, e: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if e.mods.shift || e.mods.alt || e.mods.ctrl || e.mods.logo || e.mods.cmd
        || s.mentions.dismissed || query(&s.draft).is_none() { return false; }
    if e.key == rune::Key::Escape { s.mentions.dismissed = true; return true; }
    let people = candidates(s);
    if people.is_empty() { return false; }
    match e.key {
        rune::Key::Down => s.mentions.picked = (s.mentions.picked + 1) % people.len(),
        rune::Key::Up => s.mentions.picked = (s.mentions.picked + people.len() - 1) % people.len(),
        rune::Key::Enter | rune::Key::Tab => choose(s, &people[s.mentions.picked.min(people.len() - 1)], tx),
        _ => return false,
    }
    true
}

pub(crate) fn mention_suggestions(s: &State) -> Option<El> {
    if s.mentions.dismissed || query(&s.draft).is_none() { return None; }
    let t = tok::theme(dark());
    let people = candidates(s);
    let list = col().key("mention-suggestions").label("People to mention").w_full().max_h(240.0)
        .scroll(ScrollSpec { stick_bottom: false }).fill(t.card).border(1.0, t.separator).radius(6.0);
    Some(if people.is_empty() {
        list.child(text(match &s.mentions.people {
            Some(Err(_)) => "Could not load members. Plain @text still sends.",
            Some(Ok(_)) => "No matching members",
            None => "Loading members…",
        }).font(type_::LABEL).color(t.text_secondary).padding(8.0))
    } else {
        list.children(people.into_iter().enumerate().map(|(i, user)| {
            let detail = if user.email.is_empty() { user.id.clone() } else { user.email.clone() };
            col().key(format!("mention/{}", user.id)).role("button").focusable(true)
                .label(format!("Mention {} ({detail})", user.name)).padding(8.0).cursor(rune::Cursor::Pointer)
                .fill(if i == s.mentions.picked { t.press } else { t.card }).hover(t.hover)
                .child(text(&user.name).font(type_::BODY).color(t.text))
                .child(text(detail).font(type_::LABEL).color(t.text_secondary))
                .on_press_tx(move |s: &mut State, tx| choose(s, &user, tx))
        }))
    })
}

pub(crate) fn draft_mentions(draft: &TextEdit) -> Vec<teams::Mention> {
    draft.pills().filter(|(_, id)| teams::person_mri(id)).map(|(range, id)| teams::Mention { range, mri: id.into() }).collect()
}

pub(crate) fn edit_draft(message: &Msg) -> TextEdit {
    let mut draft = TextEdit::new(&message.text);
    for m in message.mentions.iter().rev() {
        if let Some(label) = message.text.get(m.range.clone()) {
            draft.replace_with_pill(m.range.clone(), &m.mri, label);
        }
    }
    draft.set_caret(draft.text.len());
    draft
}

#[cfg(test)]
mod tests {
    use super::*;
    const MRI: &str = "8:orgid:11111111-2222-3333-4444-555555555555";

    fn ready(s: &mut State) {
        s.mentions.chat = s.selected.clone();
        s.mentions.people = Some(Ok(vec![
            auth::User { id:MRI.into(), name:"Alícia".into(), email:"one@example.com".into() },
            auth::User { id:"8:orgid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(), name:"Alícia".into(), email:"two@example.com".into() },
        ]));
    }

    #[test]
    fn chips_keep_identity_through_edits_and_navigation_clears_candidates() {
        let mut s = init();
        demo(&mut s);
        ready(&mut s);
        s.draft = TextEdit::new("你好 @Al");
        s.draft.set_caret(s.draft.text.len());
        let mut tx = Tx::default();
        let key = |key| rune::KeyEvent { key, mods:Default::default() };
        assert_eq!(candidates(&s).len(), 2);
        assert!(mention_key(&mut s, &key(rune::Key::Down), &mut tx));
        assert!(mention_key(&mut s, &key(rune::Key::Enter), &mut tx));
        assert_eq!(s.draft.text, "你好 @Alícia ");
        let chosen = draft_mentions(&s.draft)[0].clone();
        assert_ne!(chosen.mri, MRI, "duplicate names must keep the selected identity");
        s.draft.set_caret(0);
        s.draft.insert("prefix ");
        assert_eq!(draft_mentions(&s.draft)[0].range.start, chosen.range.start + 7);
        let (a,b) = { let m = &draft_mentions(&s.draft)[0]; (m.range.start, m.range.end) };
        s.draft.set_selection(a + 1, b - 1);
        s.draft.insert("");
        assert!(draft_mentions(&s.draft).is_empty(), "partial deletion removes the whole chip");
        s.draft.undo();
        assert_eq!(draft_mentions(&s.draft)[0].mri, chosen.mri);
        let mut message = s.messages.as_ref().unwrap().as_ref().unwrap().msgs[0].clone();
        message.text = s.draft.text.clone();
        message.mentions = draft_mentions(&s.draft);
        assert_eq!(draft_mentions(&edit_draft(&message)), message.mentions);
        open_chat(&mut s, "another-chat".into());
        assert!(s.mentions.people.is_none());
        assert!(s.draft.pills().next().is_none());
        ready(&mut s);
        navigate(&mut s, false);
        assert!(s.mentions.people.is_none());
    }

    #[test]
    fn both_composers_allow_keyboard_and_mouse_selection() {
        for expanded in [false, true] {
            let mut s = init();
            demo(&mut s);
            ready(&mut s);
            s.editor = expanded;
            let mut rt = rune::Runtime::new(s, view, (1000.0, 800.0), rune::Env::new(true, 1.0));
            rt.frame(0.0);
            rt.update(move |_, tx| tx.focus(if expanded { "editor-draft" } else { "draft" }, false));
            rt.frame(0.0);
            for ch in "@Al".chars() {
                rt.key(rune::KeyEvent { key:rune::Key::Text(ch.to_string()), mods:Default::default() });
                rt.frame(0.0);
            }
            assert_eq!(candidates(&rt.state).len(), 2);
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            let hit = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&format!("mention/{MRI}"))).unwrap();
            assert!(hit.rect[2] > 0.0 && hit.rect[3] > 0.0);
            rt.pointer_down(hit.rect[0] + 10.0, hit.rect[1] + 10.0);
            rt.pointer_up(hit.rect[0] + 10.0, hit.rect[1] + 10.0);
            rt.frame(0.0);
            assert_eq!(draft_mentions(&rt.state.draft)[0].mri, MRI);
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some(if expanded { "editor-draft" } else { "draft" }) && e.focused));
        }
    }

    #[test]
    fn failed_send_restores_mention_chips() {
        let mut s = init();
        demo(&mut s);
        let mut draft = TextEdit::new("Hello ");
        draft.set_caret(draft.text.len());
        draft.insert_pill(MRI, "@Alícia");
        let expected = draft_mentions(&draft);
        let id = s.selected.clone().unwrap();
        s.sender.request((), move |_| (id, "failed-send".into(), Err(("offline".into(), draft, vec![], vec![], (None, None)))));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut tx = Tx::default();
        while s.sender.is_pending() {
            crate::poll(&mut s, 0.0, &mut tx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        crate::poll(&mut s, 0.0, &mut tx);
        assert_eq!(draft_mentions(&s.draft), expected);
        assert!(s.send_error.as_deref().unwrap().contains("offline"));
    }

    #[test]
    fn escape_keeps_plain_text_and_code_and_emails_do_not_trigger() {
        let mut s = init();
        demo(&mut s);
        ready(&mut s);
        for text in ["a@b", "```rs\n@A", "@a\nnext"] {
            s.draft = TextEdit::new(text);
            s.draft.set_caret(text.len());
            assert!(query(&s.draft).is_none(), "{text}");
        }
        s.draft = TextEdit::new("@Al");
        s.draft.set_caret(3);
        assert!(mention_key(&mut s, &rune::KeyEvent { key:rune::Key::Escape, mods:Default::default() }, &mut Tx::default()));
        assert_eq!(s.draft.text, "@Al");
        assert!(candidates(&s).is_empty());
        assert!(draft_mentions(&s.draft).is_empty());
    }
}
