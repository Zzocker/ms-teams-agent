//! A Teams desktop client: device-code sign-in that lasts across launches, then your chats.

mod auth;
mod teams;

use auth::Session;
use rune::tok::{self, type_};
use rune::{Align, AppSpec, Cx, El, GrowthSpec, Side, Drag, Latest, Live, Phase, ScrollPos, ScrollSpec, TextEdit, Tx, Waker, auto_grow, col, el, row, text, text_input};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use teams::{Chat, Msg, Page};

enum Screen {
    Loading,
    /// Signed out, with why the last attempt failed.
    Login(Option<String>),
    Code { user_code: String, url: String },
    Main { session: Arc<Session>, chats: Vec<Chat> },
}

type Messages = Result<Page, String>;

struct State {
    screen: Screen,
    panel_width: f32,
    reply_target: Option<String>,
    reply_error: Option<String>,
    reply_highlight: Option<(String, std::time::Instant)>,
    reply_highlight_on: bool,
    show_teams: bool,
    teams: Option<Result<Vec<teams::Team>, String>>,
    teams_loader: Latest<Result<Vec<teams::Team>, String>>,
    collapsed_teams: std::collections::HashSet<String>,
    /// Bumped on every sign-in attempt, cancel and sign-out; a worker from an older one is ignored.
    attempt: Arc<AtomicU64>,
    events: (mpsc::Sender<(u64, Screen)>, mpsc::Receiver<(u64, Screen)>),
    waker: Option<Waker>,
    selected: Option<String>,
    /// The selected chat's messages; `None` while they load.
    messages: Option<Messages>,
    loader: Latest<(String, Messages)>,
    /// A fetch of the page before the loaded ones: the chat, the link it followed, the page.
    older_loader: Latest<(String, String, Messages)>,
    /// The message being written to the selected chat.
    draft: TextEdit,
    /// A send in flight: the chat, the message's id, then its reloaded messages, or the error and the unsent text.
    sender: Latest<(String, String, Result<Messages, (String, String)>)>,
    send_error: Option<String>,
    /// Filters the chat list by name.
    search: TextEdit,
    /// Where the chat list's and the messages' scroll thumbs are drawn.
    chat_bar: Bar,
    msg_bar: Bar,
}

fn init() -> State {
    State {
        screen: Screen::Loading,
        panel_width: 300.0,
        reply_target: None,
        reply_error: None,
        reply_highlight: None,
        reply_highlight_on: false,
        show_teams: false,
        teams: None,
        teams_loader: Latest::default(),
        collapsed_teams: Default::default(),
        attempt: Arc::default(),
        events: mpsc::channel(),
        waker: None,
        selected: None,
        messages: None,
        loader: Latest::default(),
        older_loader: Latest::default(),
        draft: TextEdit::new(""),
        sender: Latest::default(),
        send_error: None,
        search: TextEdit::new(""),
        chat_bar: Bar::default(),
        msg_bar: Bar::default(),
    }
}

