//! Search the same account-scoped history used by the conversation pane. Older pages are
//! visited in bounded batches, with one cursor per conversation.
use crate::*;
use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::time::{Duration, Instant};

const PAGE_BATCH: usize = 12;
const CONCURRENT_PAGES: usize = 4;
const SEARCH_DELAY: Duration = Duration::from_millis(350);
const PREVIEW_LIMIT: usize = 3;

#[derive(Clone)]
pub(crate) struct Hit {
    pub conversation: String,
    pub channel: bool,
    pub id: String,
    pub sender: String,
    pub time: String,
    pub snippet: String,
    pub matches: Vec<Range<usize>>,
}

#[derive(Clone)]
struct Cursor {
    conversation: String,
    channel: bool,
    older: Option<String>,
}

struct Batch {
    count: usize,
    hits: Vec<Hit>,
    next: Vec<Cursor>,
    failed: Vec<Cursor>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Filter {
    #[default]
    All,
    Chats,
    Channels,
}

impl Filter {
    pub fn includes(self, channel: bool) -> bool {
        match self {
            Self::All => true,
            Self::Chats => !channel,
            Self::Channels => channel,
        }
    }
}

#[derive(Default)]
pub(crate) struct Search {
    pub open: bool,
    pub scope: Option<String>,
    pub input: TextEdit,
    pub query: String,
    pub filter: Filter,
    pub hits: Vec<Hit>,
    waiting_for_channels: bool,
    live_deadline: Option<Instant>,
    budget: usize,
    cursors: VecDeque<Cursor>,
    failed: Vec<Cursor>,
    seen: HashSet<(String, Option<String>)>,
    hit_ids: HashSet<(String, String)>,
    worker: Latest<Batch>,
}

impl Search {
    pub fn set_waker(&mut self, waker: Waker) { self.worker.set_waker(waker); }

    pub fn busy(&self) -> bool {
        self.waiting_for_channels || self.live_deadline.is_some() || self.budget > 0
    }

    pub fn has_more(&self) -> bool { !self.cursors.is_empty() }

    pub fn has_failures(&self) -> bool { !self.failed.is_empty() }

    pub fn close(&mut self) {
        self.worker.cancel();
        self.open = false;
        self.scope = None;
        self.input.clear();
        self.clear_results();
    }

    fn clear_results(&mut self) {
        self.worker.cancel();
        self.query.clear();
        self.waiting_for_channels = false;
        self.live_deadline = None;
        self.budget = 0;
        self.hits.clear();
        self.cursors.clear();
        self.failed.clear();
        self.seen.clear();
        self.hit_ids.clear();
    }

    fn add_hits(&mut self, hits: Vec<Hit>) {
        self.hits.extend(hits.into_iter().filter(|hit| self.hit_ids.insert((hit.conversation.clone(), hit.id.clone()))));
        sort_hits(&mut self.hits);
    }

    fn enqueue(&mut self, cursor: Cursor) {
        if self.seen.insert((cursor.conversation.clone(), cursor.older.clone())) {
            self.cursors.push_back(cursor);
        }
    }
}

pub(crate) struct Preview {
    query: String,
    hits: Vec<Hit>,
    pending: bool,
    worker: Latest<Batch>,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            query: String::new(),
            hits: Vec::new(),
            pending: false,
            worker: Latest::default().debounce(SEARCH_DELAY),
        }
    }
}

impl Preview {
    pub fn set_waker(&mut self, waker: Waker) { self.worker.set_waker(waker); }

    pub fn results(&self, query: &str) -> &[Hit] {
        if self.query == query { &self.hits } else { &[] }
    }

    pub fn pending(&self, query: &str) -> bool {
        self.query == query && self.pending
    }

    fn clear(&mut self) {
        self.worker.cancel();
        self.query.clear();
        self.hits.clear();
        self.pending = false;
    }

    fn poll(&mut self) -> bool {
        let Some(batch) = self.worker.take() else { return false };
        self.pending = false;
        self.hits.extend(batch.hits);
        trim_preview(&mut self.hits);
        true
    }
}

