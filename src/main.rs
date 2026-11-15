//! A Teams desktop client: device-code sign-in that lasts across launches, then your chats.

mod auth;
mod avatars;
#[cfg(feature = "image")]
mod images;
#[cfg(feature = "markdown")]
mod markdown;
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
    avatars: avatars::Avatars,
    #[cfg(feature = "image")]
    images: images::Images,
    panel_width: f32,
    /// The picked theme: `None` follows the system, else dark or light.
    theme: Option<bool>,
    /// The picture open full-window, by preview URL; closed if it isn't in the open chat.
    #[cfg_attr(not(feature = "image"), allow(dead_code))]
    viewer: Option<String>,
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
    loader: Latest<(String, Messages, Result<(), String>)>,
    /// A fetch of the page before the loaded ones: the chat, the link it followed, the page.
    older_loader: Latest<(String, String, Messages)>,
    /// The message being written to the selected chat.
    draft: TextEdit,
    /// The draft open in the pop-up editor; its preview on, the writing side's share of the
    /// editor's width, and that width as last laid out.
    editor: bool,
    editor_preview: bool,
    editor_split: f32,
    editor_w: f32,
    /// The sent message the draft replaces when sent, as it was before the edit.
    editing: Option<Msg>,
    /// Pictures pasted into the draft, with their decoded thumbnails; sent with it.
    pasted: Vec<(rune::ClipboardImage, rune::Image)>,
    /// A send in flight: the chat, the message's id, then its reloaded messages and the pictures'
    /// URLs, or the error and the unsent text, pictures, and edited and quoted messages.
    sender: Latest<(String, String, Result<(Messages, Vec<String>), (String, String, Vec<(rune::ClipboardImage, rune::Image)>, (Option<Msg>, Option<Msg>))>)>,
    /// The message the draft replies to when sent.
    replying: Option<Msg>,
    /// The message whose "…" menu is open, by id.
    menu: Option<String>,
    /// The message under the pointer, and the one whose toolbar is under it, by id. The toolbar
    /// reaches over the row above, so it wins: only one message's toolbar shows.
    hovered: Option<String>,
    hovered_bar: Option<String>,
    /// The message waiting for the user to confirm its deletion.
    deleting: Option<Msg>,
    /// A delete in flight: the chat, the message's id, and how it went.
    deleter: Latest<(String, String, Result<(), String>)>,
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
        avatars: Default::default(),
        #[cfg(feature = "image")]
        images: Default::default(),
        panel_width: PANEL_MAX,
        theme: None,
        viewer: None,
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
        editor: false,
        editor_preview: true,
        editor_split: 0.75,
        editor_w: 0.0,
        editing: None,
        pasted: Vec::new(),
        sender: Latest::default(),
        replying: None,
        menu: None,
        hovered: None,
        hovered_bar: None,
        deleting: None,
        deleter: Latest::default(),
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
    s.avatars = Default::default();
    #[cfg(feature = "image")]
    { s.images = Default::default(); }
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
    s.editing = None;
    s.editor = false;
    (s.replying, s.menu, s.deleting) = (None, None, None);
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

