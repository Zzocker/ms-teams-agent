//! Resolves one Ask Me citation without changing the selected Teams conversation.
use crate::*;
use std::collections::HashSet;

pub(super) const PANEL_WIDTH: f32 = 328.0;
const MESSAGES: &str = "ask-me-source-messages";
const CITED: &str = "ask-me-source-cited";
const CONTEXT: usize = 2;
const PAGE_BATCH: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Citation {
    pub(super) conversation: String,
    pub(super) message: String,
}

#[derive(Default)]
pub(super) struct Source {
    preview: Option<Preview>,
    worker: Latest<(Fetch, Result<Page, String>)>,
}

struct Preview {
    citation: Citation,
    origin: String,
    status: Status,
}

enum Status {
    Loading,
    Ready(Vec<Excerpt>),
    Unavailable,
    More(Scan),
    Failed(Scan),
}

struct Excerpt {
    message: Msg,
    cited: bool,
}

struct Scan {
    next: Option<String>,
    visited: HashSet<String>,
    newer: Vec<Msg>,
    remaining: usize,
}

enum Fetch {
    Find(Scan),
    Preceding { link: String, count: usize },
}

impl Default for Scan {
    fn default() -> Self {
        Self { next: None, visited: HashSet::new(), newer: Vec::new(), remaining: PAGE_BATCH }
    }
}

impl Source {
    pub(super) fn dismiss(&mut self) {
        self.preview = None;
        self.worker.cancel();
    }

    pub(super) fn citation(&self) -> Option<&Citation> {
        self.preview.as_ref().map(|preview| &preview.citation)
    }
}

pub(super) fn open(s: &mut State, tx: &mut Tx, citation: Citation, origin: String) {
    s.ask_me.source.dismiss();
    let allowed = !citation.message.is_empty() && notifications::conversation(s, &citation.conversation).is_some();
    let cached = allowed.then(|| {
        s.selected.as_deref().filter(|id| *id == citation.conversation)
            .and_then(|_| s.messages.as_ref()?.as_ref().ok())
            .or_else(|| s.history.page(&citation.conversation))
    }).flatten();
    let scan = Scan::default();
    let (status, next) = match cached {
        Some(page) => examine(&citation, scan, page),
        None if allowed => (Status::Loading, Some(Fetch::Find(scan))),
        None => (Status::Unavailable, None),
    };
    s.ask_me.source.preview = Some(Preview { citation, origin, status });
    if let Some(scan) = next {
        request(s, scan);
    }
    tx.focus("ask-me-source-close", true);
    tx.scroll_center_in(MESSAGES, CITED);
}

pub(super) fn close(s: &mut State, tx: &mut Tx) {
    let origin = s.ask_me.source.preview.as_ref().map(|preview| preview.origin.clone());
    s.ask_me.source.dismiss();
    if let Some(origin) = origin {
        tx.scroll_center_in("ask-me-history", origin.clone());
        tx.focus(origin, true);
    }
}

/// Pages arrive oldest first. Keep the nearest rows crossing the newer page boundary.
fn examine(citation: &Citation, mut scan: Scan, page: &Page) -> (Status, Option<Fetch>) {
    if let Some(index) = page.msgs.iter().position(|message| {
        message.server_id == citation.message || message.id == citation.message
    }) {
        if page.msgs[index].deleted {
            return (Status::Unavailable, None);
        }
        let cited = &page.msgs[index];
        let mut rows: Vec<_> = page.msgs[..index].iter().rev()
            .filter(|message| !message.deleted && !same_message(message, cited)).take(CONTEXT)
            .map(|message| Excerpt { message: message.clone(), cited: false }).collect();
        rows.reverse();
        let preceding_count = CONTEXT - rows.len();
        rows.push(Excerpt { message: cited.clone(), cited: true });
        let limit = rows.len() + CONTEXT;
        for message in page.msgs[index + 1..].iter().chain(&scan.newer) {
            if !message.deleted && !rows.iter().any(|row| same_message(&row.message, message)) {
                rows.push(Excerpt { message: message.clone(), cited: false });
                if rows.len() == limit { break; }
            }
        }
        let preceding = page.older.as_ref()
            .filter(|link| preceding_count > 0 && !scan.visited.contains(*link))
            .map(|link| Fetch::Preceding { link: link.clone(), count: preceding_count });
        return (Status::Ready(rows), preceding);
    }
    let Some(link) = page.older.as_ref().filter(|link| scan.visited.insert((*link).clone())) else {
        return (Status::Unavailable, None);
    };
    scan.next = Some(link.clone());
    scan.newer = page.msgs.iter().chain(&scan.newer).take(CONTEXT).cloned().collect();
    if scan.remaining == 0 {
        (Status::More(scan), None)
    } else {
        (Status::Loading, Some(Fetch::Find(scan)))
    }
}