pub(crate) fn clear_top(s: &mut State) {
    s.search.clear();
    s.search_preview.clear();
}

pub(crate) fn preview_changed(s: &mut State, _: &mut Tx) {
    let query = s.search.text.trim().to_lowercase();
    if s.search_preview.query == query { return; }
    s.search_preview.clear();
    if query.is_empty() { return; }
    let (mut hits, mut cursors) = candidates(s, None, &query);
    trim_preview(&mut hits);
    let Screen::Main { session, .. } = &s.screen else { return };
    let preview = &mut s.search_preview;
    preview.hits = hits;
    preview.query = query.clone();
    if s.waker.is_some() && query.chars().take(2).count() == 2 && !cursors.is_empty() {
        cursors.truncate(CONCURRENT_PAGES);
        preview.pending = true;
        let session = session.clone();
        preview.worker.request(cursors, move |cursors| fetch_batch(&session, &query, cursors));
    }
}

fn trim_preview(hits: &mut Vec<Hit>) {
    sort_hits(hits);
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert((hit.conversation.clone(), hit.id.clone())));
    hits.truncate(PREVIEW_LIMIT);
}

fn sort_hits(hits: &mut [Hit]) {
    hits.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.conversation.cmp(&b.conversation)).then_with(|| a.id.cmp(&b.id)));
}

pub(crate) fn open_scoped(s: &mut State, tx: &mut Tx) -> bool {
    let Some(id) = s.selected.clone() else { return false };
    clear_top(s);
    if s.message_search.scope.as_ref() != Some(&id) {
        s.message_search.close();
        s.message_search.scope = Some(id);
        s.message_search.filter = Filter::All;
    }
    s.message_search.open = true;
    tx.focus("message-search-input", false);
    true
}

pub(crate) fn open_global(s: &mut State, tx: &mut Tx) {
    let query = s.search.text.trim().to_owned();
    if query.is_empty() { return; }
    clear_top(s);
    s.message_search.close();
    s.message_search.open = true;
    s.message_search.filter = Filter::All;
    s.message_search.input.replace_text(query);
    submit(s);
    tx.focus("message-search-input", false);
}

pub(crate) fn submit(s: &mut State) {
    let query = s.message_search.input.text.trim().to_owned();
    if s.message_search.scope.is_some() && s.message_search.query == query && s.message_search.busy() {
        if s.message_search.live_deadline.take().is_some() { more(s); }
        return;
    }
    set_query(s, query);
    more(s);
}

pub(crate) fn scoped_input_changed(s: &mut State, tx: &mut Tx) {
    if s.message_search.scope.is_none() { return; }
    let query = s.message_search.input.text.trim().to_owned();
    if s.message_search.query == query { return; }
    set_query(s, query);
    tx.scroll_to_instant("message-search-results", 0.0, 0.0);
    if s.message_search.has_more() {
        s.message_search.live_deadline = Some(Instant::now() + SEARCH_DELAY);
        tx.wake_after(SEARCH_DELAY);
    }
}

fn set_query(s: &mut State, query: String) {
    s.message_search.clear_results();
    if query.is_empty() { return; }
    s.message_search.query = query;
    if s.message_search.scope.is_none() && s.teams.is_none() && s.waker.is_some() {
        s.message_search.waiting_for_channels = true;
        load_teams(s);
    } else {
        seed(s);
    }
}

fn seed(s: &mut State) {
    let (hits, cursors) = candidates(s, s.message_search.scope.as_deref(), &s.message_search.query);
    s.message_search.add_hits(hits);
    if s.waker.is_some() {
        for cursor in cursors { s.message_search.enqueue(cursor); }
    }
}

