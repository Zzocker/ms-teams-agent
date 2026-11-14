//! Phase 1: sign in with a device code, stay signed in across launches, show who is signed in.

mod auth;

use auth::User;
use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, Tx, Waker, col, el, text};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, mpsc};

enum Screen {
    Loading,
    /// Signed out, with why the last attempt failed.
    Login(Option<String>),
    Code { user_code: String, url: String },
    Profile(User),
}

struct State {
    screen: Screen,
    /// Bumped on every sign-in attempt, cancel and sign-out; a worker from an older one is ignored.
    attempt: Arc<AtomicU64>,
    events: (mpsc::Sender<(u64, Screen)>, mpsc::Receiver<(u64, Screen)>),
    waker: Option<Waker>,
}

fn init() -> State {
    State { screen: Screen::Loading, attempt: Arc::default(), events: mpsc::channel(), waker: None }
}

/// Signs in off the UI thread: from the stored session, else (when `interactive`) with a device code.
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
        let user = match auth::resume() {
            Ok(None) if interactive => {
                auth::device_login(|user_code, url| emit(Screen::Code { user_code, url }), || attempt.load(SeqCst) == mine).map(Some)
            }
            r => r,
        };
        emit(match user {
            Ok(Some(u)) => Screen::Profile(u),
            Ok(None) => Screen::Login(None),
            Err(e) => Screen::Login(Some(e)),
        });
    });
}

fn setup(s: &mut State, waker: Waker) {
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
    changed
}

fn button(key: &'static str, label: &'static str, primary: bool, on_press: fn(&mut State)) -> El {
    let t = tok::theme(true);
    el().key(key).role("button").label(label).focusable(true).px(16.0).py(8.0).radius(8.0)
        .fill(if primary { tok::accent(0, true) } else { t.control_off })
        .on_press(on_press)
        .child(text(label).font(type_::TITLE).color(t.text))
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
        Screen::Profile(u) => {
            let rows = [("Email", &u.email), ("Job title", &u.job_title), ("Office", &u.office), ("User ID", &u.id)];
            page.child(title(&u.name))
                .children(rows.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| note(&format!("{k}: {v}")).selectable(true)))
                .child(button("sign-out", "Sign out", false, |s| {
                    s.attempt.fetch_add(1, SeqCst);
                    auth::sign_out();
                    s.screen = Screen::Login(None);
                }))
        }
    }
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("login", |s| s.screen = Screen::Login(None)),
            ("error", |s| s.screen = Screen::Login(Some("Couldn't reach login.microsoftonline.com.".into()))),
            ("code", |s| s.screen = Screen::Code { user_code: "ABCD1234".into(), url: "https://microsoft.com/devicelogin".into() }),
            ("profile", |s| {
                s.screen = Screen::Profile(User {
                    name: "Ada Lovelace".into(),
                    email: "ada@example.com".into(),
                    job_title: "Engineer".into(),
                    office: "London".into(),
                    id: "00000000-0000-0000-0000-000000000000".into(),
                })
            }),
        ],
        setup: Some(setup),
        poll: Some(poll),
        ..AppSpec::new("Teams", (420.0, 360.0), init, view)
    });
}