fn open_chat(s: &mut State, id: String) {
    s.reply_highlight = None;
    s.reply_target = None;
    s.reply_error = None;
    let Screen::Main { session, chats } = &s.screen else { return };
    let unread = chats.iter().any(|chat| chat.id == id && chat.unread);
    let session = session.clone();
    if s.selected.as_ref() != Some(&id) {
        // A draft belongs to the chat it was written in; it must not follow to another one.
        s.draft.clear();
        s.editing = None;
        s.editor = false;
        (s.replying, s.menu, s.deleting) = (None, None, None);
        s.send_error = None;
    }
    (s.selected, s.messages) = (Some(id.clone()), None);
    s.loader.request(id, move |id| {
        let r = teams::messages(&session, &id);
        let read = match &r {
            Ok(page) if unread => teams::mark_read(&session, &id, page),
            _ => Ok(()),
        };
        (id, r, read)
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

/// Shows the draft in the selected chat at once and sends it (or, while editing, puts it in place
/// of the edited message's text), then reloads its messages.
fn send(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    let text = s.draft.text.trim().to_string();
    // One send at a time: `Latest` would drop the first one's answer.
    if (text.is_empty() && s.pasted.is_empty()) || s.sender.is_pending() {
        return;
    }
    let session = session.clone();
    let id = id.clone();
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
    if let (Some(Ok(page)), Some(e)) = (&mut s.messages, &editing) {
        if let Some(m) = page.msgs.iter_mut().find(|m| m.id == e.id) {
            m.text = text.clone();
            // Its link ranges point into the old text.
            m.links.clear();
            m.images.extend(local.iter().cloned());
        }
    } else if let Some(Ok(page)) = &mut s.messages {
        // In this input's commit, so it glides in; the reload's copy shares its id and key.
        page.msgs.push(Msg { id: msg_id.clone(), server_id: String::new(), sender: session.user.name.clone(), sender_id: session.user.id.clone(), text: text.clone(), time: teams::now(), mine: true, reply: replying.as_ref().map(|q| teams::Reply { id: q.server_id.clone(), sender: q.sender.clone(), time: q.time.clone(), text: q.text.clone() }), images: local, links: vec![], files: vec![], raw: String::new() });
    }
    s.draft.clear();
    s.send_error = None;
    s.sender.request((id, msg_id, text, pasted, editing, replying), move |(id, msg_id, text, pasted, editing, replying)| {
        let sent = (|| {
            let urls = pasted.iter().map(|(png, _)| teams::upload_image(&session, &id, png.png())).collect::<Result<Vec<_>, _>>()?;
            let sized: Vec<_> = urls.iter().cloned().zip(pasted.iter().map(|(png, _)| png.size())).collect();
            match &editing {
                Some(e) => teams::edit(&session, &id, &e.server_id, &e.raw, &text, &sized),
                None => teams::send(&session, &id, &msg_id, &text, &sized, replying.as_ref()),
            }.map(|()| urls)
        })();
        let r = match sent {
            Ok(urls) => Ok((teams::messages(&session, &id), urls)),
            Err(e) => Err((e, text, pasted, (editing, replying))),
        };
        (id, msg_id, r)
    });
}

/// The key a sent picture shows under before its upload has a URL.
fn local_image(msg_id: &str, i: usize) -> String {
    format!("local:{msg_id}:{i}")
}

/// Attach a pasted picture to the draft, decoding its thumbnail off the UI thread.
#[cfg(feature = "image")]
fn paste_image(s: &mut State, png: &rune::ClipboardImage) {
    let Some(waker) = s.waker.clone() else { return };
    let image = rune::ImageCache::new(waker).load(png.png().to_vec());
    s.pasted.push((png.clone(), image));
}

fn setup(s: &mut State, waker: Waker) {
    s.teams_loader.set_waker(waker.clone());
    s.loader.set_waker(waker.clone());
    s.sender.set_waker(waker.clone());
    s.older_loader.set_waker(waker.clone());
    s.deleter.set_waker(waker.clone());
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
    if let Some((id, r, read)) = s.loader.take() {
        if s.selected.as_ref() == Some(&id) {
            if r.is_ok() && read.is_ok() && let Screen::Main { chats, .. } = &mut s.screen
                && let Some(chat) = chats.iter_mut().find(|chat| chat.id == id) {
                chat.unread = false;
            }
            if let Err(error) = read {
                s.reply_error = Some(format!("Could not save read status: {error}. Reopen the chat to retry."));
            }
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
    if let Some((id, msg_id, r)) = s.deleter.take() {
        match r {
            Ok(()) => if let (true, Some(Ok(page))) = (s.selected.as_ref() == Some(&id), &mut s.messages) {
                page.msgs.retain(|m| m.id != msg_id);
            },
            Err(e) => s.send_error = Some(format!("Not deleted: {e}")),
        }
        changed = true;
    }
    if let Some((id, msg_id, r)) = s.sender.take() {
        let here = s.selected.as_ref() == Some(&id);
        let r = r.map(|(fresh, urls)| {
            // The server's copy names the uploads; show them from the pasted pixels, not a download.
            #[cfg(feature = "image")]
            for (i, url) in urls.into_iter().enumerate() {
                if let Some(image) = s.images.get(&local_image(&msg_id, i)).flatten().cloned() {
                    s.images.insert(url, Some(image));
                }
            }
            #[cfg(not(feature = "image"))]
            let _ = urls;
            fresh
        });
        match r {
            Ok(Ok(fresh)) if here => {
                s.messages = Some(Ok(match s.messages.take() {
                    Some(Ok(page)) => teams::merge(page, fresh),
                    _ => fresh,
                }));
            }
            Ok(fresh) if here => s.messages = Some(fresh),
            Ok(_) => {}
            Err((e, text, pasted, (editing, replying))) => {
                if let (true, Some(Ok(page))) = (here, &mut s.messages) {
                    page.msgs.retain(|m| m.id != msg_id);
                    // An edit that failed shows the message as it was.
                    if let Some(e) = &editing && let Some(m) = page.msgs.iter_mut().find(|m| m.id == e.id) {
                        *m = e.clone();
                    }
                }
                s.send_error = Some(format!("Not {}: {e}", if editing.is_some() { "edited" } else { "sent" }));
                if here && s.draft.text.is_empty() && s.editing.is_none() && s.replying.is_none() {
                    (s.editing, s.replying) = (editing, replying);
                }
                // Give the text back, unless the user moved on to another chat or draft.
                if here && s.draft.text.is_empty() {
                    s.draft.replace_text(text);
                }
                if here && s.pasted.is_empty() {
                    s.pasted = pasted;
                }
            }
        }
        changed = true;
    }
    if std::env::args().len() == 1 && let Screen::Main { session, chats } = &s.screen {
        let ids = std::iter::once(session.user.id.clone())
            .chain(chats.iter().map(|c| chat_avatar_id(c, &session.user.id).to_owned()))
            .chain(chats.iter().flat_map(|c| c.avatar_members.iter().take(3).map(|m| m.id.clone())))
            .chain(s.messages.as_ref().and_then(|r| r.as_ref().ok()).into_iter().flat_map(|p| p.msgs.iter().map(|m| m.sender_id.clone())));
        changed |= s.avatars.poll(session, ids, s.waker.clone().unwrap());
        #[cfg(feature = "image")]
        {
            // The open picture's full size first, so it isn't queued behind the chat's previews.
            let urls = s.viewer.iter().map(|u| images::full(u)).chain(s.messages.as_ref().and_then(|r| r.as_ref().ok()).into_iter().flat_map(|p| p.msgs.iter().flat_map(|m| m.images.iter().cloned().chain(m.files.iter().filter_map(|f| f.preview.clone())))));
            changed |= s.images.poll(session, urls, s.waker.clone().unwrap());
        }
        tx.wake_after(std::time::Duration::from_secs(60));
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
        .child(el().absolute(0.0, 0.0).radius(2.0).fill(tok::theme(dark()).text_secondary.alpha(0.5)).live_rect(&bar.rect))
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
    let t = tok::theme(dark());
    el().key(key).role("button").label(label).focusable(true).px(16.0).py(8.0).radius(8.0)
        .fill(if primary { tok::accent(0, dark()) } else { t.control_off })
        .on_press(on_press)
        .child(text(label).font(type_::TITLE).color(t.text))
}

fn local_time(iso: &str, format: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|time| time.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_else(|_| iso.to_owned())
}

fn message_date(iso: &str, today: chrono::NaiveDate) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let date = time.with_timezone(&chrono::Local).date_naive();
    match (today - date).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=7 => date.format("%A").to_string(),
        _ => date.format("%A, %B %-d").to_string(),
    }
}

fn day(iso: &str) -> String {
    local_time(iso, "%Y-%m-%d")
}

fn chat_time(iso: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let time = time.with_timezone(&chrono::Local);
    let format = if time.date_naive() == now.date_naive() {
        "%-I:%M %p"
    } else {
        "%-m/%-d"
    };
    time.format(format).to_string()
}

fn message_time(iso: &str, today: chrono::NaiveDate) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let time = time.with_timezone(&chrono::Local);
    let date = if (0..=7).contains(&(today - time.date_naive()).num_days()) {
        message_date(iso, today)
    } else {
        time.format("%-m/%-d").to_string()
    };
    format!("{date} {}", time.format("%-I:%M %p"))
}

fn same_message_group(previous: &Msg, current: &Msg) -> bool {
    previous.sender == current.sender && previous.mine == current.mine
        && day(&previous.time) == day(&current.time)
        && chrono::DateTime::parse_from_rfc3339(&previous.time).ok()
            .zip(chrono::DateTime::parse_from_rfc3339(&current.time).ok())
            .is_some_and(|(before, after)| {
                let gap = after - before;
                gap >= chrono::Duration::zero() && gap < chrono::Duration::minutes(5)
            })
}

/// A circle with the name's initials, coloured by the name so a person keeps their colour.
fn avatar(name: &str, size: f32) -> El {
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect();
    let hue = name.bytes().map(usize::from).sum();
    el().size(size, size).radius(size / 2.0).center().fill(tok::accent(hue, dark()))
        .child(text(initials.as_str()).font(type_::LABEL).color(tok::theme(dark()).bg))
}

fn chat_avatar_id<'a>(chat: &'a Chat, me: &'a str) -> &'a str {
    if !chat.avatar_id.is_empty() { &chat.avatar_id }
    else if chat.id == "48:notes" { me }
    else { teams::peer(&chat.id, me).unwrap_or("") }
}

fn chat_avatar(s: &State, chat: &Chat, me: &str, size: f32) -> El {
    let members = &chat.avatar_members;
    if members.len() < 2 {
        return user_avatar(s, chat_avatar_id(chat, me), &chat.title, size);
    }
    let tile = |member: &auth::User, w: f32, h: f32| {
        match s.avatars.get(&member.id) {
            Some(image) => el().size(w, h).pixels_fit(image.pixels(), rune::SurfaceFit::Fill),
            None => avatar(&member.name.chars().take(1).collect::<String>(), w.max(h)).size(w, h).radius(0.0).clip_rounded(0.0),
        }
    };
    let half = size / 2.0;
    let right = if members.len() >= 3 {
        col().size(half, size)
            .child(tile(&members[1], half, half))
            .child(tile(&members[2], half, half))
    } else { tile(&members[1], half, size) };
    row().size(size, size).clip_rounded(half).label(chat.title.as_str())
        .child(tile(&members[0], half, size)).child(right)
}

fn user_avatar(s: &State, id: &str, name: &str, size: f32) -> El {
    match s.avatars.get(id) {
        Some(image) => el().size(size, size).clip_rounded(size / 2.0).pixels(image.pixels()).label(name),
        None => avatar(name, size),
    }
}

/// A picture's tile in a message, as Teams lays them out side by side.
#[cfg(feature = "image")]
const IMAGE_TILE: (f32, f32) = (256.0, 198.0);

#[cfg(feature = "image")]
fn message_image(s: &State, url: &str) -> El {
    let t = tok::theme(dark());
    let open = url.to_owned();
    let tile = el().size(IMAGE_TILE.0, IMAGE_TILE.1).radius(6.0).clip_rounded(6.0).fill(t.bg).border(1.0, t.separator)
        .role("button").focusable(true).cursor(rune::Cursor::Pointer).hover(t.hover)
        .on_press(move |s: &mut State| s.viewer = Some(open.clone()));
    match s.images.get(url) {
        // Letterboxed, so a tall phone screenshot and a wide desktop one share the tile size.
        Some(Some(image)) => tile.pixels_fit(image.pixels(), rune::SurfaceFit::Fit).label("Open image"),
        Some(None) => tile.center().label("Image unavailable").child(text("Image unavailable").font(type_::LABEL).color(t.text_secondary)),
        None => tile.label("Loading image"),
    }
}

/// The open picture filling the window, as Teams shows it, with the chat's other pictures a click or arrow key away.
#[cfg(feature = "image")]
fn image_viewer(s: &State) -> Option<El> {
    static BACK: rune::Icon = rune::Icon { name: "viewer-back", view: 24.0, body: r#"<path d="M20 12H5 M11 6l-6 6 6 6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static PREV: rune::Icon = rune::Icon { name: "viewer-prev", view: 24.0, body: r#"<path d="M15 5l-7 7 7 7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static NEXT: rune::Icon = rune::Icon { name: "viewer-next", view: 24.0, body: r#"<path d="M9 5l7 7-7 7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"# };
    let open = s.viewer.as_deref()?;
    let page = s.messages.as_ref()?.as_ref().ok()?;
    let all: Vec<(&Msg, &String)> = page.msgs.iter().flat_map(|m| m.images.iter().map(move |u| (m, u))).collect();
    let i = all.iter().position(|(_, u)| *u == open)?;
    let msg = all[i].0;
    let prev = i.checked_sub(1).map(|j| all[j].1.clone());
    let next = all.get(i + 1).map(|(_, u)| (*u).clone());
    let t = tok::theme(dark());
    let icon_button = |icon: &'static rune::Icon, label: &'static str, target: Option<String>| {
        el().size(40.0, 40.0).center().radius(20.0).role("button").label(label).focusable(true)
            .cursor(rune::Cursor::Pointer).hover(t.hover)
            .on_press(move |s: &mut State| s.viewer = target.clone())
            .child(rune::icon(icon, 24.0).color(t.text))
    };
    let arrow = |icon, label, target: Option<String>| match target {
        Some(_) => icon_button(icon, label, target),
        None => el().size(40.0, 40.0),
    };
    let full = images::full(open);
    // The preview stands in until the full size arrives, or if it can't.
    let picture = match s.images.get(&full).flatten().or(s.images.get(open).flatten()) {
        Some(image) => el().grow(1.0).shrink().min_w(0.0).h_full().pixels_fit(image.pixels(), rune::SurfaceFit::Fit).label("Image"),
        None if s.images.get(&full).is_some() => el().grow(1.0).h_full().center()
            .child(text("Image unavailable").font(type_::BODY).color(t.text_secondary)),
        None => el().grow(1.0).h_full().center().child(loading_messages()),
    };
    let (back, keys) = (prev.clone(), next.clone());
    Some(col().key("image-viewer").w_full().h_full().fill(t.bg).focusable(true).autofocus().label("Image viewer")
        .modal(false, |s: &mut State| s.viewer = None)
        .on_key(move |s: &mut State, e: &rune::KeyEvent, _| {
            s.viewer = match e.key {
                rune::Key::Left if back.is_some() => back.clone(),
                rune::Key::Right if keys.is_some() => keys.clone(),
                _ => return false,
            };
            true
        })
        .child(row().w_full().items_center().gap(12.0).px(12.0).py(8.0).fill(t.card).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
            .child(icon_button(&BACK, "Close image", None))
            .child(col().gap(2.0)
                .child(text(msg.sender.as_str()).font(type_::LABEL).color(t.text))
                .child(text(message_time(&msg.time, chrono::Local::now().date_naive())).font(type_::LABEL).color(t.text_secondary))))
        .child(row().grow(1.0).w_full().min_h(0.0).items_center().gap(12.0).padding(16.0)
            .child(arrow(&PREV, "Previous image", prev))
            .child(picture)
            .child(arrow(&NEXT, "Next image", next))))
}

#[cfg(not(feature = "image"))]
fn image_viewer(_: &State) -> Option<El> {
    None
}

#[cfg(not(feature = "image"))]
fn message_image(_: &State, _: &str) -> El {
    text("[image]").font(type_::BODY).color(tok::theme(dark()).text_secondary)
}

/// A shared file as Teams shows it: its name over a preview of its first page, or its icon when
/// there's none (not on SharePoint, or not shared with us). A click opens it in the browser.
fn file_card(s: &State, f: &teams::File) -> El {
    static DOC: rune::Icon = rune::Icon { name: "file-doc", view: 24.0, body: r#"<path d="M6 2.75h8l4.25 4.25v14.25H6z M14 2.75V7h4.25 M9 12h6 M9 15h6 M9 18h4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"# };
    const W: f32 = 360.0;
    const PREVIEW_H: f32 = 200.0;
    let t = tok::theme(dark());
    // Office's colours, so a Word file reads as Word at a glance.
    let tint = match f.kind.as_str() {
        "doc" | "docx" => rune::color::hex("#185abd").unwrap_or(t.text_secondary),
        "xls" | "xlsx" | "csv" => rune::color::hex("#107c41").unwrap_or(t.text_secondary),
        "ppt" | "pptx" => rune::color::hex("#c43e1c").unwrap_or(t.text_secondary),
        "pdf" => rune::color::hex("#d13438").unwrap_or(t.text_secondary),
        "loop" => rune::color::hex("#7b5ce6").unwrap_or(t.text_secondary),
        _ => t.text_secondary,
    };
    #[cfg(feature = "image")]
    let preview = f.preview.as_deref().map_or(Some(None), |u| s.images.get(u));
    #[cfg(not(feature = "image"))]
    let preview: Option<Option<&rune::Image>> = { let _ = s; Some(None) };
    let body = el().w_full().h(PREVIEW_H).clip_rounded(0.0).fill(t.bg);
    let body = match preview {
        // Full width from the top, as Teams crops a page's preview.
        Some(Some(image)) => {
            let [w, h] = image.size().unwrap_or([1, 1]);
            body.fill(rune::Color::WHITE).child(el().w(W).h(W * h as f32 / w.max(1) as f32).pixels_fit(image.pixels(), rune::SurfaceFit::Fit).a11y_hidden())
        }
        Some(None) => body.center().child(rune::icon(&DOC, 40.0).color(tint)),
        None => body,
    };
    let url = f.url.clone();
    col().w(W).radius(8.0).clip_rounded(8.0).fill(t.card).border(1.0, t.separator)
        .role("link").label(format!("Open {}", f.name)).help(f.url.as_str()).focusable(true)
        .cursor(rune::Cursor::Pointer).hover(t.hover)
        .on_press_tx(move |_: &mut State, tx| { let _ = tx.open_url(&url); })
        .child(row().w_full().items_center().gap(10.0).px(12.0).py(10.0).border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
            .child(rune::icon(&DOC, 24.0).color(tint))
            .child(text(f.name.as_str()).font(type_::TITLE).color(t.text).truncate().min_w(0.0)))
        .child(body)
}

/// Whether `before_caret` leaves a ``` block open: there Enter types a line, as in Teams, rather than sending.
fn in_code_block(before_caret: &str) -> bool {
    before_caret.lines().filter(|l| l.trim_start().starts_with("```")).count() % 2 == 1
}

/// Puts `m`'s text in the box to replace it when sent.
fn start_edit(s: &mut State, m: &Msg) {
    s.draft.replace_text(m.text.clone());
    (s.editing, s.replying, s.menu) = (Some(m.clone()), None, None);
    s.send_error = None;
}

/// Quotes `m` above the box, so the next send replies to it.
fn start_reply(s: &mut State, m: &Msg) {
    // A reply is a new message: an edit's text isn't its draft.
    if s.editing.take().is_some() { s.draft.clear(); }
    (s.replying, s.menu) = (Some(m.clone()), None);
    s.send_error = None;
}

/// Deletes the message the user confirmed; it leaves the list once Teams has deleted it.
fn delete_message(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id), Some(m)) = (&s.screen, &s.selected, s.deleting.take()) else { return };
    let session = session.clone();
    s.send_error = None;
    s.deleter.request((id.clone(), m), move |(id, m)| {
        let r = teams::delete(&session, &id, &m.server_id);
        (id, m.id, r)
    });
}

/// The hover toolbar of a message the server has (its actions need its id): edit for one of
/// mine, quote (reply) for someone else's, and "…" for the rest. It stays shown while its menu is open.
// shortcut: no reactions yet; add their buttons here when the client can send them.
fn message_actions(m: &Msg, shown: bool) -> Option<El> {
    static PENCIL: rune::Icon = rune::Icon { name: "msg-edit", view: 20.0, body: r#"<path d="M13.5 3.5l3 3L7 16H4v-3z M11.5 5.5l3 3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>"# };
    static QUOTE: rune::Icon = rune::Icon { name: "msg-quote", view: 20.0, body: r#"<path d="M4 11.5c0-3 1.5-5 4-5.5 M4 11.5h3.5v3.5H4z M11 11.5c0-3 1.5-5 4-5.5 M11 11.5h3.5v3.5H11z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>"# };
    static MORE: rune::Icon = rune::Icon { name: "msg-more", view: 20.0, body: r#"<circle cx="5" cy="10" r="1.3" fill="currentColor"/><circle cx="10" cy="10" r="1.3" fill="currentColor"/><circle cx="15" cy="10" r="1.3" fill="currentColor"/>"# };
    if m.server_id.is_empty() { return None }
    let t = tok::theme(dark());
    let button = |key: String, label: &'static str, icon: &'static rune::Icon| el().key(key).size(28.0, 28.0).center().radius(6.0)
        .role("button").label(label).focusable(true).cursor(rune::Cursor::Pointer).hover(t.hover)
        .child(rune::icon(icon, 16.0).color(t.text));
    let first = {
        let m = m.clone();
        if m.mine {
            button(format!("edit/{}", m.id), "Edit message", &PENCIL).on_press(move |s: &mut State| start_edit(s, &m))
        } else {
            button(format!("quote/{}", m.id), "Reply", &QUOTE).on_press(move |s: &mut State| start_reply(s, &m))
        }
    };
    let id = m.id.clone();
    let more = button(format!("more/{}", m.id), "More options", &MORE)
        .hover(t.hover)
        .on_press(move |s: &mut State| s.menu = if s.menu.as_ref() == Some(&id) { None } else { Some(id.clone()) });
    Some(row().gap(2.0).padding(3.0).radius(8.0).fill(t.card).border(1.0, t.separator)
        .shadow(2.0, 8.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 })
        // Shown for the hovered message (or its open menu); a hovered group without it is the row
        // above the pointer, which the toolbar of the one below overlaps.
        .group_opacity(if shown { 1.0 } else { 0.0 }, if shown { 1.0 } else { 0.0 })
        .child(first).child(more))
}

/// The open "…" menu, under its button: copy, plus reply, edit and delete for one of mine
/// (someone else's has reply in the toolbar).
fn message_menu(s: &State) -> Option<El> {
    let id = s.menu.as_deref()?;
    let m = s.messages.as_ref()?.as_ref().ok()?.msgs.iter().find(|m| m.id == id)?;
    let t = tok::theme(dark());
    let item = |key: &'static str, label: &'static str| row().key(key).role("menuitem").label(label).focusable(true)
        .w_full().px(12.0).py(6.0).radius(4.0).cursor(rune::Cursor::Pointer).hover(t.hover)
        .child(text(label).font(type_::BODY).color(t.text));
    let (reply, copy, edit, delete) = (m.clone(), m.text.clone(), m.clone(), m.clone());
    Some(col().key("message-menu").role("menu").label("Message options").min_w(160.0).padding(4.0).radius(8.0)
        .fill(t.card).border(1.0, t.separator).shadow(4.0, 16.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.4 })
        .anchored(format!("more/{id}"), Side::Bottom, Align::End, 4.0)
        .modal(true, |s: &mut State| s.menu = None)
        .children(m.mine.then(|| item("menu-reply", "Reply").on_press(move |s: &mut State| start_reply(s, &reply))))
        .child(item("menu-copy", "Copy text").on_press_tx(move |s: &mut State, tx| { tx.copy(copy.clone()); s.menu = None; }))
        .children(m.mine.then(|| item("menu-edit", "Edit").on_press(move |s: &mut State| start_edit(s, &edit))))
        .children(m.mine.then(|| item("menu-delete", "Delete").on_press(move |s: &mut State| { s.deleting = Some(delete.clone()); s.menu = None; }))))
}

/// A picture waiting in the draft, with a button to take it out.
fn pasted_image(image: &rune::Image, i: usize) -> El {
    static CLOSE: rune::Icon = rune::Icon { name: "pasted-remove", view: 16.0, body: r#"<path d="M4 4l8 8 M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"# };
    let t = tok::theme(dark());
    col().key(format!("pasted/{i}")).size(150.0, 100.0).radius(6.0).clip_rounded(6.0).fill(t.bg).border(1.0, t.separator)
        .child(el().w_full().h_full().pixels_fit(image.pixels(), rune::SurfaceFit::Fit).label("Pasted image"))
        .child(el().pin(None, Some(4.0), Some(4.0), None).size(22.0, 22.0).center().radius(11.0).fill(t.card)
            .role("button").label("Remove image").focusable(true).cursor(rune::Cursor::Pointer).hover(t.hover)
            .on_press(move |s: &mut State| { if i < s.pasted.len() { s.pasted.remove(i); } })
            .child(rune::icon(&CLOSE, 12.0).color(t.text)))
}

/// A picture pasted into the message box is attached to the draft; text pastes as usual.
#[cfg(feature = "image")]
fn draft_paste(input: El) -> El {
    input.on_input_paste(|s: &mut State, content: &rune::ClipboardContent, _| match content.image() {
        Some(png) => { paste_image(s, png); true }
        None => false,
    })
}

#[cfg(not(feature = "image"))]
fn draft_paste(input: El) -> El {
    input
}

fn loading_messages() -> El {
    static RING: rune::Icon = rune::Icon {
        name: "message-loader",
        view: 16.0,
        body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2" fill="none" opacity="0.2"/><path d="M8 2A6 6 0 0 1 13.258 10.89" stroke="currentColor" stroke-width="2" stroke-linecap="round" fill="none"/>"#,
    };
    rune::icon(&RING, 24.0).key("message-loader")
        .role("progressbar").label("Loading messages")
        .color(tok::theme(dark()).text_secondary).looping(rune::LoopSpec::rotation(0.9))
}

/// A message's text, its web links in the accent colour and opened in the browser on a click.
fn message_text(m: &Msg) -> El {
    let t = tok::theme(dark());
    let link = tok::accent(0, dark());
    #[cfg(feature = "markdown")]
    let (text, links, mut spans) = {
        // As the draft's preview showed it.
        let md = markdown::parse(&m.text);
        let links: Vec<_> = m.links.iter().map(|(r, url)| (md.at(r.start)..md.at(r.end), url)).filter(|(r, _)| !r.is_empty()).collect();
        let spans = md_spans(&md);
        (md.text, links, spans)
    };
    #[cfg(not(feature = "markdown"))]
    let (text, links, mut spans) = (m.text.clone(), m.links.iter().map(|(r, url)| (r.clone(), url)).collect::<Vec<_>>(), vec![]);
    spans.extend(links.iter().map(|(r, _)| rune::Span::new(r.start, r.end).color(link)));
    links.iter().fold(rune::rich(text, spans), |el, (r, url)| {
        el.link(rune::Link::new(r.clone(), url.as_str()).hover(rune::LinkPaint::new().underline(link)).help(url.as_str()))
    })
        // Long URLs have no spaces to wrap at.
        .font(type_::BODY).color(t.text).wrap_anywhere().selectable(true)
        .on_link(|_: &mut State, url: &str, tx: &mut Tx| { let _ = tx.open_url(url); })
}

#[cfg(feature = "markdown")]
fn md_spans(md: &markdown::Md) -> Vec<rune::Span> {
    use markdown::Style;
    md.spans.iter().map(|(r, style)| {
        let span = rune::Span::new(r.start, r.end);
        match style {
            Style::Bold | Style::Heading => span.bold(),
            Style::Italic => span.italic(),
            Style::Code => span.mono().bg(tok::theme(dark()).press, 2.0),
            Style::Block => span.mono(),
        }
    }).collect()
}

/// The draft as its bubble will show it.
#[cfg(feature = "markdown")]
fn preview_text(draft: &str) -> El {
    let md = markdown::parse(draft);
    let spans = md_spans(&md);
    rune::rich(md.text, spans)
}

#[cfg(not(feature = "markdown"))]
fn preview_text(draft: &str) -> El {
    text(draft)
}

/// The writing side's share after moving the divider `delta` points across `width`.
fn editor_split(start: f32, delta: f32, width: f32) -> f32 {
    if width <= 0.0 { return start }
    (start + delta / width).clamp(0.2, 0.9)
}

/// The draft in a pop-up editor: the raw text on the left and, when on, how it will read on the
/// right, split by a divider that drags. Escape or a click outside closes it; the draft stays.
fn draft_editor(s: &State) -> Option<El> {
    if !s.editor { return None }
    let t = tok::theme(dark());
    let input = draft_paste(text_input(&s.draft, type_::BODY).key("editor-draft").label("Message").color(t.text)
        .placeholder("Type a message", t.text_secondary)
        .caret(t.text, tok::accent(0, dark()).alpha(0.3))
        .autofocus()
        .on_edit(|s: &mut State| &mut s.draft)
        // Enter is a new line here; Send sends.
        .on_submit(|s: &mut State, _| s.draft.insert("\n")));
    let write = col().key("editor-write").h_full().min_w(0.0).padding(16.0).child(auto_grow(input, 1, 10_000));
    let body = row().w_full().grow(1.0).min_h(0.0).on_layout(|s: &mut State, [_, _, w, _]| s.editor_w = w);
    let body = if s.editor_preview {
        let divider = col().key("editor-resize").w(8.0).h_full().center().cursor(rune::Cursor::ResizeColumn)
            .label("Resize editor and preview with Left and Right arrows").focusable(true)
            .on_key(|s: &mut State, e: &rune::KeyEvent, _| {
                let delta = match e.key {
                    rune::Key::Left => -20.0,
                    rune::Key::Right => 20.0,
                    _ => return false,
                };
                s.editor_split = editor_split(s.editor_split, delta, s.editor_w);
                true
            })
            .on_drag(|s: &mut State, d: &mut Drag, _| {
                if d.phase == Phase::Down { d.data = s.editor_split; }
                s.editor_split = if d.cancelled() { d.data } else { editor_split(d.data, d.dx, s.editor_w) };
            })
            .child(el().w(1.0).h_full().fill(t.separator));
        // Both sides grow from zero in the split's ratio, so it holds before the width is known.
        body.child(write.w(0.0).grow(s.editor_split))
            .child(divider)
            .child(col().key("editor-preview").w(0.0).grow(1.0 - s.editor_split).min_w(160.0).h_full().padding(16.0).scroll(ScrollSpec { stick_bottom: false })
                .child(preview_text(&s.draft.text).font(type_::BODY).color(t.text).wrap_anywhere().selectable(true)))
    } else {
        body.child(write.grow(1.0))
    };
    // Centred and capped, so the chat stays in sight around it on a large window.
    Some(col().w_full().h_full().center().padding(64.0).fill(t.bg.alpha(0.7))
        .child(col().key("draft-editor").label("Editor").w_full().max_w(1000.0).h_full().max_h(700.0).min_h(0.0).radius(12.0).fill(t.card).border(1.0, t.separator)
            .modal(true, |s: &mut State| s.editor = false)
            .child(row().w_full().gap(8.0).px(16.0).py(10.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                .child(text("Editor").font(type_::TITLE).color(t.text).grow(1.0))
                .child(el().key("editor-preview-toggle").role("button").label("Preview").focusable(true).px(12.0).py(6.0).radius(6.0)
                    .fill(if s.editor_preview { t.press } else { t.control_off }).cursor(rune::Cursor::Pointer)
                    .on_press(|s: &mut State| s.editor_preview = !s.editor_preview)
                    .child(text(if s.editor_preview { "Preview on" } else { "Preview off" }).font(type_::LABEL).color(t.text)))
                .child(button("editor-close", "Close", false, |s| s.editor = false)))
            .child(body)
            .child(row().w_full().justify_end().px(16.0).py(10.0).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
                .child(button("editor-send", "Send", true, |s| { s.editor = false; send(s) })))))
}

fn main_view(s: &State, session: &Session, chats: &[Chat]) -> El {
    let t = tok::theme(dark());
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary);

    let me = col().gap(2.0).grow(1.0).min_w(0.0)
        .child(text(session.user.name.as_str()).font(type_::TITLE).color(t.text).truncate())
        .child(note(&session.user.email).truncate());
    let header = row().w_full().gap(10.0).padding(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(user_avatar(s, &session.user.id, &session.user.name, 36.0)).child(me)
        .child(el().key("sign-out").role("button").label("Sign out").focusable(true).px(10.0).py(6.0).radius(6.0)
            .fill(t.control_off).hover(t.hover).on_press(sign_out)
            .child(text("Sign out").font(type_::LABEL).color(t.text)));
    let chat_row = |c: &Chat, key: &str| {
        let id = c.id.clone();
        let picked = s.selected.as_ref() == Some(&c.id);
        let title_font = if c.unread { rune::tok::TextStyle::new(13.0, 700.0) } else { type_::BODY };
        row().key(format!("{key}/{}", c.id)).role("button").label(if c.unread { format!("{}, unread messages", c.title) } else { c.title.clone() }).focusable(true)
            .w_full().gap(10.0).px(if s.show_teams { 30.0 } else { 10.0 }).py(8.0).radius(8.0).items_center()
            .fill(if picked { t.press } else { t.card }).hover(t.hover)
            .on_press(move |s: &mut State| {
                s.search.clear();
                open_chat(s, id.clone());
            })
            .children((!s.show_teams).then(|| chat_avatar(s, c, &session.user.id, 36.0)))
            .child(col().gap(2.0).grow(1.0).min_w(0.0)
                .child(row().w_full().gap(8.0)
                    .child(text(c.title.as_str()).font(title_font).color(if c.unread { t.text } else { t.text_secondary }).truncate().grow(1.0).min_w(0.0))
                    .children((!c.time.is_empty()).then(|| text(&chat_time(&c.time, chrono::Local::now())).font(if c.unread { title_font } else { type_::LABEL }).color(if c.unread { t.text } else { t.text_secondary }))))
                .child(row().w_full().gap(8.0).items_center()
                    .children((!c.preview.is_empty()).then(|| text(&c.preview).font(if c.unread { type_::TITLE } else { type_::BODY }).color(if c.unread { t.text } else { t.text_secondary }).truncate().grow(1.0).min_w(0.0)))
                    .children(c.unread.then(|| el().size(8.0, 8.0).radius(4.0).fill(tok::accent(0, dark()))))))
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
                .caret(t.text, tok::accent(0, dark()).alpha(0.3))
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
        }))
        .child(el().grow(1.0))
        .child(theme_button(s.theme));

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
                // Nearby messages from one sender share an avatar, name and timestamp.
                let head = prev.is_none_or(|p| !same_message_group(p, m));
                let time = note(&message_time(&m.time, chrono::Local::now().date_naive())).font(type_::LABEL);
                let bubble = col().max_w(860.0).shrink().min_w(0.0).px(12.0).py(8.0).radius(12.0)
                    .fill(if m.mine { tok::accent(0, dark()).alpha(0.3) } else { t.card })
                    .border(2.0, tok::accent(0, dark()).alpha(if s.reply_highlight_on && s.reply_highlight.as_ref().is_some_and(|(id, _)| id == &m.id) { 1.0 } else { 0.0 }))
                    .children(m.reply.as_ref().map(|reply| {
                        let target = reply.id.clone();
                        // A set width: sized from its content, the quote measured its wrapped text at one width and drew it at another, spilling out of a short reply's bubble.
                        row().w(400.0).shrink().min_w(0.0).gap(10.0).padding(8.0).mb(6.0).radius(4.0).fill(t.bg)
                            .border(1.0, t.separator)
                            .cursor(rune::Cursor::Pointer).hover(t.hover)
                            .label("Go to quoted message").focusable(true)
                            .on_press_tx(move |s: &mut State, tx| {
                                s.reply_highlight = None;
                                s.reply_error = None;
                                s.reply_target = Some(target.clone());
                                reveal_reply(s, tx);
                            })
                            .child(el().w(2.0).radius(1.0).fill(t.text_secondary))
                            .child(col().gap(2.0).grow(1.0).shrink().min_w(0.0)
                                .child(note(&format!("{}  {}", reply.sender, local_time(&reply.time, "%Y-%m-%d %I:%M %p"))).font(type_::LABEL).truncate())
                                .child(text(reply.text.as_str()).font(type_::BODY).color(t.text).max_lines(2)))
                    }))
                    .children((!m.text.is_empty()).then(|| message_text(m)))
                    .children((!m.images.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0).mt(if m.text.is_empty() { 0.0 } else { 8.0 })
                        .children(m.images.iter().map(|url| message_image(s, url)))))
                    .children((!m.files.is_empty()).then(|| col().gap(8.0).mt(if m.text.is_empty() && m.images.is_empty() { 0.0 } else { 8.0 })
                        .children(m.files.iter().map(|f| file_card(s, f)))));
                // Keyed by id, not index: the 50-message window slides on a new message, and index keys would morph each bubble into its neighbour.
                let hover_id = m.id.clone();
                let line = row().key(format!("msg/{}", m.id)).hover_group_shared(format!("msg/{}", m.id)).arrives_whole()
                    .on_hover_change(move |s: &mut State, on, _| {
                        if on { s.hovered = Some(hover_id.clone()) } else if s.hovered.as_ref() == Some(&hover_id) { s.hovered = None }
                    }).w_full().gap(8.0).pt(if head { 12.0 } else { 0.0 });
                let line = if edge == Some(i) {
                    line.on_visible(|s: &mut State, on, _| if on { load_older(s) })
                } else {
                    line
                };
                // Only a message the server has: an edit needs its id and HTML.
                // Over the bubble's top-right corner while the message is hovered (or focused), as in Teams.
                // The toolbar reaches above the row, so it shares the row's hover: the pointer on it
                // keeps this message's actions, not the one above.
                let shown = s.menu.as_ref() == Some(&m.id) || s.hovered_bar.as_ref().or(s.hovered.as_ref()) == Some(&m.id);
                let bar_id = m.id.clone();
                let actions = message_actions(m, shown)
                    .map(|a| el().hover_group_shared(format!("msg/{}", m.id)).pin(None, Some(-30.0), Some(0.0), None)
                        .on_hover_change(move |s: &mut State, on, _| {
                            if on { s.hovered_bar = Some(bar_id.clone()) } else if s.hovered_bar.as_ref() == Some(&bar_id) { s.hovered_bar = None }
                        })
                        .child(a));
                let bubble = bubble.children(actions);
                let content = col().gap(4.0).shrink().min_w(0.0);
                let content = if m.mine { content.items_end() } else { content }
                    .children(head.then(|| row().gap(8.0).items_center()
                        .child(text(m.sender.as_str()).font(type_::LABEL).color(t.text_secondary))
                        .child(time)))
                    .child(bubble);
                let line = if m.mine {
                    line.justify_end().items_end().child(content)
                } else {
                    line.child(if head { user_avatar(s, &m.sender_id, &m.sender, 32.0) } else { el().w(32.0) })
                        .child(content)
                };
                let divider = new_day.then(|| row().key(format!("day/{}", m.id)).arrives_whole().w_full().justify_center().pt(10.0)
                    .child(note(&message_date(&m.time, chrono::Local::now().date_naive())).font(type_::LABEL)));
                divider.into_iter().chain([line])
            });
            pane.child(row().w_full().gap(10.0).px(16.0).py(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                    .child(chats.iter().find(|c| &c.id == id).map_or_else(|| avatar(title, 32.0), |c| chat_avatar(s, c, &session.user.id, 32.0)))
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
                    .child(row().w_full().gap(8.0).items_end()
                        .child(col().grow(1.0).shrink().min_w(0.0).gap(8.0).px(12.0).py(8.0).radius(8.0).fill(t.card)
                            .children((!s.pasted.is_empty()).then(|| row().w_full().flex_wrap().gap(8.0)
                                .children(s.pasted.iter().enumerate().map(|(i, (_, image))| pasted_image(image, i)))))
                            // One box shows the draft at a time; behind the editor it would echo every key.
                            .child(if s.editor { note("Writing in the editor…") } else {
                                auto_grow(draft_paste(text_input(&s.draft, type_::BODY).key("draft").label("Message").color(t.text)
                                    .placeholder("Type a message", t.text_secondary)
                                    .caret(t.text, tok::accent(0, dark()).alpha(0.3))
                                    .on_edit(|s: &mut State| &mut s.draft)
                                    .on_submit(|s: &mut State, _| if in_code_block(&s.draft.text[..s.draft.range().0]) { s.draft.insert("\n") } else { send(s) })), 1, 6)
                            }))
                        .child(button("editor", "Editor", false, |s| s.editor = true))
                        .child(button("send", "Send", true, send)))))
        }
    };
    col().w_full().h_full().fill(t.bg).child(search)
        .child(row().w_full().grow(1.0).min_h(0.0).child(rail).child(sidebar).child(divider).child(pane))
        .children(results)
}

