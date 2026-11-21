//! Date-filtered counts from direct and group chat history.
use crate::{Chat, ConversationSection, Msg, Screen, Session, State, teams};
use chrono::{DateTime, Days, Local, Months, NaiveDate};
use rune::{Latest, TextEdit, Tx, Waker};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod cache;
use cache::ChatCache;

const PARALLEL_CHATS: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Range {
    Day,
    #[default]
    Week,
    Month,
    SixMonths,
    AllTime,
    Custom,
}

impl Range {
    fn preset(self, today: NaiveDate) -> Option<Dates> {
        Some(match self {
            Self::Day => Dates::ending(today, 1),
            Self::Week => Dates::ending(today, 7),
            Self::Month => Dates::ending(today, 30),
            Self::SixMonths => Dates {
                from: today
                    .checked_sub_months(Months::new(6))
                    .unwrap_or(NaiveDate::MIN),
                to: today,
            },
            // The scanner can follow every older page without a date cutoff.
            Self::AllTime => Dates {
                from: NaiveDate::MIN,
                to: today,
            },
            Self::Custom => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Dates {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl Dates {
    pub fn ending(to: NaiveDate, days: u64) -> Self {
        Self {
            from: to - Days::new(days - 1),
            to,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Counts {
    pub sent: u64,
    pub total: u64,
}

impl Counts {
    fn record(&mut self, sent: bool) {
        self.total += 1;
        if sent {
            self.sent += 1;
        }
    }
}

#[derive(Default)]
struct Scan {
    days: DaysByChat,
    seen: HashSet<String>,
    links: HashSet<String>,
    next: Option<String>,
    newest_page_date: Option<NaiveDate>,
    cache: Option<ChatCache>,
}

impl Scan {
    /// Stop after a page wholly before the range. A mixed page can contain
    /// late arrivals with an older compose date.
    fn add(&mut self, messages: &[Msg], from: NaiveDate) -> bool {
        let mut newest = None;
        for message in messages {
            let Ok(time) = DateTime::parse_from_rfc3339(&message.time) else {
                continue;
            };
            let date = time.with_timezone(&Local).date_naive();
            newest = newest.max(Some(date));

            // Pages can overlap. Prefer the newest occurrence of a server message ID.
            let id = if message.server_id.is_empty() {
                &message.id
            } else {
                &message.server_id
            };
            if (!id.is_empty() && !self.seen.insert(id.clone())) || message.deleted {
                continue;
            }
            if date >= from {
                self.days.entry(date).or_default().record(message.mine);
            }
        }
        if newest.is_some() {
            self.newest_page_date = newest;
        }
        newest.is_some_and(|date| date < from)
    }

    fn misses(&self, dates: Dates) -> bool {
        self.newest_page_date.is_none_or(|date| date >= dates.from)
            && !self.cache.as_ref().is_some_and(|cache| cache.covers(dates))
    }

    fn merge_cached_counts(&self, days: &mut DaysByChat) {
        if let Some(cache) = &self.cache {
            cache.merge_counts(days);
        }
    }
}

type DaysByChat = HashMap<NaiveDate, Counts>;
type ChatResult = (String, ScannedChat);

enum ScannedChat {
    History(DaysByChat),
    Incomplete(Scan),
    Bot,
}

#[derive(Default)]
pub(crate) struct Insights {
    pub range: Range,
    pub dates: Option<Dates>,
    pub from: TextEdit,
    pub to: TextEdit,
    pub date_error: Option<&'static str>,
    pub range_menu: bool,
    pub graph: crate::ui::insights::Graph,
    pub selected: Option<String>,
    pub hovered: Option<String>,
    pub focused: Option<String>,
    pub suppress_press: bool,
    pub counts: HashMap<String, Counts>,
    pub sent: u64,

    // History covers every available day from this date through the latest service page.
    // Reuse it for narrower filters while Insights is open.
    since: Option<NaiveDate>,
    roster: HashSet<String>,
    bot_chats: HashSet<String>,
    history: HashMap<String, DaysByChat>,
    queued: VecDeque<String>,
    failed: HashMap<String, Scan>,
    worker: Latest<()>,
    results: Arc<Mutex<Vec<ChatResult>>>,
    cancelled: Arc<AtomicBool>,
    demo_today: Option<NaiveDate>,
}

impl Insights {
    pub fn today(&self) -> NaiveDate {
        self.demo_today.unwrap_or_else(|| Local::now().date_naive())
    }

    pub fn loading(&self) -> bool {
        self.worker.is_pending() || !self.queued.is_empty()
    }

    pub fn failed(&self) -> usize {
        self.failed
            .values()
            .filter(|scan| self.dates.is_some_and(|dates| scan.misses(dates)))
            .count()
    }

    pub fn unavailable(&self, id: &str) -> bool {
        self.failed
            .get(id)
            .is_some_and(|scan| self.dates.is_some_and(|dates| scan.misses(dates)))
    }

    pub fn has_partial_history(&self) -> bool {
        self.failed.keys().any(|id| {
            self.counts.get(id).is_some_and(|count| count.total > 0) && self.unavailable(id)
        })
    }

    pub(crate) fn includes_chat(&self, chat: &Chat) -> bool {
        eligible_chat(chat) && !self.bot_chats.contains(&chat.id)
    }

    fn preset_dates(&mut self) -> Option<Dates> {
        let dates = self.range.preset(self.today())?;
        if self.range != Range::AllTime {
            self.from.replace_text(dates.from.to_string());
            self.to.replace_text(dates.to.to_string());
        }
        Some(dates)
    }

    fn cancel(&mut self) {
        self.cancelled.store(true, Relaxed);
        self.worker.cancel();
        self.queued.clear();
        self.results = Arc::default();
    }

    fn summarize(&mut self) {
        self.counts.clear();
        self.sent = 0;
        let Some(dates) = self.dates else { return };
        for (id, days) in &self.history {
            let mut counts = Counts::default();
            for (&date, day) in days {
                if (dates.from..=dates.to).contains(&date) {
                    counts.total += day.total;
                    counts.sent += day.sent;
                }
            }
            self.sent += counts.sent;
            self.counts.insert(id.clone(), counts);
        }
        self.graph.set_counts(&self.counts);
        for id in [&mut self.selected, &mut self.hovered, &mut self.focused] {
            if id.as_ref().is_some_and(|id| !self.graph.contains(id)) {
                *id = None;
            }
        }
    }
}

impl Drop for Insights {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn eligible_chat(chat: &Chat) -> bool {
    !chat.channel
        && !chat.id.starts_with("28:")
        // A group can have a bot as its most recent sender.
        && !(chat.id.ends_with("@unq.gbl.spaces") && chat.avatar_id.starts_with("28:"))
}

fn chat_ids(chats: &[Chat]) -> impl Iterator<Item = &str> {
    chats
        .iter()
        .filter(|chat| eligible_chat(chat))
        .map(|chat| chat.id.as_str())
}

pub(crate) fn open(s: &mut State) {
    if let Some(dates) = s.insights.preset_dates() {
        s.insights.dates = Some(dates);
    }
    refresh(s);
}

pub(crate) fn demo(
    s: &mut State,
    today: NaiveDate,
    events: impl Iterator<Item = (&'static str, DateTime<Local>, bool)>,
) {
    s.insights.demo_today = Some(today);
    for (id, time, sent) in events {
        s.insights
            .history
            .entry(id.into())
            .or_default()
            .entry(time.date_naive())
            .or_default()
            .record(sent);
    }
    open(s);
}

pub(crate) fn select_range(s: &mut State, range: Range) {
    let state = &mut s.insights;
    state.range = range;
    state.range_menu = false;
    state.date_error = None;
    if let Some(dates) = state.preset_dates() {
        apply_dates(s, dates);
    }
}

pub(crate) fn apply_custom(s: &mut State) {
    let state = &mut s.insights;
    let parse = |edit: &TextEdit| {
        let value = edit.text.trim();
        let day = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
        (day.format("%Y-%m-%d").to_string() == value).then_some(day)
    };
    state.date_error = Some("Enter both dates as YYYY-MM-DD.");
    if let (Some(from), Some(to)) = (parse(&state.from), parse(&state.to)) {
        state.date_error = if from > to {
            Some("From must be on or before To.")
        } else if to > state.today() {
            Some("To must be today or earlier.")
        } else {
            None
        };
        if state.date_error.is_none() {
            apply_dates(s, Dates { from, to });
        }
    }
}

fn apply_dates(s: &mut State, dates: Dates) {
    s.insights.dates = Some(dates);
    if s.insights.since.is_some_and(|since| dates.from >= since) {
        s.insights.summarize();
    } else {
        refresh(s);
    }
}

pub(crate) fn refresh(s: &mut State) {
    let Screen::Main { session, chats } = &s.screen else {
        return;
    };
    let state = &mut s.insights;
    let Some(dates) = state.dates else { return };
    let ids: Vec<_> = chat_ids(chats).map(str::to_owned).collect();
    state.roster = ids.iter().cloned().collect();
    state.cancel();
    state.cancelled = Arc::default();
    state.since = Some(dates.from);
    state.failed.clear();

    if let Some(waker) = &s.waker
        && !session.chat_service.is_empty()
    {
        state.history.clear();
        state.worker.set_waker(waker.clone());
        state
            .queued
            .extend(ids.into_iter().filter(|id| !state.bot_chats.contains(id)));
        request_next(state, session, waker);
    } else {
        state.history.retain(|id, _| state.roster.contains(id));
    }
    state.summarize();
}

/// Refresh just this chat after its transcript changes, including changes from another device.
pub(crate) fn invalidate(s: &mut State, id: &str) {
    if s.conversation_section != ConversationSection::Analytics || s.waker.is_none() {
        return;
    }
    let Screen::Main { session, chats } = &s.screen else {
        return;
    };
    let state = &mut s.insights;
    if state.since.is_none()
        || session.chat_service.is_empty()
        || !chats
            .iter()
            .any(|chat| chat.id == id && state.includes_chat(chat))
    {
        return;
    }
    state.queued.retain(|pending| pending != id);
    state.queued.push_front(id.to_owned());
    state.failed.remove(id);
    if state.history.remove(id).is_some() {
        state.summarize();
    }
}

pub(crate) fn retry(s: &mut State) {
    let state = &mut s.insights;
    let retry: Vec<_> = state
        .failed
        .keys()
        .filter(|id| state.unavailable(id) && !state.queued.contains(id))
        .cloned()
        .collect();
    state.queued.extend(retry);
    if let Some(waker) = &s.waker {
        waker();
    }
}

fn request_next(state: &mut Insights, session: &Arc<Session>, waker: &Waker) {
    if state.worker.is_pending() || state.queued.is_empty() {
        return;
    }
    let count = PARALLEL_CHATS.min(state.queued.len());
    let requests: Vec<_> = state
        .queued
        .drain(..count)
        .map(|id| {
            let scan = state.failed.remove(&id);
            (id, scan)
        })
        .collect();
    let from = state.since.unwrap();
    let today = state.today();
    let session = Arc::clone(session);
    let cancelled = Arc::clone(&state.cancelled);
    let results = Arc::clone(&state.results);
    let waker = Arc::clone(waker);
    state.worker.request(requests, move |requests| {
        std::thread::scope(|scope| {
            for (id, scan) in requests {
                let (session, cancelled, results, waker) = (&session, &cancelled, &results, &waker);
                scope.spawn(move || {
                    let scan = scan.unwrap_or_else(|| Scan {
                        cache: ChatCache::open(&session.user.id, &id, today),
                        ..Scan::default()
                    });
                    if let Some(result) = scan_chat(session, &id, from, scan, cancelled)
                        && !cancelled.load(Relaxed)
                    {
                        results.lock().unwrap().push((id, result));
                        waker();
                    }
                });
            }
        });
    });
}

fn scan_chat(
    session: &Session,
    id: &str,
    from: NaiveDate,
    mut scan: Scan,
    cancelled: &AtomicBool,
) -> Option<ScannedChat> {
    let peer = teams::peer(id, &session.user.id);
    let from = scan
        .cache
        .as_ref()
        .map_or(from, |cache| cache.scan_from(from));
    while !cancelled.load(Relaxed) {
        let mut attempts = 0;
        let page = loop {
            let cursor = scan
                .next
                .as_deref()
                .map_or(teams::HistoryPage::First(id), teams::HistoryPage::Older);
            match teams::history_page(session, cursor) {
                Ok(page) => break page,
                Err(error) => {
                    let Some(delay) = error.retry_after.filter(|_| attempts < 2) else {
                        return Some(ScannedChat::Incomplete(scan));
                    };
                    let delay = delay.max(Duration::from_millis(250 * (1 << attempts)));
                    attempts += 1;
                    let start = Instant::now();
                    while let Some(remaining) = delay.checked_sub(start.elapsed()) {
                        if cancelled.load(Relaxed) {
                            return None;
                        }
                        std::thread::sleep(remaining.min(Duration::from_millis(50)));
                    }
                    if cancelled.load(Relaxed) {
                        return None;
                    }
                }
            }
        };
        if peer.is_some_and(|peer| {
            page.msgs.iter().any(|message| {
                message
                    .sender_id
                    .strip_prefix("28:")
                    .is_some_and(|bot| bot.eq_ignore_ascii_case(peer))
            })
        }) {
            return Some(ScannedChat::Bot);
        }
        let reached_start = scan.add(&page.msgs, from);
        let Some(older) = page.older.filter(|_| !reached_start) else {
            if cancelled.load(Relaxed) {
                return None;
            }
            if let Some(cache) = &scan.cache {
                cache.remember(from, &scan.days);
                cache.merge_counts(&mut scan.days);
            }
            return Some(ScannedChat::History(scan.days));
        };
        if !scan.links.insert(older.clone()) {
            return Some(ScannedChat::Incomplete(scan));
        }
        scan.next = Some(older);
    }
    None
}

pub(crate) fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    let state = &mut s.insights;
    if s.conversation_section != ConversationSection::Analytics {
        if state.since.take().is_some() {
            state.cancel();
        }
        state.range_menu = false;
        state.hovered = None;
        state.focused = None;
        state.suppress_press = false;
        state.graph.stop_drag();
        return false;
    }
    state.suppress_press = false;
    let focus = tx
        .focused()
        .filter(|_| tx.focus_visible())
        .and_then(|key| key.strip_prefix("insights-node/"))
        .filter(|id| state.graph.contains(id));
    let focus_changed = state.focused.as_deref() != focus;
    if focus_changed {
        state.focused = focus.map(str::to_owned);
        if let Some(id) = focus {
            state.graph.reveal(id);
        }
    }
    let mut changed = focus_changed;
    if let Screen::Main { session, chats } = &s.screen {
        // Live events can add chats or remove them while this view is open.
        let ids: HashSet<_> = chat_ids(chats).collect();
        let roster_changed = state.roster.len() != ids.len()
            || !state.roster.iter().all(|id| ids.contains(id.as_str()));
        if roster_changed {
            state.queued.retain(|id| ids.contains(id.as_str()));
            state.history.retain(|id, _| ids.contains(id.as_str()));
            state.failed.retain(|id, _| ids.contains(id.as_str()));
            if state.since.is_some() && s.waker.is_some() && !session.chat_service.is_empty() {
                state.queued.extend(
                    chat_ids(chats)
                        .filter(|id| !state.roster.contains(*id) && !state.bot_chats.contains(*id))
                        .map(str::to_owned),
                );
            }
            state.roster = ids.iter().map(|&id| id.to_owned()).collect();
        }
        // Capture completion before draining results so a new batch cannot start
        // while an unprocessed result from the previous batch is still arriving.
        let batch_finished = !state.worker.is_pending();
        let completion = batch_finished && state.worker.take().is_some();
        let results = std::mem::take(&mut *state.results.lock().unwrap());
        if roster_changed || !results.is_empty() {
            for (id, result) in results {
                if !ids.contains(id.as_str()) {
                    continue;
                }
                match result {
                    ScannedChat::Bot => {
                        state.queued.retain(|queued| *queued != id);
                        state.history.remove(&id);
                        state.failed.remove(&id);
                        state.bot_chats.insert(id);
                    }
                    // An update during the request already queued fresher history for this chat.
                    _ if state.queued.contains(&id) => {}
                    ScannedChat::History(days) => {
                        state.history.insert(id, days);
                    }
                    ScannedChat::Incomplete(scan) => {
                        let mut days = scan.days.clone();
                        scan.merge_cached_counts(&mut days);
                        if !days.is_empty() {
                            state.history.insert(id.clone(), days);
                        }
                        state.failed.insert(id, scan);
                    }
                }
            }
            state.summarize();
            changed = true;
        }
        changed |= completion;
        if batch_finished && let Some(waker) = &s.waker {
            request_next(state, session, waker);
        }
    }
    state.graph.tick(now, tx) || changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn six_month_preset_uses_calendar_months() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
        assert_eq!(
            Range::SixMonths.preset(today),
            Some(Dates {
                from: NaiveDate::from_ymd_opt(2026, 2, 28).unwrap(),
                to: today,
            })
        );
    }

    #[test]
    fn only_top_thirty_chats_are_visible_but_all_sent_messages_are_counted() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let previous = day.pred_opt().unwrap();
        let mut state = Insights::default();
        state.dates = Some(Dates { from: day, to: day });
        for index in 0..=32 {
            let sent = match index {
                0 | 1 => 0,
                2 => 2,
                _ => index - 1,
            };
            let total = match index {
                1 => 1_000,
                3 => sent + 1,
                _ => sent,
            };
            state
                .history
                .entry(index.to_string())
                .or_default()
                .insert(day, Counts { sent, total });
        }
        state.history.get_mut("2").unwrap().insert(
            previous,
            Counts {
                sent: 100,
                total: 100,
            },
        );

        state.summarize();
        assert!((3..=32).all(|index| state.graph.contains(&index.to_string())));
        assert!((0..=2).all(|index| !state.graph.contains(&index.to_string())));
        assert_eq!(state.sent, 497);

        state.selected = Some("3".into());
        state.hovered = state.selected.clone();
        state.focused = state.selected.clone();
        state.dates = Some(Dates {
            from: previous,
            to: day,
        });
        state.summarize();
        assert!(state.graph.contains("2"));
        assert!(!state.graph.contains("3"));
        assert!(state.selected.is_none() && state.hovered.is_none() && state.focused.is_none());
        assert_eq!(state.sent, 597);
    }

    #[test]
    fn history_pages_respect_local_dates_and_deduplicate_messages() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 14).unwrap();
        let timestamp = |date: NaiveDate, minute| {
            Local
                .from_local_datetime(&date.and_hms_opt(12, minute, 0).unwrap())
                .single()
                .unwrap()
                .to_rfc3339()
        };
        let message = move |id, date, minute, from| {
            json!({
                "id": id,
                "messagetype": "Text",
                "content": "message",
                "composetime": timestamp(date, minute),
                "from": from
            })
        };
        let mine = "8:orgid:me";
        let other = "8:orgid:other";
        let previous = day.pred_opt().unwrap();
        let oldest = day - Days::new(500);
        let next = day.succ_opt().unwrap();
        let mut duplicate = message("4", day, 9, mine);
        duplicate["clientmessageid"] = json!("shared");
        let mut second = message("3", day, 17, mine);
        second["clientmessageid"] = json!("shared");
        let mut deleted = message("5", day, 40, mine);
        deleted["deletetime"] = json!(1);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let session = Session {
            user: crate::auth::User {
                id: "me".into(),
                ..Default::default()
            },
            chat_service: address.clone(),
            ..Default::default()
        };
        let server = std::thread::spawn(move || {
            let pages = [
                (
                    "/v1/users/ME/conversations/chat/messages?",
                    json!({
                        "messages": [message("6", next, 15, mine), deleted, duplicate],
                        "_metadata": { "backwardLink": format!("{address}/older") }
                    }),
                ),
                (
                    "/older ",
                    json!({
                        "messages": [duplicate, second, message("2", day, 20, other), message("1", previous, 30, mine)],
                        "_metadata": { "backwardLink": format!("{address}/before") }
                    }),
                ),
                (
                    "/before ",
                    json!({
                        "messages": [message("7", day, 40, mine), message("0", previous, 10, mine)],
                        "_metadata": { "backwardLink": format!("{address}/cutoff") }
                    }),
                ),
                (
                    "/cutoff ",
                    json!({
                        "messages": [message("8", previous, 10, other)],
                        "_metadata": { "backwardLink": format!("{address}/too-old") }
                    }),
                ),
            ];
            for (path, page) in pages.clone().into_iter().chain(pages) {
                respond(&listener, path, page);
            }
            respond(
                &listener,
                "/too-old ",
                json!({"messages": [message("9", oldest, 0, mine)]}),
            );
        });
        let ScannedChat::History(history) = scan_chat(
            &session,
            "chat",
            day,
            Scan::default(),
            &AtomicBool::new(false),
        )
        .unwrap() else {
            panic!("expected chat history");
        };
        let from = Range::AllTime.preset(day).unwrap().from;
        let ScannedChat::History(all_time) = scan_chat(
            &session,
            "chat",
            from,
            Scan::default(),
            &AtomicBool::new(false),
        )
        .unwrap() else {
            panic!("expected all-time chat history");
        };
        server.join().unwrap();
        assert_eq!(all_time[&oldest].sent, 1);
        assert!(!history.contains_key(&previous));
        assert_eq!(history[&day].total, 4);
        assert_eq!(history[&day].sent, 3);

        let mut s = crate::init();
        s.insights.dates = Some(Dates { from: day, to: day });
        s.insights.history.insert("chat".into(), history);
        s.insights
            .history
            .entry("another chat".into())
            .or_default()
            .insert(day, Counts { sent: 1, total: 1 });
        s.insights.summarize();
        assert_eq!(s.insights.sent, 4);
    }

    #[test]
    fn all_time_reuses_persisted_closed_days_and_refreshes_today() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let yesterday = today.pred_opt().unwrap();
        let old_day = today - Days::new(500);
        let message = move |id, day: NaiveDate| {
            let time = Local
                .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
                .single()
                .unwrap();
            json!({
                "id": id, "messagetype": "Text", "content": "private body",
                "from": "8:orgid:me", "composetime": time.to_rfc3339()
            })
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let session = Session {
            user: crate::auth::User {
                id: "me".into(),
                ..Default::default()
            },
            chat_service: address.clone(),
            ..Default::default()
        };
        let server = std::thread::spawn(move || {
            let first = "/v1/users/ME/conversations/chat/messages?";
            respond(
                &listener,
                first,
                json!({
                    "messages": [message("3", today)],
                    "_metadata": { "backwardLink": format!("{address}/older") }
                }),
            );
            respond(
                &listener,
                "/older ",
                json!({
                    "messages": [message("2", yesterday), message("1", old_day)]
                }),
            );
            respond(
                &listener,
                first,
                json!({
                    "messages": [message("4", today), message("3", today)],
                    "_metadata": { "backwardLink": format!("{address}/boundary") }
                }),
            );
            // The old message has disappeared from Teams since the first scan.
            respond(
                &listener,
                "/boundary ",
                json!({
                    "messages": [message("5", yesterday)],
                    "_metadata": { "backwardLink": format!("{address}/must-not-read") }
                }),
            );
        });
        let directory =
            std::env::temp_dir().join(format!("teams-insights-{}", uuid::Uuid::new_v4()));
        let file = directory.join("counts.json");
        let scan = || {
            let scan = Scan {
                cache: ChatCache::at(file.clone(), "chat", today),
                ..Scan::default()
            };
            let ScannedChat::History(days) = scan_chat(
                &session,
                "chat",
                NaiveDate::MIN,
                scan,
                &AtomicBool::new(false),
            )
            .unwrap() else {
                panic!("expected complete history")
            };
            days
        };
        let first = scan();
        assert_eq!(first[&today].sent, 1);
        let stored = std::fs::read_to_string(&file).unwrap();
        let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
        let counts = stored["counts"].as_object().unwrap();
        assert_eq!(counts.len(), 2);
        assert!(
            counts
                .iter()
                .all(|(key, value)| key.starts_with("chat:chat:")
                    && value == &json!({"sent_total": 1, "total": 1}))
        );
        let second = scan();
        assert_eq!(second[&today].sent, 2);
        assert_eq!(second[&yesterday].sent, 1);
        assert_eq!(second[&old_day].sent, 1);
        server.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn respond(listener: &TcpListener, path: &str, page: serde_json::Value) {
        let (stream, request) = read_request(listener);
        assert!(request.starts_with(&format!("GET {path}")));
        send_page(stream, page);
    }

    fn read_request(listener: &TcpListener) -> (std::net::TcpStream, String) {
        let (stream, _) = listener.accept().unwrap();
        let mut input = BufReader::new(&stream);
        let mut request = String::new();
        input.read_line(&mut request).unwrap();
        let mut line = String::new();
        while line != "\r\n" {
            line.clear();
            if input.read_line(&mut line).unwrap() == 0 {
                break;
            }
        }
        (stream, request)
    }

    fn send_page(stream: std::net::TcpStream, page: serde_json::Value) {
        send_response(stream, "200 OK", "", page);
    }

    fn send_response(
        mut stream: std::net::TcpStream,
        status: &str,
        headers: &str,
        page: serde_json::Value,
    ) {
        let body = page.to_string();
        write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len()).unwrap();
    }

    fn poll_until(s: &mut State, done: impl Fn(&Insights) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            poll(s, 0.0, &mut Tx::default());
            if done(&s.insights) {
                break;
            }
            assert!(Instant::now() < deadline, "history refresh timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn all_time_keeps_counts_and_resumes_after_an_older_page_fails() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let timestamp = |date: NaiveDate| {
            Local
                .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
                .single()
                .unwrap()
                .to_rfc3339()
        };
        let message = move |id, date| {
            json!({
                "id": id, "messagetype": "Text", "content": "message",
                "from": "8:orgid:me", "composetime": timestamp(date)
            })
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let session = Arc::new(Session {
            user: crate::auth::User {
                id: "me".into(),
                ..Default::default()
            },
            chat_service: address.clone(),
            ..Default::default()
        });
        let chats = teams::parse_chats(&json!({"conversations":[{"id":"chat"}]}), &session.user);
        let mut s = crate::init();
        s.screen = Screen::Main { session, chats };
        s.waker = Some(Arc::new(|| {}));
        s.conversation_section = ConversationSection::Analytics;
        s.insights.demo_today = Some(day);
        s.insights.range = Range::AllTime;

        let server = std::thread::spawn(move || {
            let older = message("2", day - Days::new(30));
            respond(
                &listener,
                "/v1/users/ME/conversations/chat/messages?",
                json!({
                    "messages": [message("3", day)],
                    "_metadata": { "backwardLink": format!("{address}/older") }
                }),
            );
            let (stream, request) = read_request(&listener);
            assert!(request.starts_with("GET /older "));
            send_response(
                stream,
                "429 Too Many Requests",
                "Retry-After: 0\r\n",
                json!({}),
            );
            respond(
                &listener,
                "/older ",
                json!({
                    "messages": [older],
                    "_metadata": { "backwardLink": format!("{address}/tail") }
                }),
            );
            let (stream, request) = read_request(&listener);
            assert!(request.starts_with("GET /tail "));
            send_response(stream, "404 Not Found", "", json!({}));
            respond(
                &listener,
                "/tail ",
                json!({
                    "messages": [older, message("1", day - Days::new(60))],
                    "_metadata": { "backwardLink": " " }
                }),
            );
        });

        open(&mut s);
        poll_until(&mut s, |state| !state.loading() && state.failed() == 1);
        assert_eq!(s.insights.sent, 2);
        assert!(s.insights.graph.contains("chat"));
        assert!(s.insights.has_partial_history());

        select_range(&mut s, Range::Week);
        assert_eq!(s.insights.sent, 1);
        assert_eq!(s.insights.failed(), 0);
        select_range(&mut s, Range::AllTime);
        assert_eq!(s.insights.failed(), 1);
        retry(&mut s);
        poll_until(&mut s, |state| !state.loading() && state.sent == 3);
        assert_eq!(s.insights.failed(), 0);
        assert_eq!(s.insights.counts["chat"].total, 3);
        server.join().unwrap();
    }

    #[test]
    fn a_new_outbound_push_refreshes_an_open_graphs_zero_count() {
        const BOT: &str = "19:me_github@unq.gbl.spaces";
        let day = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let time = Local
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .to_rfc3339();
        let page = json!({ "messages": [{
            "id": "1", "messagetype": "Text", "content": "message",
            "from": "8:orgid:me", "composetime": time
        }] });
        let bot_page = json!({ "messages": [page["messages"][0].clone(), {
            "id": "2", "messagetype": "Text", "content": "Notification",
            "from": "28:github", "composetime": time
        }] });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let session = Arc::new(Session {
            user: crate::auth::User {
                id: "me".into(),
                ..Default::default()
            },
            chat_service: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        });
        let mut s = crate::init();
        let chats = teams::parse_chats(
            &json!({"conversations":[{"id":"chat"},{"id":"active"},{"id":BOT}]}),
            &session.user,
        );
        let mut chat = chats.iter().find(|chat| chat.id == "chat").unwrap().clone();
        let message = teams::parse_messages(&page, &session.user.id)
            .pop()
            .unwrap();
        chat.time.clone_from(&message.time);
        s.screen = Screen::Main { session, chats };
        s.waker = Some(Arc::new(|| {}));
        s.conversation_section = ConversationSection::Analytics;
        s.insights.demo_today = Some(day);

        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let path = "/v1/users/ME/conversations/chat/messages?";
            let mut slow = None;
            for _ in 0..3 {
                let (stream, request) = read_request(&listener);
                if request.starts_with(&format!("GET {path}")) {
                    slow = Some(stream);
                } else if request.starts_with(
                    "GET /v1/users/ME/conversations/19%3Ame_github%40unq.gbl.spaces/messages?",
                ) {
                    send_page(stream, bot_page.clone());
                } else {
                    assert!(request.starts_with("GET /v1/users/ME/conversations/active/messages?"));
                    send_page(stream, page.clone());
                }
            }
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            send_page(slow.unwrap(), json!({"messages":[]}));
            respond(&listener, path, page);
        });
        open(&mut s);
        assert!(s.insights.counts.is_empty());
        poll_until(&mut s, |state| {
            state.graph.contains("active") && state.bot_chats.contains(BOT)
        });
        assert!(s.insights.loading());
        assert!(!s.insights.counts.contains_key("chat"));
        assert!(!s.insights.counts.contains_key(BOT));
        assert_eq!(s.insights.sent, 1);
        release.send(()).unwrap();
        poll_until(&mut s, |state| {
            !state.loading() && state.counts.contains_key("chat")
        });
        assert_eq!(s.insights.counts["chat"].total, 0);

        s.updates
            .0
            .send((
                0,
                teams::events::Update::Message {
                    chat,
                    message: Some(message),
                    server_id: "1".into(),
                    new: true,
                },
            ))
            .unwrap();
        crate::notifications::poll(&mut s);
        assert!(!s.insights.counts.contains_key("chat"));
        poll_until(&mut s, |state| {
            !state.loading() && state.graph.contains("chat")
        });
        assert_eq!(s.insights.counts["chat"].sent, 1);
        assert_eq!(s.insights.counts["chat"].total, 1);
        server.join().unwrap();
    }
}
