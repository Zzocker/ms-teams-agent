//! A Teams desktop client: device-code sign-in that lasts across launches, then your chats.

mod auth;
mod teams;

use auth::Session;
use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, Latest, ScrollSpec, TextEdit, Tx, Waker, auto_grow, col, el, row, text, text_input};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, mpsc};
use teams::{Chat, Msg};

enum Screen {
    Loading,
    /// Signed out, with why the last attempt failed.
    Login(Option<String>),
    Code { user_code: String, url: String },
    Main { session: Arc<Session>, chats: Vec<Chat> },
}

type Messages = Result<Vec<Msg>, String>;

struct State {
    screen: Screen,
    /// Bumped on every sign-in attempt, cancel and sign-out; a worker from an older one is ignored.
    attempt: Arc<AtomicU64>,
    events: (mpsc::Sender<(u64, Screen)>, mpsc::Receiver<(u64, Screen)>),
    waker: Option<Waker>,
    selected: Option<String>,
    /// The selected chat's messages; `None` while they load.
    messages: Option<Messages>,
    loader: Latest<(String, Messages)>,
    /// The message being written to the selected chat.
    draft: TextEdit,
    /// A send in flight: the chat, then its reloaded messages, or the error and the unsent text.
    sender: Latest<(String, Result<Messages, (String, String)>)>,
    send_error: Option<String>,
    /// Filters the chat list by name.
    search: TextEdit,
}

