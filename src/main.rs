
//! A Teams desktop client: device-code sign-in that lasts across launches, then your chats.

mod actions;
mod history;
mod auth;
mod avatars;
mod demo;
#[cfg(feature = "image")]
mod images;
#[cfg(feature = "markdown")]
mod markdown;
mod teams;
mod notifications;
mod message_state;
mod releases;
mod ui;

use actions::*;
use demo::*;
use auth::Session;
use rune::tok::{self, type_};
use rune::{Align, AppSpec, Cx, El, GrowthSpec, Side, Drag, Latest, Live, Phase, ScrollPos, ScrollSpec, Spring, TextEdit, Tx, Waker, WheelEvent, auto_grow, col, el, row, text, text_input};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use teams::{Chat, Msg, Page};
use ui::*;

enum Screen {
    Loading,
    /// Signed out, with why the last attempt failed.
    Login(Option<String>),
    Code { user_code: String, url: String },
    Main { session: Arc<Session>, chats: Vec<Chat> },
}

type Messages = Result<Page, String>;

struct State {
    message_state: message_state::MessageState,
    release_loader: Latest<Result<Option<releases::Release>, String>>,
    release_status: releases::Status,
    release_panel: bool,
    release_checked_at: Option<chrono::DateTime<chrono::Local>>,
    release_open_error: bool,
    screen: Screen,
    avatars: avatars::Avatars,
    #[cfg(feature = "image")]
    images: images::Images,
    #[cfg(feature = "image")]
    preview_rows: std::collections::HashMap<String, [f32; 4]>,
    #[cfg(feature = "image")]
    preview_viewport: Live<[f32; 4]>,
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
    pending_channels: Vec<Chat>,
    teams_loader: Latest<Result<Vec<teams::Team>, String>>,
    collapsed_teams: std::collections::HashSet<String>,
    /// Bumped on every sign-in attempt, cancel and sign-out; a worker from an older one is ignored.
    attempt: Arc<AtomicU64>,
    events: (mpsc::Sender<(u64, Screen)>, mpsc::Receiver<(u64, Screen)>),
    waker: Option<Waker>,
    updates: (mpsc::SyncSender<(u64, teams::events::Update)>, mpsc::Receiver<(u64, teams::events::Update)>),
    live_attempt: Option<u64>,
    live_status: Option<String>,
    notifications: Option<mpsc::SyncSender<(u64, String, String, String)>>,
    window: Option<rune::AppHandle<State>>,
    window_focused: bool,
    window_visible: bool,
    reader: Latest<(String, teams::ReadHorizon, Result<(), String>)>,
    read_in_flight: bool,
    live_baseline: std::collections::HashMap<String, String>,
    delivered: std::collections::VecDeque<(String, String)>,
    loading_updates: Vec<(String, Option<Msg>)>,
    selected: Option<String>,
    /// The selected chat's messages; `None` while they load.
    messages: Option<Messages>,
    loader: Latest<(String, Messages)>,
    history: history::History,
    restore_scroll: Option<ScrollPos>,
    prefetch: Latest<(String, Messages)>,
    prefetch_queue: std::collections::VecDeque<String>,
    prefetch_id: Option<String>,
    /// A fetch of the page before the loaded ones: the chat, the link it followed, the page.
    older_loader: Latest<(String, String, Messages)>,
    /// The message being written to the selected chat.
    draft: TextEdit,
    mentions: MentionPicker,
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
    sender: Latest<(String, String, Result<(Messages, Vec<String>), (String, TextEdit, Vec<(rune::ClipboardImage, rune::Image)>, (Option<Msg>, Option<Msg>))>)>,
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
    /// A delete in flight: the chat, the original message, and how it went.
    deleter: Latest<(String, Msg, Result<(), String>)>,
    reactor: Latest<(String, String, String, bool, Result<(), String>)>,
    send_error: Option<String>,
    /// Filters the chat list by name.
    search: TextEdit,
    /// Where the chat list's and the messages' scroll thumbs are drawn.
    chat_bar: Bar,
    msg_bar: Bar,
}