/// Signs in off the UI thread: from the stored session, else (when `interactive`) with a device
/// code; then loads the chat list.
fn sign_in(s: &mut State, interactive: bool) {
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

fn sign_out(s: &mut State) {
    s.reply_highlight = None;
    s.reply_target = None;
    s.reply_error = None;
    s.attempt.fetch_add(1, SeqCst);
    s.teams_loader.cancel();
    s.teams = None;
    s.show_teams = false;
    s.collapsed_teams.clear();
    s.loader.cancel();
    s.sender.cancel();
    s.draft.clear();
    s.search.clear();
    s.send_error = None;
    auth::sign_out();
    (s.selected, s.messages) = (None, None);
    s.screen = Screen::Login(None);
}

fn navigate(s: &mut State, teams: bool) {
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
    s.send_error = None;
    if teams && s.teams.is_none() && !s.teams_loader.is_pending() {
        let Screen::Main { session, .. } = &s.screen else { return };
        let session = session.clone();
        s.teams_loader.request((), move |_| teams::teams(&session));
    }
}

fn open_chat(s: &mut State, id: String) {
    s.reply_highlight = None;
    s.reply_target = None;
    s.reply_error = None;
    let Screen::Main { session, .. } = &s.screen else { return };
    let session = session.clone();
    if s.selected.as_ref() != Some(&id) {
        // A draft belongs to the chat it was written in; it must not follow to another one.
        s.draft.clear();
        s.send_error = None;
    }
    (s.selected, s.messages) = (Some(id.clone()), None);
    s.loader.request(id, move |id| {
        let r = teams::messages(&session, &id);
        (id, r)
    });
}

/// Reveal the reply target, following older pages until it is found or history runs out.
fn reveal_reply(s: &mut State, tx: &mut Tx) -> bool {
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
fn load_older(s: &mut State) {
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

/// Shows the draft in the selected chat at once and sends it, then reloads its messages.
fn send(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    let text = s.draft.text.trim().to_string();
    // One send at a time: `Latest` would drop the first one's answer.
    if text.is_empty() || s.sender.is_pending() {
        return;
    }
    let session = session.clone();
    let id = id.clone();
    let msg_id = teams::new_msg_id();
    if let Some(Ok(page)) = &mut s.messages {
        // In this input's commit, so it glides in; the reload's copy shares its id and key.
        page.msgs.push(Msg { id: msg_id.clone(), server_id: String::new(), sender: session.user.name.clone(), text: text.clone(), time: teams::now(), mine: true, reply: None });
    }
    s.draft.clear();
    s.send_error = None;
    s.sender.request((id, msg_id, text), move |(id, msg_id, text)| {
        let r = match teams::send(&session, &id, &msg_id, &text) {
            Ok(()) => Ok(teams::messages(&session, &id)),
            Err(e) => Err((e, text)),
        };
        (id, msg_id, r)
    });
}

fn setup(s: &mut State, waker: Waker) {
    s.teams_loader.set_waker(waker.clone());
    s.loader.set_waker(waker.clone());
    s.sender.set_waker(waker.clone());
    s.older_loader.set_waker(waker.clone());
    s.waker = Some(waker);
    // The app has no arguments of its own; any argument is Rune's headless tooling, which gets no network.
    if std::env::args().len() == 1 {
        sign_in(s, false);
    }
}

fn reply_blink(elapsed: f32) -> Option<bool> {
    (elapsed < 1.8).then(|| (elapsed / 0.3) as u32 % 2 == 0)
}

fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some((_, started)) = &s.reply_highlight {
        if let Some(on) = reply_blink(started.elapsed().as_secs_f32()) {
            changed = s.reply_highlight_on != on;
            s.reply_highlight_on = on;
            tx.wake_after(std::time::Duration::from_millis(150));
        } else {
            s.reply_highlight = None;
            s.reply_highlight_on = false;
            changed = true;
        }
    }
    while let Ok((attempt, screen)) = s.events.1.try_recv() {
        if attempt != s.attempt.load(SeqCst) {
            continue;
        }
        if let Screen::Code { url, .. } = &screen {
            let _ = tx.open_url(url); // the URL is also on screen if no browser opens
        }
        s.screen = screen;
        changed = true;
    }
    if let Some(result) = s.teams_loader.take() {
        s.teams = Some(result);
        changed = true;
    }
    if let Some((id, r)) = s.loader.take() {
        if s.selected.as_ref() == Some(&id) {
            s.messages = Some(r);
            changed = true;
        }
    }
    if let Some((id, link, r)) = s.older_loader.take() {
        if s.selected.as_ref() == Some(&id) && s.reply_target.is_some() {
            if let Err(e) = &r {
                s.reply_target = None;
                s.reply_error = Some(format!("Could not load the quoted message: {e}"));
                changed = true;
            }
        }
        // Only onto the page it was asked for: a reload meanwhile has its own cursor.
        if let (true, Some(Ok(page)), Ok(old)) = (s.selected.as_ref() == Some(&id), s.messages.as_mut(), r) {
            if page.older.as_deref() == Some(&link) {
                // The anchor lets go mid-scroll and at the very top, which is when pages land; pin the old first row.
                if let Some(first) = page.msgs.first() {
                    tx.keep_on_screen(format!("msg/{}", first.id));
                }
                page.msgs.splice(0..0, old.msgs);
                page.older = old.older.filter(|next| next != &link);
                changed = true;
            }
        }
    }
    if let Some((id, msg_id, r)) = s.sender.take() {
        let here = s.selected.as_ref() == Some(&id);
        match r {
            Ok(Ok(fresh)) if here => {
                s.messages = Some(Ok(match s.messages.take() {
                    Some(Ok(page)) => teams::merge(page, fresh),
                    _ => fresh,
                }));
            }
            Ok(fresh) if here => s.messages = Some(fresh),
            Ok(_) => {}
            Err((e, text)) => {
                if let (true, Some(Ok(page))) = (here, &mut s.messages) {
                    page.msgs.retain(|m| m.id != msg_id);
                }
                s.send_error = Some(format!("Not sent: {e}"));
                // Give the text back, unless the user moved on to another chat or draft.
                if here && s.draft.text.is_empty() {
                    s.draft.replace_text(text);
                }
            }
        }
        changed = true;
    }
    let revealed = reveal_reply(s, tx);
    changed || revealed
}

/// A scroll bar's thumb, where it's drawn, and where its viewport was last scrolled to, for dragging.
#[derive(Clone)]
struct Bar {
    rect: Live<[f32; 4]>,
    at: Rc<Cell<ScrollPos>>,
}

impl Default for Bar {
    fn default() -> Self {
        Bar { rect: Live::new([0.0; 4]), at: Rc::default() }
    }
}

/// The scroll viewport keyed `key`, with a draggable bar over its right edge: Rune at this
/// revision draws no scroll bars. `list` must grow to fill its parent, as a scrolling column does.
fn with_bar(key: &'static str, list: El, bar: &Bar) -> El {
    let b = bar.clone();
    let list = list.key(key).on_scroll_offset(move |at: ScrollPos| {
        b.at.set(at);
        b.rect.snap(thumb(at));
    });
    let at = bar.at.clone();
    // The track, not the thumb, takes the drag: wider to hit, and a press off the thumb jumps there.
    let track = el().pin(None, Some(0.0), Some(0.0), Some(0.0)).w(12.0)
        .on_drag_any(move |d: &mut Drag, tx: &mut Tx| {
            let now = at.get();
            let [_, top, _, h] = thumb(now);
            if h == 0.0 {
                return;
            }
            if d.phase == Phase::Down {
                // Hold the thumb where it was grabbed; off it, grab its middle.
                d.data = if (top..top + h).contains(&d.y) { d.y - top } else { h / 2.0 };
            }
            tx.scroll_to_y(key, offset_at(now, d.y - d.data));
        });
    col().w_full().grow(1.0).min_h(0.0)
        .child(list)
        .child(el().absolute(0.0, 0.0).radius(2.0).fill(tok::theme(true).text_secondary.alpha(0.5)).live_rect(&bar.rect))
        .child(track)
}

/// The scroll offset that puts the thumb's top at `top`; what [`thumb`] undoes.
fn offset_at(at: ScrollPos, top: f32) -> f32 {
    let [_, _, _, h] = thumb(at);
    let room = at.viewport[1] - 4.0 - h;
    if room <= 0.0 { 0.0 } else { ((top - 2.0) / room).clamp(0.0, 1.0) * at.max[1] }
}

/// The thumb's `[x, y, w, h]` in the viewport: as tall as the share of the content in view, at least 24pt; none when nothing scrolls.
fn thumb(at: ScrollPos) -> [f32; 4] {
    let ([w, vh], max, y) = (at.viewport, at.max[1], at.offset[1]);
    if max <= 0.5 {
        return [0.0; 4];
    }
    let track = vh - 4.0;
    let h = (track * vh / (vh + max)).clamp(24.0_f32.min(track), track);
    [w - 6.0, 2.0 + (y / max).clamp(0.0, 1.0) * (track - h), 4.0, h]
}

fn button(key: &'static str, label: &'static str, primary: bool, on_press: fn(&mut State)) -> El {
    let t = tok::theme(true);
    el().key(key).role("button").label(label).focusable(true).px(16.0).py(8.0).radius(8.0)
        .fill(if primary { tok::accent(0, true) } else { t.control_off })
        .on_press(on_press)
        .child(text(label).font(type_::TITLE).color(t.text))
}

/// `2026-11-14T10:23:45.123Z` → `11-14 10:23`.
// shortcut: shown in UTC; convert to local time when there's a time-zone source.
fn short_time(iso: &str) -> String {
    iso.get(5..16).unwrap_or(iso).replace('T', " ")
}

fn day(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

/// A circle with the name's initials, coloured by the name so a person keeps their colour.
fn avatar(name: &str, size: f32) -> El {
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect();
    let hue = name.bytes().map(usize::from).sum();
    el().size(size, size).radius(size / 2.0).center().fill(tok::accent(hue, true))
        .child(text(initials.as_str()).font(type_::LABEL).color(tok::theme(true).bg))
}

fn loading_messages() -> El {
    static RING: rune::Icon = rune::Icon {
        name: "message-loader",
        view: 16.0,
        body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2" fill="none" opacity="0.2"/><path d="M8 2A6 6 0 0 1 13.258 10.89" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none"/>"#,
    };
    rune::icon(&RING, 24.0).key("message-loader")
        .role("progressbar").label("Loading messages")
        .color(tok::theme(true).text_secondary).looping(rune::LoopSpec::rotation(0.9))
}

fn main_view(s: &State, session: &Session, chats: &[Chat]) -> El {
    let t = tok::theme(true);
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary);

    let me = col().gap(2.0).grow(1.0).min_w(0.0)
        .child(text(session.user.name.as_str()).font(type_::TITLE).color(t.text).truncate())
        .child(note(&session.user.email).truncate());
    let header = row().w_full().gap(10.0).padding(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(avatar(&session.user.name, 36.0)).child(me)
        .child(el().key("sign-out").role("button").label("Sign out").focusable(true).px(10.0).py(6.0).radius(6.0)
            .fill(t.control_off).hover(t.hover).on_press(sign_out)
            .child(text("Sign out").font(type_::LABEL).color(t.text)));
    let chat_row = |c: &Chat, key: &str| {
        let id = c.id.clone();
        let picked = s.selected.as_ref() == Some(&c.id);
        row().key(format!("{key}/{}", c.id)).role("button").label(c.title.as_str()).focusable(true)
            .w_full().gap(10.0).px(if s.show_teams { 30.0 } else { 10.0 }).py(8.0).radius(8.0).items_center()
            .fill(if picked { t.press } else { t.card }).hover(t.hover)
            .on_press(move |s: &mut State| {
                s.search.clear();
                open_chat(s, id.clone());
            })
            .children((!s.show_teams).then(|| avatar(&c.title, 36.0)))
            .child(col().gap(2.0).grow(1.0).min_w(0.0)
                .child(row().w_full().gap(8.0)
                    .child(text(c.title.as_str()).font(type_::TITLE).color(t.text).truncate().grow(1.0).min_w(0.0))
                    .children((!c.time.is_empty()).then(|| note(&short_time(&c.time)).font(type_::LABEL))))
                .children((!c.preview.is_empty()).then(|| note(&c.preview).truncate())))
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
                        .focusable(true).w_full().gap(8.0).padding(10.0).items_center().hover(t.hover)
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
            .children(hits.is_empty().then(|| note("No matches.").px(10.0).py(8.0)))
            .child(col().w_full().gap(2.0).children(hits).scroll(ScrollSpec { stick_bottom: false }).shrink().min_h(0.0))
    });
    // Top bar, not the sidebar: search will cover messages too, not just chat names.
    let search = row().w_full().justify_center().px(16.0).py(8.0).fill(t.card).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(col().key("search-box").w_full().max_w(560.0).shrink().px(12.0).py(6.0).radius(8.0).fill(t.bg)
            .child(text_input(&s.search, type_::BODY).key("search").label("Search").color(t.text)
                .placeholder("Search", t.text_secondary)
                .caret(t.text, tok::accent(0, true).alpha(0.3))
                .on_edit(|s: &mut State| &mut s.search)));
    let list = with_bar("chats", col().w_full().gap(2.0).padding(6.0).children(rows)
        .scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0), &s.chat_bar);
    let sidebar = col().key("sidebar").w(s.panel_width).min_w(0.0).h_full().min_h(0.0)
        .motion(rune::Spring::NONE).clip_rounded(0.0).fill(t.card)
        .child(header)
        .child(text(if s.show_teams { "Teams" } else { "Chat" }).font(type_::TITLE).color(t.text).padding(12.0))
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
            true
        })
        .on_drag(|s: &mut State, d: &mut Drag, _| {
            if d.phase == Phase::Down { d.data = s.panel_width; }
            s.panel_width = if d.cancelled() { d.data } else { panel_width(d.data, d.dx) };
        })
        .child(el().w(1.0).h_full().fill(t.text_secondary));
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
                .fill(if s.show_teams == teams { t.press } else { t.bg }).hover(t.hover)
                .on_press(move |s: &mut State| if s.show_teams != teams { navigate(s, teams) })
                .child(rune::icon(icon, 24.0).color(t.text))
                .child(text(label).font(type_::LABEL).color(t.text))
        }));

    let pane = col().grow(1.0).shrink().h_full().min_w(0.0).min_h(0.0).fill(t.bg);
    let pane = match (&s.selected, &s.messages) {
        (None, _) => pane.center().child(note(if s.show_teams { "Pick a channel." } else if chats.is_empty() { "No chats yet." } else { "Pick a chat." })),
        (Some(_), None) => {
            pane.key("messages-loading").center().motion(rune::Spring::NONE)
                .child(loading_messages())
        },
        (Some(_), Some(Err(e))) => pane.center().padding(32.0).child(note(e).wrap()),
        (Some(id), Some(Ok(page))) => {
            let msgs = &page.msgs;
            // Fetch older a screenful before the top, so it's usually in before the scroll gets there.
            let edge = if page.older.is_some() { Some(15.min(msgs.len().saturating_sub(1))) } else { None };
            let title = chats.iter().chain(s.teams.as_ref().and_then(|r| r.as_ref().ok()).into_iter().flatten().flat_map(|team| &team.channels)).find(|c| &c.id == id).map_or("Chat", |c| c.title.as_str());
            let bubbles = msgs.iter().enumerate().flat_map(|(i, m)| {
                let prev = i.checked_sub(1).map(|j| &msgs[j]);
                let new_day = prev.is_none_or(|p| day(&p.time) != day(&m.time));
                // A run of messages from one sender shares one avatar and name.
                let head = new_day || prev.is_some_and(|p| p.sender != m.sender || p.mine != m.mine);
                let time = note(m.time.get(11..16).unwrap_or(&m.time)).font(type_::LABEL);
                let bubble = col().max_w(520.0).shrink().min_w(0.0).px(12.0).py(8.0).radius(12.0)
                    .fill(if m.mine { tok::accent(0, true).alpha(0.3) } else { t.card })
                    .border(2.0, tok::accent(0, true).alpha(if s.reply_highlight_on && s.reply_highlight.as_ref().is_some_and(|(id, _)| id == &m.id) { 1.0 } else { 0.0 }))
                    .children(m.reply.as_ref().map(|reply| {
                        let target = reply.id.clone();
                        col().w_full().min_w(0.0).gap(4.0).padding(8.0).mb(6.0).radius(4.0).fill(t.bg)
                            .border_sides([0.0, 0.0, 0.0, 2.0], t.text_secondary)
                            .cursor(rune::Cursor::Pointer).hover(t.hover)
                            .label("Go to quoted message").focusable(true)
                            .on_press_tx(move |s: &mut State, tx| {
                                s.reply_highlight = None;
                                s.reply_error = None;
                                s.reply_target = Some(target.clone());
                                reveal_reply(s, tx);
                            })
                            .child(note(&format!("{}  {}", reply.sender, if reply.time.is_empty() { String::new() } else { format!("{} UTC", reply.time.get(..16).unwrap_or(&reply.time).replace('T', " ")) })).font(type_::LABEL).wrap())
                            .child(text(reply.text.as_str()).font(type_::BODY).color(t.text).wrap())
                    }))
                    .children((!m.text.is_empty()).then(|| text(m.text.as_str()).font(type_::BODY).color(t.text).wrap().selectable(true)));
                // Keyed by id, not index: the 50-message window slides on a new message, and index keys would morph each bubble into its neighbour.
                let line = row().key(format!("msg/{}", m.id)).arrives_whole().w_full().gap(8.0).mt(if head { 10.0 } else { 0.0 });
                let line = if edge == Some(i) {
                    line.on_visible(|s: &mut State, on, _| if on { load_older(s) })
                } else {
                    line
                };
                let line = if m.mine {
                    line.justify_end().items_end().child(time).child(bubble)
                } else {
                    line.child(if head { avatar(&m.sender, 32.0) } else { el().w(32.0) })
                        .child(col().gap(4.0).grow(1.0).shrink().min_w(0.0)
                            .children(head.then(|| text(m.sender.as_str()).font(type_::LABEL).color(t.text_secondary)))
                            .child(row().gap(8.0).shrink().min_w(0.0).items_end().child(bubble).child(time)))
                };
                let divider = new_day.then(|| row().key(format!("day/{}", m.id)).arrives_whole().w_full().justify_center().mt(10.0)
                    .child(note(day(&m.time)).font(type_::LABEL)));
                divider.into_iter().chain([line])
            });
            pane.child(row().w_full().gap(10.0).px(16.0).py(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                    .child(avatar(title, 32.0))
                    .child(text(title).font(type_::TITLE).color(t.text).truncate()))
                // A growth stack, so a sent message and the viewport following it glide in as one.
                .child(with_bar("messages", col().w_full().items_center().px(16.0).py(8.0)
                    .child(col().w_full().max_w(1040.0).gap(4.0).growth(GrowthSpec::default())
                        .children(bubbles)
                        .children(msgs.is_empty().then(|| note("No messages."))))
                    .scroll(ScrollSpec { stick_bottom: true }).scroll_anchor().grow(1.0).min_h(0.0), &s.msg_bar))
                .child(col().w_full().items_center().px(16.0).py(10.0).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
                    .child(col().w_full().max_w(1040.0).gap(6.0)
                    .children(s.reply_error.as_deref().map(|e| note(e).wrap()))
                    .children(s.send_error.as_deref().map(|e| note(e).wrap()))
                    .child(row().w_full().gap(8.0).items_end()
                        .child(col().grow(1.0).shrink().min_w(0.0).px(12.0).py(8.0).radius(8.0).fill(t.card)
                            .child(auto_grow(text_input(&s.draft, type_::BODY).key("draft").label("Message").color(t.text)
                                .placeholder("Type a message", t.text_secondary)
                                .caret(t.text, tok::accent(0, true).alpha(0.3))
                                .on_edit(|s: &mut State| &mut s.draft)
                                .on_submit(|s: &mut State, _| send(s)), 1, 6)))
                        .child(button("send", "Send", true, send)))))
        }
    };
    col().w_full().h_full().fill(t.bg).child(search)
        .child(row().w_full().grow(1.0).min_h(0.0).child(rail).child(sidebar).child(divider).child(pane))
        .children(results)
}