fn init() -> State {
    State {
        screen: Screen::Loading,
        attempt: Arc::default(),
        events: mpsc::channel(),
        waker: None,
        selected: None,
        messages: None,
        loader: Latest::default(),
        draft: TextEdit::new(""),
        sender: Latest::default(),
        send_error: None,
        search: TextEdit::new(""),
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
    s.attempt.fetch_add(1, SeqCst);
    s.loader.cancel();
    s.sender.cancel();
    s.draft.clear();
    s.search.clear();
    s.send_error = None;
    auth::sign_out();
    (s.selected, s.messages) = (None, None);
    s.screen = Screen::Login(None);
}

fn open_chat(s: &mut State, id: String) {
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

/// Sends the draft to the selected chat, then reloads its messages.
fn send(s: &mut State) {
    let (Screen::Main { session, .. }, Some(id)) = (&s.screen, &s.selected) else { return };
    let text = s.draft.text.trim().to_string();
    // One send at a time: `Latest` would drop the first one's answer.
    if text.is_empty() || s.sender.is_pending() {
        return;
    }
    let session = session.clone();
    let id = id.clone();
    s.draft.clear();
    s.send_error = None;
    s.sender.request((id, text), move |(id, text)| {
        let r = match teams::send(&session, &id, &text) {
            Ok(()) => Ok(teams::messages(&session, &id)),
            Err(e) => Err((e, text)),
        };
        (id, r)
    });
}

fn setup(s: &mut State, waker: Waker) {
    s.loader.set_waker(waker.clone());
    s.sender.set_waker(waker.clone());
    s.waker = Some(waker);
    // The app has no arguments of its own; any argument is Rune's headless tooling, which gets no network.
    if std::env::args().len() == 1 {
        sign_in(s, false);
    }
}

fn poll(s: &mut State, _: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
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
    if let Some((id, r)) = s.loader.take() {
        if s.selected.as_ref() == Some(&id) {
            s.messages = Some(r);
            changed = true;
        }
    }
    if let Some((id, r)) = s.sender.take() {
        let here = s.selected.as_ref() == Some(&id);
        match r {
            Ok(msgs) if here => s.messages = Some(msgs),
            Ok(_) => {}
            Err((e, text)) => {
                s.send_error = Some(format!("Not sent: {e}"));
                // Give the text back, unless the user moved on to another chat or draft.
                if here && s.draft.text.is_empty() {
                    s.draft.replace_text(text);
                }
            }
        }
        changed = true;
    }
    changed
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
    let query = s.search.text.trim().to_lowercase();
    let rows = chats.iter().filter(|c| c.title.to_lowercase().contains(&query)).map(|c| {
        let id = c.id.clone();
        let picked = s.selected.as_ref() == Some(&c.id);
        row().key(format!("chat/{}", c.id)).role("button").label(c.title.as_str()).focusable(true)
            .w_full().gap(10.0).px(10.0).py(8.0).radius(8.0).items_center()
            .fill(if picked { t.press } else { t.card }).hover(t.hover)
            .on_press(move |s: &mut State| open_chat(s, id.clone()))
            .child(avatar(&c.title, 36.0))
            .child(col().gap(2.0).grow(1.0).min_w(0.0)
                .child(row().w_full().gap(8.0)
                    .child(text(c.title.as_str()).font(type_::TITLE).color(t.text).truncate().grow(1.0).min_w(0.0))
                    .child(note(&short_time(&c.time)).font(type_::LABEL)))
                .child(note(&c.preview).truncate()))
    });
    let search = col().mx(10.0).mt(10.0).px(12.0).py(8.0).radius(8.0).fill(t.bg)
        .child(text_input(&s.search, type_::BODY).key("search").label("Search chats").color(t.text)
            .placeholder("Search chats", t.text_secondary)
            .caret(t.text, tok::accent(0, true).alpha(0.3))
            .on_edit(|s: &mut State| &mut s.search));
    let list = col().key("chats").w_full().gap(2.0).padding(6.0).children(rows)
        .scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0);
    let sidebar = col().w(300.0).h_full().min_h(0.0).fill(t.card).border_sides([0.0, 1.0, 0.0, 0.0], t.separator)
        .child(header).child(search).child(list);

    let pane = col().grow(1.0).shrink().h_full().min_w(0.0).min_h(0.0).fill(t.bg);
    let pane = match (&s.selected, &s.messages) {
        (None, _) => pane.center().child(note(if chats.is_empty() { "No chats yet." } else { "Pick a chat." })),
        (Some(_), None) => pane.center().child(note("Loading messages…")),
        (Some(_), Some(Err(e))) => pane.center().padding(32.0).child(note(e).wrap()),
        (Some(id), Some(Ok(msgs))) => {
            let title = chats.iter().find(|c| &c.id == id).map_or("Chat", |c| c.title.as_str());
            let bubbles = msgs.iter().enumerate().flat_map(|(i, m)| {
                let prev = i.checked_sub(1).map(|j| &msgs[j]);
                let new_day = prev.is_none_or(|p| day(&p.time) != day(&m.time));
                // A run of messages from one sender shares one avatar and name.
                let head = new_day || prev.is_some_and(|p| p.sender != m.sender || p.mine != m.mine);
                let time = note(m.time.get(11..16).unwrap_or(&m.time)).font(type_::LABEL);
                let bubble = col().max_w(520.0).shrink().min_w(0.0).px(12.0).py(8.0).radius(12.0)
                    .fill(if m.mine { tok::accent(0, true).alpha(0.3) } else { t.card })
                    .child(text(m.text.as_str()).font(type_::BODY).color(t.text).wrap().selectable(true));
                let line = row().key(format!("msg/{i}")).w_full().gap(8.0).mt(if head { 10.0 } else { 0.0 });
                let line = if m.mine {
                    line.justify_end().items_end().child(time).child(bubble)
                } else {
                    line.child(if head { avatar(&m.sender, 32.0) } else { el().w(32.0) })
                        .child(col().gap(4.0).grow(1.0).shrink().min_w(0.0)
                            .children(head.then(|| text(m.sender.as_str()).font(type_::LABEL).color(t.text_secondary)))
                            .child(row().gap(8.0).shrink().min_w(0.0).items_end().child(bubble).child(time)))
                };
                let divider = new_day.then(|| row().key(format!("day/{i}")).w_full().justify_center().mt(10.0)
                    .child(note(day(&m.time)).font(type_::LABEL)));
                divider.into_iter().chain([line])
            });
            pane.child(row().w_full().gap(10.0).px(16.0).py(12.0).items_center().border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
                    .child(avatar(title, 32.0))
                    .child(text(title).font(type_::TITLE).color(t.text).truncate()))
                .child(col().key("messages").w_full().gap(4.0).px(16.0).py(8.0)
                    .children(bubbles)
                    .children(msgs.is_empty().then(|| note("No messages.")))
                    .scroll(ScrollSpec { stick_bottom: true }).grow(1.0).min_h(0.0))
                .child(col().w_full().gap(6.0).px(16.0).py(10.0).border_sides([1.0, 0.0, 0.0, 0.0], t.separator)
                    .children(s.send_error.as_deref().map(|e| note(e).wrap()))
                    .child(row().w_full().gap(8.0).items_end()
                        .child(col().grow(1.0).shrink().min_w(0.0).px(12.0).py(8.0).radius(8.0).fill(t.card)
                            .child(auto_grow(text_input(&s.draft, type_::BODY).key("draft").label("Message").color(t.text)
                                .placeholder("Type a message", t.text_secondary)
                                .caret(t.text, tok::accent(0, true).alpha(0.3))
                                .on_edit(|s: &mut State| &mut s.draft)
                                .on_submit(|s: &mut State, _| send(s)), 1, 6)))
                        .child(button("send", "Send", true, send))))
        }
    };
    row().w_full().h_full().fill(t.bg).child(sidebar).child(pane)
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
    let msg = |sender: &str, text: &str, time: &str, mine| Msg { sender: sender.into(), text: text.into(), time: time.into(), mine };
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
    s.messages = Some(Ok(vec![
        msg("Charles Babbage", "Did you get a chance to look at the notes?", "2026-11-14T10:20:00Z", false),
        msg("Ada Lovelace", "Yes. Note G is done: it computes the Bernoulli numbers.\nI'll send it tonight.", "2026-11-14T10:24:00Z", true),
        msg("Charles Babbage", "The engine needs another column of gears, I think", "2026-11-14T10:31:00Z", false),
    ]));
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("login", |s| s.screen = Screen::Login(None)),
            ("error", |s| s.screen = Screen::Login(Some("chat service 403: Forbidden".into()))),
            ("code", |s| s.screen = Screen::Code { user_code: "ABCD1234".into(), url: "https://microsoft.com/devicelogin".into() }),
            ("main", demo),
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
