//! State and worker lifecycle for one outbound audio call.

use crate::call_audio::Microphone;
use crate::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use teams::calling::{Progress, dial};

#[derive(Default)]
pub struct Calling {
    pub active: Option<Active>,
    pub finished: Option<Finished>,
    pub expanded: bool,
}

pub struct Contact {
    pub chat: String,
    pub avatar: String,
    pub name: String,
}

pub enum Phase {
    Dialing,
    Ringing,
    Connecting,
    Connected(Instant),
    Ending,
}

pub struct Active {
    pub contact: Arc<Contact>,
    pub phase: Phase,
    pub muted: bool,
    displayed_seconds: u64,
    microphone: Microphone,
    cancelled: Arc<AtomicBool>,
    events: mpsc::Receiver<Event>,
}

pub struct Finished {
    pub contact: Arc<Contact>,
    pub error: Option<String>,
}

enum Event {
    Progress(Progress),
    Finished(Result<(), String>),
}

impl Active {
    pub fn status(&self) -> &'static str {
        match self.phase {
            Phase::Dialing => "Calling…",
            Phase::Ringing => "Ringing…",
            Phase::Connecting => "Connecting audio…",
            Phase::Connected(_) => "Connected",
            Phase::Ending => "Ending call…",
        }
    }

    pub fn duration(&self) -> Option<String> {
        if !matches!(self.phase, Phase::Connected(_)) {
            return None;
        }
        let seconds = self.displayed_seconds;
        let minutes = seconds / 60;
        Some(if minutes < 60 {
            format!("{minutes:02}:{:02}", seconds % 60)
        } else {
            format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds % 60)
        })
    }
}

impl Calling {
    pub fn peer<'a>(s: &State, session: &Session, chat: &'a Chat) -> Option<&'a str> {
        let peer = teams::calling::direct_peer(&chat.id, &session.user.id)?;
        if chat.channel || chat.avatar_id.starts_with("28:") {
            return None;
        }
        // The conversation preview may be our own message. In that case a loaded bot message
        // identifies the peer even when the preview did not carry the bot's MRI.
        if s.selected.as_deref() == Some(chat.id.as_str())
            && let Some(Ok(page)) = &s.messages
            && page.msgs.iter().any(|message| {
                message
                    .sender_id
                    .strip_prefix("28:")
                    .is_some_and(|id| id.eq_ignore_ascii_case(peer))
            })
        {
            return None;
        }
        Some(peer)
    }

    pub fn start_teams(s: &mut State, contact: Arc<Contact>, tx: &mut Tx) {
        let (Screen::Main { session, .. }, Some(waker)) = (&s.screen, s.waker.clone()) else {
            return;
        };
        if s.calling.active.is_some() || s.selected.as_ref() != Some(&contact.chat) {
            return;
        }
        if notifications::conversation(s, &contact.chat)
            .and_then(|chat| Self::peer(s, session, chat))
            .is_none()
        {
            return;
        }
        let (session, id) = (session.clone(), contact.chat.clone());
        let calling = &mut s.calling;
        let microphone = Microphone::default();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (send, events) = mpsc::channel();
        calling.finished = None;
        calling.expanded = true;
        calling.active = Some(Active {
            contact,
            phase: Phase::Dialing,
            muted: false,
            displayed_seconds: 0,
            microphone: microphone.clone(),
            cancelled: cancelled.clone(),
            events,
        });
        std::thread::spawn(move || {
            let emit = |event| {
                let _ = send.send(event);
                waker();
            };
            let result = dial(&session, &id, cancelled, &microphone, |p| {
                emit(Event::Progress(p))
            });
            emit(Event::Finished(result));
        });
        tx.focus("mute-audio-call", false);
    }

    pub fn toggle_mute(&mut self) -> bool {
        let Some(call) = &mut self.active else {
            return false;
        };
        if matches!(call.phase, Phase::Ending) {
            return false;
        }
        call.muted = !call.muted;
        call.microphone.set_muted(call.muted);
        true
    }

    pub fn cancel(&mut self) {
        if let Some(call) = &mut self.active {
            call.microphone.set_muted(true);
            call.cancelled.store(true, Ordering::Relaxed);
            call.phase = Phase::Ending;
        }
    }

    pub fn quit(&mut self) {
        self.cancel();
        if let Some(call) = &self.active {
            let deadline = Instant::now() + Duration::from_secs(16);
            while let Ok(Event::Progress(_)) = call
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {}
        }
    }

    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        let Some(call) = &mut self.active else {
            return false;
        };
        let mut changed = false;
        loop {
            let error = match call.events.try_recv() {
                Ok(Event::Progress(progress)) => {
                    if !matches!(call.phase, Phase::Ending) {
                        call.phase = match progress {
                            Progress::Ringing => Phase::Ringing,
                            Progress::Connecting => Phase::Connecting,
                            Progress::Connected => Phase::Connected(Instant::now()),
                        };
                        changed = true;
                    }
                    continue;
                }
                Ok(Event::Finished(result)) => result.err(),
                Err(mpsc::TryRecvError::Disconnected) => Some("The audio worker stopped.".into()),
                Err(mpsc::TryRecvError::Empty) => break,
            };
            if self.expanded || error.is_some() {
                self.finished = Some(Finished {
                    contact: call.contact.clone(),
                    error,
                });
            }
            if self.expanded {
                tx.focus("close-call", false);
            }
            self.active = None;
            return true;
        }
        if let Phase::Connected(started) = call.phase {
            let elapsed = started.elapsed();
            let until_next_second =
                Duration::from_nanos(1_000_000_000 - u64::from(elapsed.subsec_nanos()));
            tx.wake_after(until_next_second);
            changed |= call.displayed_seconds != elapsed.as_secs();
            call.displayed_seconds = elapsed.as_secs();
        }
        changed
    }
}