fn same_message(a: &Msg, b: &Msg) -> bool {
    (!a.id.is_empty() && a.id == b.id) || (!a.server_id.is_empty() && a.server_id == b.server_id)
}

fn request(s: &mut State, mut fetch: Fetch) {
    let source = &mut s.ask_me.source;
    let Some(preview) = &mut source.preview else { return };
    let (Screen::Main { session, .. }, Some(waker)) = (&s.screen, &s.waker) else {
        if matches!(fetch, Fetch::Find(_)) {
            preview.status = Status::Unavailable;
        }
        return;
    };
    let session = session.clone();
    let conversation = preview.citation.conversation.clone();
    if let Fetch::Find(scan) = &mut fetch { scan.remaining -= 1; }
    source.worker.set_waker(waker.clone());
    source.worker.request(fetch, move |fetch| {
        let next = match &fetch {
            Fetch::Find(scan) => scan.next.as_deref(),
            Fetch::Preceding { link, .. } => Some(link.as_str()),
        };
        let result = match next {
            Some(link) => teams::older(&session, link),
            None => teams::messages(&session, &conversation),
        };
        (fetch, result)
    });
}

pub(super) fn poll(s: &mut State, tx: &mut Tx) -> bool {
    let Some((fetch, result)) = s.ask_me.source.worker.take() else { return false };
    let Some(preview) = &mut s.ask_me.source.preview else { return false };
    let scan = match fetch {
        Fetch::Find(scan) => scan,
        Fetch::Preceding { count, .. } => {
            // The source is already usable even when this optional page fails.
            let (Ok(page), Status::Ready(rows)) = (result, &mut preview.status) else { return false };
            let mut preceding: Vec<_> = page.msgs.into_iter().rev()
                .filter(|message| !message.deleted && !rows.iter().any(|row| same_message(&row.message, message)))
                .take(count).map(|message| Excerpt { message, cited: false }).collect();
            preceding.reverse();
            rows.splice(0..0, preceding);
            tx.keep_on_screen(CITED);
            return true;
        }
    };
    let (status, next) = match result {
        Ok(page) => examine(&preview.citation, scan, &page),
        // teams::answer preserves the HTTP status in this error prefix.
        Err(error) if error.strip_prefix("chat service ")
            .and_then(|status| status.split_whitespace().next())
            .is_some_and(|status| matches!(status, "403" | "404" | "410"))
            => (Status::Unavailable, None),
        Err(_) => (Status::Failed(scan), None),
    };
    if matches!(status, Status::Ready(_)) {
        tx.scroll_center_in(MESSAGES, CITED);
    }
    preview.status = status;
    if let Some(scan) = next {
        request(s, scan);
    }
    true
}

fn resume(s: &mut State, tx: &mut Tx) {
    let Some(preview) = &mut s.ask_me.source.preview else { return };
    if !matches!(preview.status, Status::More(_) | Status::Failed(_)) { return; }
    let (Status::More(mut scan) | Status::Failed(mut scan)) =
        std::mem::replace(&mut preview.status, Status::Loading)
    else {
        return;
    };
    scan.remaining = PAGE_BATCH;
    request(s, Fetch::Find(scan));
    tx.focus("ask-me-source-close", true);
}

fn jump(s: &mut State, tx: &mut Tx) {
    let Some(citation) = s.ask_me.source.citation().cloned() else { return };
    if notifications::conversation(s, &citation.conversation).is_none() { return; }
    message_search::jump_to_message(s, tx, &citation.conversation, &citation.message);
    tx.focus("find-messages", false);
}