/// The rail's theme button: each press steps System → Dark → Light → System.
fn theme_button(theme: Option<bool>) -> El {
    static SYSTEM: rune::Icon = rune::Icon {
        name: "theme-system", view: 24.0,
        body: r#"<rect x="3" y="4" width="18" height="12" rx="2" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 20h8 M12 16v4" fill="none" stroke="currentColor" stroke-width="1.5"/>"#,
    };
    static DARK_ICON: rune::Icon = rune::Icon {
        name: "theme-dark", view: 24.0,
        body: r#"<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>"#,
    };
    static LIGHT_ICON: rune::Icon = rune::Icon {
        name: "theme-light", view: 24.0,
        body: r#"<circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M12 2v2 M12 20v2 M2 12h2 M20 12h2 M4.9 4.9l1.4 1.4 M17.7 17.7l1.4 1.4 M4.9 19.1l1.4-1.4 M17.7 6.3l1.4-1.4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
    };
    let t = tok::theme(dark());
    let (icon, label) = match theme {
        None => (&SYSTEM, "System"),
        Some(true) => (&DARK_ICON, "Dark"),
        Some(false) => (&LIGHT_ICON, "Light"),
    };
    col().key("theme").role("button").label(format!("Theme: {label}")).focusable(true).w_full().center().gap(4.0).py(10.0).radius(6.0)
        .hover(t.hover).cursor(rune::Cursor::Pointer)
        .on_press(|s: &mut State| s.theme = next_theme(s.theme))
        .child(rune::icon(icon, 24.0).color(t.text))
        .child(text(label).font(type_::LABEL).color(t.text))
}