fn candidates(s: &State, scope: Option<&str>, query: &str) -> (Vec<Hit>, Vec<Cursor>) {
    let Screen::Main { chats, .. } = &s.screen else { return (vec![], vec![]) };
    let channels = s.teams.as_ref().and_then(|teams| teams.as_ref().ok()).into_iter().flatten().flat_map(|team| &team.channels);
    let mut ids = HashSet::new();
    let mut sources: Vec<&Chat> = channels.chain(&s.pending_channels).chain(chats)
        .filter(|chat| scope.is_none_or(|id| id == chat.id) && ids.insert(&chat.id))
        .collect();
    sources.sort_by(|a, b| b.time.cmp(&a.time));

    let query = query.to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    // Queue unvisited conversations before continuing older history in cached conversations.
    let mut hits = Vec::new();
    let mut cursors = Vec::new();
    let mut older = Vec::new();
    for chat in sources {
        let page = if s.selected.as_ref() == Some(&chat.id) {
            s.messages.as_ref().and_then(|page| page.as_ref().ok())
        } else {
            s.history.page(&chat.id)
        };
        let cursor = Cursor { conversation: chat.id.clone(), channel: chat.channel, older: None };
        if let Some(page) = page {
            hits.extend(find_hits(&cursor, page, &words));
            if let Some(link) = page.older.clone() {
                older.push(Cursor { older: Some(link), ..cursor });
            }
        } else {
            cursors.push(cursor);
        }
    }
    cursors.extend(older);
    (hits, cursors)
}

pub(crate) fn more(s: &mut State) {
    if !s.message_search.busy() && s.message_search.has_more() {
        s.message_search.budget = PAGE_BATCH;
        request_next(s);
    }
}

pub(crate) fn retry(s: &mut State) {
    let search = &mut s.message_search;
    search.cursors.extend(std::mem::take(&mut search.failed));
    more(s);
}

fn request_next(s: &mut State) {
    let Screen::Main { session, .. } = &s.screen else { return };
    let search = &mut s.message_search;
    let count = CONCURRENT_PAGES.min(search.budget).min(search.cursors.len());
    if count == 0 {
        search.budget = 0;
        return;
    }
    let cursors: Vec<_> = search.cursors.drain(..count).collect();
    let session = session.clone();
    let query = search.query.to_lowercase();
    search.worker.request(cursors, move |cursors| fetch_batch(&session, &query, cursors));
}

fn fetch_batch(session: &Session, query: &str, cursors: Vec<Cursor>) -> Batch {
    let words: Vec<_> = query.split_whitespace().collect();
    let count = cursors.len();
    std::thread::scope(|scope| {
        let requests: Vec<_> = cursors.into_iter().map(|cursor| {
            let words = &words;
            scope.spawn(move || {
                let result = match &cursor.older {
                    Some(link) => teams::older(session, link),
                    None => teams::messages(session, &cursor.conversation),
                };
                match result {
                    Ok(page) => Ok((find_hits(&cursor, &page, words), page.older.map(|link| Cursor { older: Some(link), ..cursor }))),
                    Err(_) => Err(cursor),
                }
            })
        }).collect();
        let mut batch = Batch { count, hits: vec![], next: vec![], failed: vec![] };
        for request in requests {
            match request.join().unwrap() {
                Ok((hits, next)) => { batch.hits.extend(hits); batch.next.extend(next); }
                Err(cursor) => batch.failed.push(cursor),
            }
        }
        batch
    })
}

pub(crate) fn poll(s: &mut State, tx: &mut Tx) -> bool {
    let preview_changed = s.search_preview.poll();
    if s.message_search.waiting_for_channels && s.teams.is_some() {
        s.message_search.waiting_for_channels = false;
        seed(s);
        more(s);
        return true;
    }
    if let Some(deadline) = s.message_search.live_deadline {
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            tx.wake_after(remaining);
            return preview_changed;
        }
        s.message_search.live_deadline = None;
        more(s);
        return true;
    }
    let Some(batch) = s.message_search.worker.take() else { return preview_changed };
    let search = &mut s.message_search;
    search.budget -= batch.count;
    search.add_hits(batch.hits);
    search.failed.extend(batch.failed);
    for next in batch.next { search.enqueue(next); }
    request_next(s);
    true
}