fn panel_width(start: f32, delta: f32) -> f32 {
    (start + delta).clamp(0.0, 480.0)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let title = |s: &str| text(s).font(type_::TITLE).color(t.text);
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary).wrap();
    let page = col().w_full().h_full().center().gap(14.0).padding(32.0).fill(t.bg);
    match &s.screen {
        Screen::Loading => page.child(note("Signing in…")),
        Screen::Login(error) => page
            .child(title("Microsoft Teams"))
            .children(error.as_deref().map(note))
            .child(button("sign-in", "Sign in with Microsoft", true, |s| sign_in(s, true))),
        Screen::Code { user_code, url } => page
            .child(note("Enter this code at"))
            .child(text(url.as_str()).key("url").font(type_::BODY).color(t.text).selectable(true))
            .child(text(user_code.as_str()).key("code").font(rune::TextStyle::new(28.0, 600.0)).color(t.text).selectable(true))
            .child(note("Waiting for you to finish in the browser…"))
            .child(button("cancel", "Cancel", false, |s| {
                s.attempt.fetch_add(1, SeqCst);
                s.screen = Screen::Login(None);
            })),
        Screen::Main { session, chats } => main_view(s, session, chats),
    }
}

fn demo(s: &mut State) {
    let chat = |id: &str, title: &str, preview: &str, time: &str| Chat { id: id.into(), title: title.into(), preview: preview.into(), time: time.into() };
    let msg = |sender: &str, text: &str, time: &str, mine| Msg { id: time.into(), server_id: time.into(), sender: sender.into(), text: text.into(), time: time.into(), mine, reply: None };
    s.screen = Screen::Main {
        session: Arc::new(Session {
            user: auth::User { name: "Ada Lovelace".into(), email: "ada@example.com".into(), id: "me".into() },
            ..Session::default()
        }),
        chats: vec![
            chat("a", "Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z"),
            chat("b", "Design review", "Moved to Thursday", "2026-11-14T09:02:00Z"),
            chat("c", "Mary Somerville", "Thanks!", "2026-11-12T17:45:00Z"),
        ],
    };
    s.selected = Some("a".into());
    s.messages = Some(Ok(Page { older: None, msgs: vec![
        msg("Charles Babbage", "Did you get a chance to look at the notes?", "2026-11-14T10:20:00Z", false),
        msg("Ada Lovelace", "Yes. Note G is done: it computes the Bernoulli numbers.\nI'll send it tonight.", "2026-11-14T10:24:00Z", true),
        msg("Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z", false),
    ] }));
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("login", |s| s.screen = Screen::Login(None)),
            ("error", |s| s.screen = Screen::Login(Some("chat service 403: Forbidden".into()))),
            ("code", |s| s.screen = Screen::Code { user_code: "ABCD1234".into(), url: "https://microsoft.com/devicelogin".into() }),
            ("main", demo),
            ("reply", |s| {
                demo(s);
                if let Some(Ok(page)) = &mut s.messages {
                    let original = page.msgs[0].clone();
                    page.msgs[1].reply = Some(teams::Reply {
                        id: original.server_id, sender: original.sender, time: original.time, text: original.text,
                    });
                }
            }),
            ("teams", |s| {
                demo(s);
                s.show_teams = true;
                s.selected = None;
                s.messages = None;
                let channel = |id: &str, title: &str| Chat {
                    id: id.into(), title: title.into(), preview: String::new(), time: String::new(),
                };
                s.teams = Some(Ok(vec![
                    teams::Team { id: "development".into(), title: "Development".into(), channels: vec![
                        channel("backend", "Backend"), channel("mobile", "Mobile application"), channel("notifications", "Notifications"),
                    ] },
                    teams::Team { id: "trential".into(), title: "Trential".into(), channels: vec![
                        channel("general", "General"), channel("announcements", "Announcements"), channel("random", "Random"),
                    ] },
                ]));
            }),
            ("loading", |s| {
                demo(s);
                s.messages = None;
            }),
            ("search", |s| {
                demo(s);
                s.search = TextEdit::new("char");
            }),
            ("pick", |s| {
                demo(s);
                (s.selected, s.messages) = (None, None);
            }),
        ],
        setup: Some(setup),
        poll: Some(poll),
        min_size: Some((640.0, 400.0)),
        ..AppSpec::new("Teams", (1000.0, 680.0), init, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_highlight_blinks_three_times_then_stops() {
        assert_eq!(reply_blink(0.0), Some(true));
        assert_eq!(reply_blink(0.31), Some(false));
        assert_eq!(reply_blink(0.61), Some(true));
        assert_eq!(reply_blink(0.91), Some(false));
        assert_eq!(reply_blink(1.21), Some(true));
        assert_eq!(reply_blink(1.51), Some(false));
        assert_eq!(reply_blink(1.8), None);
        assert_eq!(reply_blink(10.0), None);
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
        navigate(&mut s, false);
        assert!(s.reply_highlight.is_none());
        assert!(s.reply_target.is_none());
        assert!(s.reply_error.is_none());
    }

    #[test]
    fn panel_resize_limits_and_reopening() {
        assert_eq!(panel_width(300.0, 60.0), 360.0);
        assert_eq!(panel_width(300.0, -500.0), 0.0);
        assert_eq!(panel_width(0.0, 80.0), 80.0);
        assert_eq!(panel_width(300.0, 500.0), 480.0);
        assert_eq!(panel_width(480.0, -40.0), 440.0);
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

    #[test]
    fn thumb_tracks_offset() {
        let mut at = ScrollPos::default();
        at.viewport = [300.0, 400.0];
        assert_eq!(thumb(at), [0.0; 4], "nothing to scroll");
        at.max[1] = 400.0; // content twice the viewport: thumb is half the track
        assert_eq!(thumb(at), [294.0, 2.0, 4.0, 198.0]);
        at.offset[1] = 400.0;
        assert_eq!(thumb(at), [294.0, 200.0, 4.0, 198.0]);
        assert_eq!(offset_at(at, 200.0), 400.0, "inverse of thumb");
        assert_eq!(offset_at(at, 2.0), 0.0);
        assert_eq!(offset_at(at, -50.0), 0.0, "dragged past the top");
        at.max[1] = 1e6; // long content: thumb keeps its minimum
        assert_eq!(thumb(at)[3], 24.0);
    }
}