fn next_theme(theme: Option<bool>) -> Option<bool> {
    match theme {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

const PANEL_MAX: f32 = 480.0;

fn panel_width(start: f32, delta: f32) -> f32 {
    (start + delta).clamp(0.0, PANEL_MAX)
}

thread_local! {
    // Set once per frame by `view`, so helpers without the state paint in the picked theme.
    static DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn dark() -> bool {
    DARK.with(|d| d.get())
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    DARK.with(|d| d.set(s.theme.unwrap_or_else(|| cx.system_dark())));
    let t = tok::theme(dark());
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
        // Over the chat, not instead of it, so closing it leaves the messages scrolled where they were.
        Screen::Main { session, chats } => main_view(s, session, chats)
            .children(image_viewer(s).map(|v| v.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))))
            .children(draft_editor(s).map(|v| v.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))))
            .children(message_menu(s)),
    }
}

fn demo(s: &mut State) {
    let chat = |id: &str, title: &str, preview: &str, time: &str| Chat { id: id.into(), title: title.into(), avatar_id: String::new(), avatar_members: Vec::new(), preview: preview.into(), time: time.into(), unread: id == "b" };
    let msg = |sender: &str, text: &str, time: &str, mine| Msg { id: time.into(), server_id: time.into(), sender: sender.into(), sender_id: String::new(), text: text.into(), time: time.into(), mine, reply: None, images: vec![], links: vec![], files: vec![], raw: String::new() };
    s.screen = Screen::Main {
        session: Arc::new(Session {
            user: auth::User { name: "Ada Lovelace".into(), email: "ada@example.com".into(), id: "me".into() },
            ..Session::default()
        }),
        chats: vec![
            chat("a", "Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z"),
            Chat { avatar_members: ["Charles Babbage", "Ada Lovelace", "Mary Somerville"].into_iter().map(|name| auth::User { name: name.into(), ..Default::default() }).collect(), ..chat("b", "Design review", "Moved to Thursday", "2026-11-14T09:02:00Z") },
            chat("c", "Mary Somerville", "Thanks!", "2026-11-12T17:45:00Z"),
        ],
    };
    s.selected = Some("a".into());
    s.messages = Some(Ok(Page { older: None, msgs: vec![
        msg("Charles Babbage", "Did you get a chance to look at the notes?", "2026-11-14T10:20:00Z", false),
        msg("Ada Lovelace", "Yes. Note G is done: it computes the Bernoulli numbers.\nI'll send it tonight.", "2026-11-14T10:24:00Z", true),
        msg("Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z", false),
        msg("Charles Babbage", "And a faster carry.", "2026-11-14T10:32:00Z", false),
    ] }));
}