fn init() -> State {
    State {
        message_state: Default::default(),
        release_loader: Latest::default(),
        release_status: releases::Status::Idle,
        release_panel: false,
        release_checked_at: None,
        release_open_error: false,
        screen: Screen::Loading,
        avatars: Default::default(),
        #[cfg(feature = "image")]
        images: Default::default(),
        #[cfg(feature = "image")]
        preview_rows: Default::default(),
        #[cfg(feature = "image")]
        preview_viewport: Live::new([0.0; 4]),
        panel_width: 320.0,
        theme: None,
        viewer: None,
        reply_target: None,
        reply_error: None,
        reply_highlight: None,
        reply_highlight_on: false,
        show_teams: false,
        teams: None,
        teams_loader: Latest::default(),
        pending_channels: Vec::new(),
        collapsed_teams: Default::default(),
        attempt: Arc::default(),
        events: mpsc::channel(),
        waker: None,
        updates: mpsc::sync_channel(128),
        live_attempt: None,
        live_status: None,
        notifications: None,
        window: None,
        window_focused: false,
        window_visible: true,
        reader: Latest::default(),
        read_in_flight: false,
        live_baseline: Default::default(),
        delivered: Default::default(),
        loading_updates: Vec::new(),
        selected: None,
        messages: None,
        loader: Latest::default(),
        history: Default::default(),
        restore_scroll: None,
        prefetch: Latest::default(),
        prefetch_queue: Default::default(),
        prefetch_id: None,
        older_loader: Latest::default(),
        draft: TextEdit::new(""),
        mentions: Default::default(),
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
        reactor: Latest::default(),
        send_error: None,
        search: TextEdit::new(""),
        chat_bar: Bar::default(),
        msg_bar: Bar::default(),
    }
}

fn setup(s: &mut State, waker: Waker) {
    s.message_state.receipts.set_waker(waker.clone());
    s.message_state.restorer.set_waker(waker.clone());
    s.teams_loader.set_waker(waker.clone());
    s.loader.set_waker(waker.clone());
    s.prefetch.set_waker(waker.clone());
    s.sender.set_waker(waker.clone());
    s.mentions.setup(waker.clone());
    s.older_loader.set_waker(waker.clone());
    s.deleter.set_waker(waker.clone());
    s.reactor.set_waker(waker.clone());
    s.reader.set_waker(waker.clone());
    s.release_loader.set_waker(waker.clone());
    s.waker = Some(waker);
    // The app has no arguments of its own; any argument is Rune's headless tooling, which gets no network.
    if !cfg!(test) && std::env::args().len() == 1 {
        notifications::start(s);
        sign_in(s, false);
    }
}

fn reply_blink(elapsed: f32) -> Option<bool> {
    (elapsed < 1.8).then(|| (elapsed / 0.3) as u32 % 2 == 0)
}

fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    let mut changed = releases::poll(s);
    changed |= s.mentions.poll(&s.selected);
    if let Some((_, started)) = &s.reply_highlight {
        if let Some(on) = reply_blink(started.elapsed().as_secs_f32()) {
            changed |= s.reply_highlight_on != on;
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
        if let (Screen::Main { chats: old, .. }, Screen::Main { chats: named, .. }) = (&mut s.screen, &screen) {
            for chat in old {
                if let Some(named) = named.iter().find(|c| c.id == chat.id) {
                    chat.title = named.title.clone();
                    chat.avatar_id = named.avatar_id.clone();
                    chat.avatar_members = named.avatar_members.clone();
                }
            }
        } else {
            if let Screen::Main { chats, .. } = &screen {
                s.prefetch_queue = chats.iter().take(10).map(|c| c.id.clone()).collect();
            }
            s.screen = screen;
        }
        changed = true;
    }
    if !cfg!(test) && std::env::args().len() == 1 && let Screen::Main { session, chats } = &s.screen {
        let mine = s.attempt.load(SeqCst);
        if s.live_attempt != Some(mine) {
            s.live_attempt = Some(mine);
            s.live_baseline = chats.iter().map(|c| (c.id.clone(), c.time.clone())).collect();
            teams::events::listen(session.clone(), s.attempt.clone(), mine, s.updates.0.clone(), s.waker.clone().unwrap());
        }
    }
    if let Some(result) = s.teams_loader.take() {
        notifications::set_teams(s, result);
        changed = true;
    }
    if let Some((id, r)) = s.loader.take() {
        if s.selected.as_ref() == Some(&id) {
            if let Ok(page) = &r { history::cache_page(s, id.clone(), page.clone()); }
            s.messages = Some(match (s.messages.take(), r) {
                (Some(Ok(pushed)), Ok(loaded)) => Ok(teams::merge(pushed, loaded)),
                (Some(Ok(cached)), Err(error)) => {
                    s.live_status = Some(format!("Could not refresh messages: {error}"));
                    Ok(cached)
                }
                (_, result) => result,
            });
            if let Some(Ok(page)) = &mut s.messages {
                for (server_id, message) in s.loading_updates.drain(..) {
                    if let Some(cached) = s.history.page_mut(&id) {
                        notifications::apply_message(cached, &server_id, message.clone());
                    }
                    notifications::apply_message(page, &server_id, message);
                }
            } else { s.loading_updates.clear(); }
            notifications::mark_selected_read(s);
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
                // History must land before wheel input can release the anchor during growth.
                tx.instant();
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
    if let Some((id, deleted, r)) = s.deleter.take() {
        history::invalidate(s, &id);
        match r {
            Ok(()) => if let (true, Some(Ok(page))) = (s.selected.as_ref() == Some(&id), &mut s.messages) {
                let mut deleted = deleted;
                message_state::tombstone(&mut deleted);
                let server_id = deleted.server_id.clone();
                notifications::apply_message(page, &server_id, Some(deleted));
                s.loader.cancel();
            },
            Err(e) => s.send_error = Some(format!("Not deleted: {e}")),
        }
        changed = true;
    }
    if let Some((id, msg_id, r)) = s.sender.take() {
        history::invalidate(s, &id);
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
        if let Ok(Ok(page)) = &r { history::cache_page(s, id.clone(), page.clone()); }
        match r {
            Ok(Ok(fresh)) if here => {
                s.loader.cancel();
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
                    s.draft = text;
                }
                if here && s.pasted.is_empty() {
                    s.pasted = pasted;
                }
            }
        }
        changed = true;
    }
    changed |= notifications::poll(s);
    if let Some((chat, message, key, remove, result)) = s.reactor.take() {
        match result {
            Ok(()) => {
                let me = match &s.screen { Screen::Main { session, .. } => session.user.id.clone(), _ => String::new() };
                if s.selected.as_ref() == Some(&chat) && let Some(Ok(page)) = &mut s.messages {
                    apply_reaction(page, &message, &key, &me, remove);
                }
                if let Some(page) = s.history.page_mut(&chat) { apply_reaction(page, &message, &key, &me, remove); }
            }
            Err(error) => s.send_error = Some(format!("Reaction not changed: {error}")),
        }
        changed = true;
    }
    changed |= message_state::poll(s, tx);
    history::poll(s);
    if !cfg!(test) && std::env::args().len() == 1 && let Screen::Main { session, chats } = &s.screen {
        let ids = std::iter::once(session.user.id.clone())
            .chain(chats.iter().map(|c| chat_avatar_id(c, &session.user.id).to_owned()))
            .chain(chats.iter().flat_map(|c| c.avatar_members.iter().take(3).map(|m| m.id.clone())))
            .chain(s.messages.as_ref().and_then(|r| r.as_ref().ok()).into_iter().flat_map(|p| p.msgs.iter().map(|m| m.sender_id.clone())));
        changed |= s.avatars.poll(session, ids, s.waker.clone().unwrap());
        #[cfg(feature = "image")]
        {
            if changed {
                let page = s.messages.as_ref().and_then(|r| r.as_ref().ok());
                s.preview_rows.retain(|id, _| page.is_some_and(|p| p.msgs.iter().any(|m| &m.id == id)));
            }
            s.images.keep_full(s.viewer.as_deref());
            // The open picture's full size first, so it isn't queued behind the chat's previews.
            let urls = s.viewer.iter().map(|u| images::full(u)).chain(nearby_previews(s)).collect::<Vec<_>>();
            changed |= s.images.poll(session, urls.into_iter(), s.waker.clone().unwrap());
        }
        tx.wake_after(std::time::Duration::from_secs(60));
    }
    let revealed = reveal_reply(s, tx);
    changed || revealed
}

thread_local! {
    // Set once per frame by `view`, so helpers without the state paint in the picked theme.
    static DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn dark() -> bool {
    DARK.with(|d| d.get())
}

fn rest(s: &mut State, _: &mut Tx) -> bool {
    #[cfg(feature = "image")]
    {
        let urls = nearby_previews(s).chain(s.viewer.iter().cloned())
            .chain(s.viewer.iter().map(|u| images::full(u))).collect();
        s.images.retain(&urls, s.sender.is_pending());
    }
    let _ = s;
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" { fn malloc_trim(pad: usize) -> std::ffi::c_int; }
        // glibc can retain freed image/layout pages after Rune has cleaned up its idle caches.
        unsafe { malloc_trim(0); }
    }
    false
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    DARK.with(|d| d.set(s.theme.unwrap_or_else(|| cx.system_dark())));
    let t = tok::theme(dark());
    let title = |s: &str| text(s).font(type_::TITLE).color(t.text);
    let note = |s: &str| text(s).font(type_::BODY).color(t.text_secondary).wrap();
    let page = col().w_full().h_full().center().gap(14.0).padding(32.0).fill(t.bg);
    let content = match &s.screen {
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
    };
    col().w_full().h_full().child(content.grow(1.0).min_h(0.0))
        .children((!matches!(s.screen, Screen::Main { .. })).then(||
            releases::nav_button(s).w(44.0).pin(Some(12.0), Some(12.0), None, None)))
        .children(releases::panel(s))
}

fn main() {
    // Linux takes the icon from assets/linux/ms-teams-agent.desktop, matched by this id; macOS and Windows from icon.png.
    rune::set_app_id("ms-teams-agent");
    rune::set_app_icon(include_bytes!("../assets/icons/icon.png").as_slice()).expect("icon.png is a PNG");
    let desktop = std::env::args().len() == 1;
    if desktop && !rune::single_instance("ms-teams-agent") { return; }
    let mut app = rune::App::new(AppSpec::<State> {
        scenes: SCENES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(search_key),
        min_size: Some((640.0, 400.0)),
        families: ("Inter", "ui-monospace"),
        ..AppSpec::new("Teams", (1000.0, 680.0), init, view)
    }).on_focus(notifications::focus).on_visibility(notifications::visibility).on_rest(rest)
        .on_close(|s, _| { s.window_visible = false; s.window_focused = false; })
        .on_quit(|s, _| { s.attempt.fetch_add(1, SeqCst); });
    let handle = app.handle();
    app = app.init(move || {
        let mut s = init();
        s.window = Some(handle);
        s
    });
    let tray = if desktop { notifications::tray(app.handle()).map_err(|e| eprintln!("Teams tray unavailable: {e}")).ok() } else { None };
    if desktop { app = app.keep_running().release_gpu_when_closed().standard_menus(); }
    app.run();
    drop(tray);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_refresh_preserves_cached_data_on_failure_and_replays_inflight_changes() {
        let mut s = init();
        demo(&mut s);
        let page = s.messages.as_ref().unwrap().as_ref().unwrap().clone();
        let deleted = page.msgs[0].server_id.clone();
        s.loading_updates.push((deleted.clone(), None));
        s.loader.request((), move |_| ("a".into(), Ok(page)));
        let mut tx = Tx::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while s.loader.is_pending() {
            poll(&mut s, 0.0, &mut tx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        poll(&mut s, 0.0, &mut tx);
        assert!(!s.history.get("a").unwrap().msgs.iter().any(|m| m.server_id == deleted));
        let count = s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len();
        s.loader.request((), |_| ("a".into(), Err("offline".into())));
        while s.loader.is_pending() {
            poll(&mut s, 0.0, &mut tx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        poll(&mut s, 0.0, &mut tx);
        assert_eq!(s.messages.as_ref().unwrap().as_ref().unwrap().msgs.len(), count);
        assert!(s.live_status.as_deref().unwrap().contains("offline"));
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
}