fn find_hits(cursor: &Cursor, page: &Page, words: &[&str]) -> Vec<Hit> {
    page.msgs.iter().filter_map(|msg| {
        if msg.deleted || msg.server_id.is_empty() { return None; }
        let body = if msg.files.is_empty() {
            std::borrow::Cow::Borrowed(msg.text.as_str())
        } else {
            std::borrow::Cow::Owned(format!("{} {}", msg.text, msg.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>().join(" ")))
        };
        let lower = body.to_lowercase();
        words.iter().all(|word| lower.contains(word)).then(|| {
            let (snippet, matches) = snippet(&body, words);
            Hit {
                conversation: cursor.conversation.clone(),
                channel: cursor.channel,
                id: msg.server_id.clone(),
                sender: msg.sender.clone(),
                time: msg.time.clone(),
                snippet,
                matches,
            }
        })
    }).collect()
}

/// Find lowercased query terms and map the hits to byte ranges in the displayed text.
pub(crate) fn matching_ranges(text: &str, words: &[&str]) -> Vec<Range<usize>> {
    let lower = text.to_lowercase();
    let mut ranges = Vec::new();
    for word in words.iter().filter(|word| !word.is_empty()) {
        let mut offset = 0;
        while let Some(found) = lower[offset..].find(word) {
            let start = offset + found;
            ranges.push(start..start + word.len());
            offset = start + lower[start..].chars().next().unwrap().len_utf8();
        }
    }
    if !ranges.is_empty() && !text.is_ascii() && lower != text {
        // Lowercasing can expand a character. A partial match in that expansion highlights
        // the source character as a whole.
        let mut lowered_end = 0;
        let boundaries: Vec<_> = text.char_indices().map(|(start, ch)| {
            lowered_end += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
            (lowered_end, start, start + ch.len_utf8())
        }).collect();
        for range in &mut ranges {
            let start = boundaries.partition_point(|(end, ..)| *end <= range.start);
            let end = boundaries.partition_point(|(end, ..)| *end < range.end);
            *range = boundaries[start].1..boundaries[end].2;
        }
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() && range.start <= last.end {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn snippet(body: &str, words: &[&str]) -> (String, Vec<Range<usize>>) {
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let matches = matching_ranges(&body, words);
    let first = matches.first().map_or(0, |range| range.start);
    let mut start = body[..first].char_indices().rev().nth(18).map_or(0, |(byte, _)| byte);
    if start > 0 && let Some(space) = body[start..first].find(' ') {
        start += space + 1;
    }
    let end = body[start..].char_indices().nth(180).map_or(body.len(), |(byte, _)| start + byte);
    let mut snippet = String::new();
    if start > 0 { snippet.push('…'); }
    let prefix = snippet.len();
    snippet.push_str(&body[start..end]);
    if end < body.len() { snippet.push('…'); }
    let matches = matches.into_iter().take_while(|range| range.start < end)
        .map(|range| prefix + range.start - start..prefix + range.end.min(end) - start)
        .collect();
    (snippet, matches)
}

pub(crate) fn jump(s: &mut State, tx: &mut Tx, hit: &Hit) {
    if s.selected.as_ref() != Some(&hit.conversation) {
        reveal_conversation(s, &hit.conversation);
        open_chat(s, hit.conversation.clone());
    }
    s.reply_error = None;
    s.reply_target = Some(hit.id.clone());
    s.search_jump = true;
    reveal_reply(s, tx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn highlights_case_insensitive_and_overlapping_matches_at_display_offsets() {
        let text = "İSTANBUL · banana · Sigma ΟΣ";
        let ranges = matching_ranges(text, &["i", "ana", "σ", "ς"]);
        let marked: Vec<_> = ranges.into_iter().map(|range| &text[range]).collect();
        assert_eq!(marked, ["İ", "anana", "i", "Σ"]);
        assert_eq!(matching_ranges("GeAr gears", &["gear"]), [0..4, 5..9]);
    }

    #[test]
    fn snippets_keep_matches_after_long_prefixes_and_whitespace_changes() {
        let body = format!("{}✅NeEdLe\n\tneedle {}", "前".repeat(240), "続".repeat(200));
        let (text, ranges) = snippet(&body, &["needle"]);
        assert!(text.starts_with('…') && text.ends_with('…'));
        let marked: Vec<_> = ranges.into_iter().map(|range| &text[range]).collect();
        assert_eq!(marked, ["NeEdLe", "needle"]);
    }

    const ARCHIVED_MESSAGE: &str =
        r#"{"messages":[{"id":"older-match","messagetype":"Text","imdisplayname":"Charles","content":"An archived test message","composetime":"2026-11-10T09:00:00Z"}]}"#;

    fn older_server<const N: usize>(responses: [(&'static str, &'static str); N]) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = BufReader::new(&socket);
                let mut line = String::new();
                request.read_line(&mut line).unwrap();
                assert!(line.starts_with("GET /older "));
                loop {
                    line.clear();
                    if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" { break; }
                }
                write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (host, server)
    }

    #[test]
    fn live_preview_searches_older_history_for_the_latest_edit() {
        let (host, server) = older_server([("200 OK", ARCHIVED_MESSAGE)]);
        let mut s = init();
        demo(&mut s);
        if let Screen::Main { session, chats } = &mut s.screen {
            Arc::make_mut(session).chat_service = host.clone();
            chats.retain(|chat| chat.id == "a");
        }
        s.messages.as_mut().unwrap().as_mut().unwrap().older = Some(format!("{host}/older"));
        let (send, wake) = mpsc::channel();
        let waker: Waker = Arc::new(move || { let _ = send.send(()); });
        s.waker = Some(waker.clone());
        s.search_preview.set_waker(waker);
        let mut tx = Tx::default();
        s.search.replace_text("obsolete");
        preview_changed(&mut s, &mut tx);
        s.search.replace_text("archived");
        preview_changed(&mut s, &mut tx);
        assert!(s.search_preview.pending("archived"));
        assert!(!s.search_preview.pending("obsolete"));
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(poll(&mut s, &mut tx));
        assert!(!s.search_preview.pending("archived"));
        assert_eq!(s.search_preview.results("archived")[0].id, "older-match");
        assert!(s.search_preview.results("obsolete").is_empty());
        server.join().unwrap();
    }

    #[test]
    fn scoped_search_retries_older_history_without_replacing_the_transcript() {
        let (host, server) = older_server([("503 Service Unavailable", "{}"), ("200 OK", ARCHIVED_MESSAGE)]);
        let mut s = init();
        demo(&mut s);
        if let Screen::Main { session, .. } = &mut s.screen {
            Arc::make_mut(session).chat_service = host.clone();
        }
        let page = s.messages.as_mut().unwrap().as_mut().unwrap();
        page.older = Some(format!("{host}/older"));
        let original_count = page.msgs.len();

        let (send, wake) = mpsc::channel();
        let waker: Waker = Arc::new(move || { let _ = send.send(()); });
        s.waker = Some(waker.clone());
        s.message_search.set_waker(waker);
        let mut tx = Tx::default();
        open_scoped(&mut s, &mut tx);
        s.message_search.input.replace_text("outdated");
        scoped_input_changed(&mut s, &mut tx);
        assert!(s.message_search.live_deadline.is_some());
        assert_eq!(s.message_search.budget, 0);
        s.message_search.input.clear();
        scoped_input_changed(&mut s, &mut tx);
        assert!(!s.message_search.busy());
        s.message_search.input.replace_text("archived TEST");
        scoped_input_changed(&mut s, &mut tx);
        assert!(s.message_search.live_deadline.is_some());
        s.message_search.live_deadline = Some(Instant::now() - Duration::from_millis(1));
        assert!(poll(&mut s, &mut tx));
        let finish = |s: &mut State| {
            let until = Instant::now() + Duration::from_secs(5);
            while s.message_search.busy() {
                wake.recv_timeout(until.saturating_duration_since(Instant::now())).unwrap();
                poll(s, &mut Tx::default());
            }
        };
        finish(&mut s);
        assert!(s.message_search.has_failures());
        retry(&mut s);
        finish(&mut s);
        assert!(!s.message_search.has_failures());
        assert_eq!(s.message_search.hits.len(), 1);
        assert_eq!(s.message_search.hits[0].id, "older-match");
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len(), original_count);
        server.join().unwrap();
    }
}