#[cfg(feature = "image")]
const IMAGE_DEMO: &str = "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-demo/views/imgo";

#[cfg(feature = "image")]
fn image_demo(s: &mut State) {
    demo(s);
    // Small enough (under 64 x 64) that rune decodes it on this thread, ready for the snapshot.
    let (w, h) = (48u32, 32u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, 180, 255]).collect();
    let image = rune::ImageCache::new(Arc::new(|| {})).rgba(w, h, px);
    s.images.insert(IMAGE_DEMO.into(), Some(image));
    if let Some(Ok(page)) = &mut s.messages {
        page.msgs[2].images = vec![IMAGE_DEMO.into(), "https://in-prod.asyncgw.teams.microsoft.com/v1/objects/0-loading/views/imgo".into()];
    }
}

/// A Loop page with a preview of its first page, and a Word file that isn't shared with us.
#[cfg(feature = "image")]
fn file_demo(s: &mut State) {
    demo(s);
    let file = |name: &str, kind: &str, url: &str| teams::File { name: name.into(), kind: kind.into(), url: url.into(), preview: teams::sharepoint_thumbnail(url) };
    let page = file("Engine Notes.loop", "loop", "https://contoso.sharepoint.com/:fl:/g/notes");
    let doc = file("Analytical Engine SLA.docx", "docx", "https://contoso-my.sharepoint.com/p/SLA.docx");
    // A page of grey text lines on white, small enough that rune decodes it on this thread.
    let w = 60u32;
    let px: Vec<u8> = (0..w * w).flat_map(|i| { let (x, y) = (i % w, i / w); if y % 6 == 3 && x > 4 && x < 30 + y % 25 { [90, 90, 90, 255] } else { [255; 4] } }).collect();
    s.images.insert(page.preview.clone().unwrap(), Some(rune::ImageCache::new(Arc::new(|| {})).rgba(w, w, px)));
    s.images.insert(doc.preview.clone().unwrap(), None);
    if let Some(Ok(p)) = &mut s.messages {
        p.msgs[0].files = vec![page];
        p.msgs[2].files = vec![doc];
    }
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
                    // A short answer: the quote must still widen the bubble.
                    page.msgs[1].text = "Well done".into();
                }
            }),
            ("link", |s| {
                demo(s);
                if let Some(Ok(page)) = &mut s.messages {
                    let (text, links) = teams::html_to_rich(r#"<p>Spec is up: <a href="https://example.com/verify?Context=650251703871&amp;X-Amz-Credential=AKIAZH53PVUYPP7M4GOK%2F20261109%2Fca-central-1%2Fses%2Faws4_request&amp;X-Amz-Signature=49758107ee3a9fab9dfe9f0ad5179b6e076d0736e4b6d9dede6f027c4f60e5c4">https://example.com/verify?Context=650251703871&amp;X-Amz-Credential=AKIAZH53PVUYPP7M4GOK%2F20261109%2Fca-central-1%2Fses%2Faws4_request&amp;X-Amz-Signature=49758107ee3a9fab9dfe9f0ad5179b6e076d0736e4b6d9dede6f027c4f60e5c4</a></p>"#);
                    (page.msgs[0].text, page.msgs[0].links) = (text, links);
                }
            }),
            #[cfg(feature = "markdown")]
            ("markdown", |s| {
                demo(s);
                if let Some(Ok(page)) = &mut s.messages {
                    page.msgs[0].text = teams::html_to_rich(r#"<pre class="language-markdown"><code>**Release&nbsp;checklist**<br><br>-&nbsp;Bump&nbsp;`version`&nbsp;in&nbsp;**Cargo.toml**<br>&nbsp;&nbsp;-&nbsp;then&nbsp;run&nbsp;*all*&nbsp;tests<br><br>```<br>cargo&nbsp;test&nbsp;--all-features<br>```<br><br>#&nbsp;Done</code></pre>"#).0;
                }
            }),
            ("edit", |s| {
                demo(s);
                if let Some(Ok(page)) = &s.messages && let Some(m) = page.msgs.iter().rev().find(|m| m.mine).cloned() {
                    s.draft = TextEdit::new(&format!("{} (edited)", m.text));
                    s.editing = Some(m);
                }
            }),
            ("menu", |s| {
                demo(s);
                s.menu = s.messages.as_ref().and_then(|r| r.as_ref().ok()).and_then(|p| p.msgs.iter().rev().find(|m| m.mine)).map(|m| m.id.clone());
            }),
            ("editor", |s| {
                demo(s);
                s.editor = true;
                s.draft = TextEdit::new("this is in plain text\n```markdown\n# Here Markdown Starts\n- with **bold** and `code`\n```\n```sh\ncargo test\n```");
            }),
            #[cfg(feature = "image")]
            ("file", file_demo),
            #[cfg(feature = "image")]
            ("image", image_demo),
            #[cfg(feature = "image")]
            ("viewer", |s| {
                image_demo(s);
                s.viewer = Some(IMAGE_DEMO.into());
            }),
            ("teams", |s| {
                demo(s);
                s.show_teams = true;
                s.selected = None;
                s.messages = None;
                let channel = |id: &str, title: &str| Chat {
                    avatar_id: String::new(), avatar_members: Vec::new(),
                    id: id.into(), title: title.into(), preview: String::new(), time: String::new(), unread: false,
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
        families: ("Inter", "ui-monospace"),
        ..AppSpec::new("Teams", (1000.0, 680.0), init, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_times_show_time_only_on_the_same_local_day() {
        use chrono::{Local, TimeZone};
        let now = Local.with_ymd_and_hms(2026, 11, 15, 0, 47, 0).earliest().unwrap();
        for (age, expected) in [
            (chrono::Duration::zero(), "12:47 AM"),
            (chrono::Duration::minutes(47), "12:00 AM"),
            (chrono::Duration::minutes(47) + chrono::Duration::seconds(1), "11/14"),
            (chrono::Duration::hours(1), "11/14"),
            (chrono::Duration::hours(24) - chrono::Duration::seconds(1), "11/14"),
            (chrono::Duration::hours(24), "11/14"),
            (chrono::Duration::days(8), "11/7"),
        ] {
            let iso = (now - age).with_timezone(&chrono::Utc).to_rfc3339();
            assert_eq!(chat_time(&iso, now), expected);
        }
        assert_eq!(chat_time("", now), "");
        assert_eq!(chat_time("invalid", now), "invalid");
    }

    #[test]
    fn message_dates_use_relative_days_then_full_dates() {
        use chrono::{Local, TimeZone};
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        for (days, expected) in [
            (0, "Today"), (1, "Yesterday"), (2, "Friday"),
            (6, "Monday"), (7, "Sunday"), (8, "Saturday, November 7"),
            (15, "Saturday, October 31"), (-1, "Monday, November 16"),
        ] {
            let date = today - chrono::Duration::days(days);
            let local = Local.from_local_datetime(&date.and_hms_opt(0, 5, 0).unwrap()).earliest().unwrap();
            assert_eq!(message_date(&local.with_timezone(&chrono::Utc).to_rfc3339(), today), expected);
        }
        assert_eq!(message_date("", today), "");
        assert_eq!(message_date("invalid", today), "invalid");
    }

    #[test]
    fn sender_times_use_relative_dates_then_numeric_dates() {
        use chrono::{Local, TimeZone};
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        for (days, expected) in [
            (0, "Today 12:04 AM"), (1, "Yesterday 12:04 AM"),
            (2, "Friday 12:04 AM"), (7, "Sunday 12:04 AM"),
            (8, "11/7 12:04 AM"),
        ] {
            let date = today - chrono::Duration::days(days);
            let local = Local.from_local_datetime(&date.and_hms_opt(0, 4, 0).unwrap()).earliest().unwrap();
            assert_eq!(message_time(&local.with_timezone(&chrono::Utc).to_rfc3339(), today), expected);
        }
        assert_eq!(message_time("", today), "");
        assert_eq!(message_time("invalid", today), "invalid");
    }

    #[test]
    fn message_groups_require_same_sender_day_and_short_gap() {
        let mut s = init();
        demo(&mut s);
        let original = &s.messages.as_ref().unwrap().as_ref().unwrap().msgs[0];
        let mut previous = original.clone();
        let mut current = original.clone();
        previous.time = "2026-11-14T10:00:00Z".into();
        current.time = "2026-11-14T10:04:59Z".into();
        assert!(same_message_group(&previous, &current));
        current.time = "2026-11-14T10:05:00Z".into();
        assert!(!same_message_group(&previous, &current));
        current.time = previous.time.clone();
        current.sender = "Someone else".into();
        assert!(!same_message_group(&previous, &current));
        current.sender = previous.sender.clone();
        current.mine = !previous.mine;
        assert!(!same_message_group(&previous, &current));
        current.mine = previous.mine;
        current.time = "invalid".into();
        assert!(!same_message_group(&previous, &current));
        current.time = "2026-11-14T09:59:59Z".into();
        assert!(!same_message_group(&previous, &current));
        use chrono::{Local, TimeZone};
        let midnight = Local.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).earliest().unwrap();
        previous.time = (midnight - chrono::Duration::seconds(1)).to_rfc3339();
        current.time = midnight.to_rfc3339();
        assert!(!same_message_group(&previous, &current));
    }

    #[test]
    fn timestamps_use_local_dates_and_twelve_hour_time() {
        use chrono::{Local, TimeZone};
        for (hour, label) in [(0, "12:05 AM"), (12, "12:05 PM"), (23, "11:05 PM")] {
            let local = Local.with_ymd_and_hms(2026, 1, 15, hour, 5, 0).earliest().unwrap();
            let iso = local.with_timezone(&chrono::Utc).to_rfc3339();
            assert_eq!(local_time(&iso, "%I:%M %p"), label);
            assert_eq!(day(&iso), "2026-01-15");
        }
        assert_eq!(local_time("", "%I:%M %p"), "");
        assert_eq!(local_time("invalid", "%I:%M %p"), "invalid");
    }

    #[test]
    fn editor_split_moves_by_share_of_width_and_stays_in_bounds() {
        assert_eq!(super::editor_split(0.75, -100.0, 1000.0), 0.65);
        assert_eq!(super::editor_split(0.75, 500.0, 1000.0), 0.9);
        assert_eq!(super::editor_split(0.75, -900.0, 1000.0), 0.2);
        assert_eq!(super::editor_split(0.75, 50.0, 0.0), 0.75);
    }

    #[test]
    fn enter_types_a_line_inside_an_open_code_block() {
        assert!(!super::in_code_block("plain text"));
        assert!(super::in_code_block("text\n```markdown\n# Heading"));
        assert!(super::in_code_block("```"));
        assert!(!super::in_code_block("```sh\nls\n```"));
    }

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
    fn theme_cycles_system_dark_light() {
        assert_eq!(next_theme(None), Some(true));
        assert_eq!(next_theme(Some(true)), Some(false));
        assert_eq!(next_theme(Some(false)), None);
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

#[cfg(test)]
mod avatar_tests {
    use super::{chat_avatar_id, Chat};

    #[test]
    fn self_chat_uses_own_photo() {
        let chat = |id: &str| Chat { id: id.into(), avatar_id: String::new(), avatar_members: Vec::new(), title: String::new(), preview: String::new(), time: String::new(), unread: false };
        let mut bot = chat("19:me_bot@unq.gbl.spaces");
        bot.avatar_id = "28:bot".into();
        assert_eq!(chat_avatar_id(&bot, "me"), "28:bot");
        assert_eq!(chat_avatar_id(&chat("48:notes"), "me"), "me");
        assert_eq!(chat_avatar_id(&chat("19:me_other@unq.gbl.spaces"), "me"), "other");
        assert_eq!(chat_avatar_id(&chat("19:group@thread.v2"), "me"), "");
    }
}