pub(super) fn panel(s: &State, compact: bool, accent: rune::Color, tint: rune::Color) -> Option<El> {
    let preview = s.ask_me.source.preview.as_ref()?;
    let chat = notifications::conversation(s, &preview.citation.conversation);
    let t = tok::theme(dark());
    let note = |label| text(label).font(type_::BODY).color(t.text_secondary).wrap();
    let title = chat.map_or("Teams conversation", |chat| chat.title.as_str());
    let open_label = if chat.is_some_and(|chat| chat.channel) { "Open in channel" } else { "Open in chat" };
    let action = |key, label| el().key(key).role("button").label(label).focusable(true)
        .min_h(36.0).px(10.0).py(6.0).radius(7.0).center().hover(t.hover)
        .cursor(rune::Cursor::Pointer).child(text(label).font(type_::LABEL).color(accent));
    let panel = col().key("ask-me-source").role("region").label("Cited message")
        .h_full().min_h(0.0).min_w(0.0).gap(12.0).padding(12.0).fill(t.card)
        .border_sides([0.0, 0.0, 0.0, 1.0], t.separator);
    let panel = if compact { panel.grow(1.0) } else { panel.w(PANEL_WIDTH).min_w(PANEL_WIDTH) };
    let panel = panel.child(row().items_center().gap(8.0).w_full()
        .child(col().grow(1.0).min_w(0.0).gap(3.0)
            .child(text("Cited message").role("heading").font(type_::TITLE).color(t.text))
            .child(text(title).font(type_::LABEL).color(t.text_secondary).truncate().help_when_truncated(title)))
        .child(action("ask-me-source-close", "Close").on_press_tx(close)));
    let status = if chat.is_some() { &preview.status } else { &Status::Unavailable };
    Some(match status {
        Status::Loading => panel.child(note("Loading the cited message…").role("status")),
        Status::Unavailable => panel.child(note("Source unavailable. The message may have been deleted or you may no longer have access.").role("status")),
        Status::More(_) => panel.child(note("The cited message is farther back in the conversation.").role("status"))
            .child(action("ask-me-source-more", "Search older messages").fill(t.bg).on_press_tx(resume)),
        Status::Failed(_) => panel.child(note("Couldn’t load the cited message.").role("status"))
            .child(action("ask-me-source-retry", "Retry").fill(t.bg).on_press_tx(resume)),
        Status::Ready(rows) => {
            let today = chrono::Local::now().date_naive();
            panel.child(col().key(MESSAGES).w_full().grow(1.0).min_h(0.0).gap(8.0)
                .scroll(ScrollSpec { stick_bottom: false }).children(rows.iter().map(|row| {
                    let message = &row.message;
                    let sender = if message.sender.is_empty() { "Unknown sender" } else { &message.sender };
                    let body = if !message.text.is_empty() { &message.text }
                        else if !message.files.is_empty() || !message.images.is_empty() { "Attachment" }
                        else { "No message text" };
                    let item = col().w_full().min_w(0.0).gap(4.0).padding(10.0).radius(8.0);
                    let item = if row.cited {
                        item.key(CITED).fill(tint).border(1.0, accent.alpha(0.45))
                            .child(text("Cited message").font(type_::LABEL).color(accent))
                    } else { item.fill(t.bg) };
                    item.child(text(sender).font(type_::TITLE).color(t.text).wrap())
                        .child(text(message_time(&message.time, today)).font(type_::LABEL).color(t.text_secondary))
                        .child(text(body).font(type_::BODY).color(t.text).wrap().selectable(true))
                })))
                .child(action("ask-me-source-open-chat", open_label).fill(tint).on_press_tx(jump))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn retrying_an_older_source_preserves_the_selected_history() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.local_addr().unwrap());
        let older = format!(r#"{{"messages":[{{"id":"archived","messagetype":"Text","imdisplayname":"Ada","content":"Earlier decision","composetime":"2026-11-10T09:00:00Z"}}],"_metadata":{{"backwardLink":"{base}/previous"}}}}"#);
        let worker = std::thread::spawn(move || {
            for (path, status, body) in [
                ("/older", "503 Service Unavailable", "{}"),
                ("/older", "200 OK", older.as_str()),
                ("/previous", "200 OK", r#"{"messages":[{"id":"lead-up","messagetype":"Text","content":"Lead-up","composetime":"2026-11-10T08:00:00Z"},{"id":"archived","messagetype":"Text","content":"Overlapping cited message","composetime":"2026-11-10T09:00:00Z"}]}"#),
            ] {
                let (mut stream, _) = server.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(&format!("GET {path} ")));
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" { break; }
                }
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let mut s = init();
        demo(&mut s);
        s.ask_me.open = true;
        s.waker = Some(Arc::new(|| {}));
        let Screen::Main { session, .. } = &mut s.screen else { panic!("missing demo session") };
        Arc::make_mut(session).chat_service.clone_from(&base);
        let history = s.messages.as_mut().unwrap().as_mut().unwrap();
        history.older = Some(format!("{base}/older"));
        let expected = history.msgs.clone();
        let mut tx = Tx::default();
        open(&mut s, &mut tx, Citation { conversation: "a".into(), message: "archived".into() }, "demo".into());
        let complete = |s: &mut State, tx: &mut Tx| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !poll(s, tx) {
                assert!(Instant::now() < deadline, "source request timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        complete(&mut s, &mut tx);
        assert!(matches!(s.ask_me.source.preview.as_ref().unwrap().status, Status::Failed(_)));
        resume(&mut s, &mut tx);
        complete(&mut s, &mut tx);
        let Status::Ready(rows) = &s.ask_me.source.preview.as_ref().unwrap().status else {
            panic!("retry should load the source");
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().find(|row| row.cited).unwrap().message.text, "Earlier decision");
        complete(&mut s, &mut tx);
        let Status::Ready(rows) = &s.ask_me.source.preview.as_ref().unwrap().status else {
            panic!("the preceding page should retain the source");
        };
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].message.server_id, "lead-up");
        assert_eq!(s.selected.as_deref(), Some("a"));
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs, expected);
        worker.join().unwrap();
    }
}