impl Drop for Calling {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_screen_keeps_the_same_call_when_browsing_chats() {
        for dark in [false, true] {
            let mut s = init();
            demo(&mut s);
            s.window_focused = true;
            let (send, events) = mpsc::channel();
            let cancelled = Arc::new(AtomicBool::new(false));
            s.calling.expanded = true;
            s.calling.active = Some(Active {
                contact: Arc::new(Contact {
                    chat: "a".into(),
                    avatar: String::new(),
                    name: "Charles Babbage".into(),
                }),
                phase: Phase::Dialing,
                muted: false,
                displayed_seconds: 0,
                microphone: Microphone::default(),
                cancelled: cancelled.clone(),
                events,
            });
            let mut rt = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(dark, 1.0));
            let settle = |rt: &mut rune::Runtime<State>| {
                for _ in 0..30 {
                    rt.frame(1.0 / 60.0);
                }
            };
            let click = |rt: &mut rune::Runtime<State>, key| {
                let (x, y) = rt.find(key).unwrap_or_else(|| panic!("Missing {key}"));
                assert!(x > 0.0 && x < 640.0 && y > 0.0 && y < 400.0);
                rt.pointer_down(x, y);
                rt.pointer_up(x, y);
                settle(rt);
            };
            settle(&mut rt);
            assert!(!notifications::chat_open(&rt.state, "a"));
            assert!(rt.find("call-screen").is_some());
            assert!(rt.find("call-status").is_none());
            send.send(Event::Progress(Progress::Ringing)).unwrap();
            rt.update(|s, tx| assert!(s.calling.poll(tx)));
            settle(&mut rt);
            assert_eq!(
                rt.state.calling.active.as_ref().unwrap().status(),
                "Ringing…"
            );
            send.send(Event::Progress(Progress::Connected)).unwrap();
            rt.update(|s, tx| assert!(s.calling.poll(tx)));
            settle(&mut rt);
            assert!(rt.find("call-duration").is_some());

            click(&mut rt, "mute-audio-call");
            assert!(rt.state.calling.active.as_ref().unwrap().muted);
            click(&mut rt, "open-call-chat");
            assert!(notifications::chat_open(&rt.state, "a"));
            assert!(rt.find("call-screen").is_none());
            click(&mut rt, "chat/b");
            assert_eq!(rt.state.selected.as_deref(), Some("b"));
            click(&mut rt, "return-to-call");
            assert!(rt.state.calling.active.as_ref().unwrap().muted);
            click(&mut rt, "open-call-chat");
            assert_eq!(rt.state.selected.as_deref(), Some("a"));

            click(&mut rt, "return-to-call");
            click(&mut rt, "end-audio-call");
            assert!(cancelled.load(Ordering::Relaxed));
            assert!(matches!(
                rt.state.calling.active.as_ref().unwrap().phase,
                Phase::Ending
            ));
            send.send(Event::Finished(Ok(()))).unwrap();
            rt.update(|s, tx| assert!(s.calling.poll(tx)));
            settle(&mut rt);
            assert!(rt.find("close-call").is_some());
            click(&mut rt, "close-call");
            assert!(rt.find("call-screen").is_none());
            assert!(rt.find("call-status").is_none());
        }
    }
}
