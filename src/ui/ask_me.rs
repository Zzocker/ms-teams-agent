//! Ask Me conversations and their cited Teams answers.

use crate::*;
use std::collections::BTreeMap;

mod answer;
mod source;

#[derive(Default)]
pub(crate) struct AskMe {
    pub(crate) open: bool,
    chats: BTreeMap<usize, Conversation>,
    next_id: usize,
    active: Option<usize>,
    deleting: Option<usize>,
    new_chat: Conversation,
    sidebar_bar: Bar,
    sidebar_width: Option<f32>,
    source: source::Source,
}

#[derive(Default)]
struct Conversation {
    turns: Vec<Turn>,
    draft: TextEdit,
}

struct Turn {
    question: String,
    answer: Answer,
}

enum Answer {
    Demo(Vec<AnswerPart>),
    Planning(Latest<Result<(answer::Plan, serde_json::Value), String>>, model_connection::Config),
    Searching(Latest<Result<Vec<AnswerPart>, String>>),
    Ready(Result<Vec<AnswerPart>, String>),
}

struct AnswerPart {
    text: String,
    citations: Vec<source::Citation>,
}

impl AskMe {
    pub(crate) fn leave(&mut self) -> bool {
        self.source.dismiss();
        std::mem::take(&mut self.open)
    }

    fn current(&self) -> &Conversation {
        self.active.map_or(&self.new_chat, |id| &self.chats[&id])
    }

    fn current_mut(&mut self) -> &mut Conversation {
        match self.active {
            Some(id) => self.chats.get_mut(&id).unwrap(),
            None => &mut self.new_chat,
        }
    }
}

impl Conversation {
    fn title(&self) -> Option<&str> {
        self.turns.first().and_then(|turn| turn.question.lines().next())
    }

    fn pending(&self) -> bool {
        matches!(self.turns.last().map(|turn| &turn.answer), Some(Answer::Planning(..) | Answer::Searching(_)))
    }

    fn model_history(&self) -> Vec<answer::Exchange> {
        let mut history: Vec<_> = self.turns.iter().rev().filter_map(|turn| {
            let Answer::Ready(Ok(parts)) = &turn.answer else { return None };
            Some(answer::Exchange::new(&turn.question, parts))
        }).take(3).collect();
        history.reverse();
        history
    }
}

const INPUT: &str = "ask-me-draft";
const HISTORY: &str = "ask-me-history";
const DIVIDER_WIDTH: f32 = 8.0;
const MIN_SIDEBAR_WIDTH: f32 = 144.0;
const PROMPTS: [(&str, &str); 3] = [
    ("Catch me up", "Catch me up on my chats"),
    ("Recap a meeting", "What did we decide in my last meeting?"),
    ("Open questions", "Which questions are still open?"),
];

pub(super) static ASK_ME_ICON: rune::Icon = rune::Icon {
    name: "ask-me-sparkles",
    view: 24.0,
    body: r#"<path d="M10 3c.8 4.7 2.3 6.2 7 7-4.7.8-6.2 2.3-7 7-.8-4.7-2.3-6.2-7-7 4.7-.8 6.2-2.3 7-7Z M18.5 13v7 M15 16.5h7 M19 3v4 M17 5h4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

static UP_ARROW: rune::Icon = rune::Icon {
    name: "ask-me-arrow-up",
    view: 24.0,
    body: r#"<path d="M12 19V5m-6 6 6-6 6 6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

static PLUS: rune::Icon = rune::Icon {
    name: "ask-me-plus",
    view: 24.0,
    body: r#"<path d="M12 5v14M5 12h14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#,
};

static TRASH: rune::Icon = rune::Icon {
    name: "ask-me-trash",
    view: 24.0,
    body: r#"<path d="M4 6h16m-11 0V3h6v3M6 6l1 14h10l1-14M10 10v6m4-6v6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) fn open(s: &mut State, tx: &mut Tx) {
    if !s.ask_me.open {
        if s.restore_scroll.is_none() && s.messages.as_ref().is_some_and(Result::is_ok) {
            s.restore_scroll = Some(s.msg_bar.position());
        }
        s.ask_me.open = true;
        s.ask_me.deleting = None;
        s.message_search.close();
    }
    tx.focus(INPUT, false);
}

pub(crate) fn poll(s: &mut State, tx: &mut Tx) -> bool {
    let mut changed = source::poll(s, tx);
    let updates: Vec<_> = s.ask_me.chats.iter().filter_map(|(&id, chat)| {
        let next = match &chat.turns.last()?.answer {
            Answer::Planning(worker, config) => match worker.take()? {
                Ok((answer::Plan::Search(queries), request)) => start_search(s, config.clone(), request, queries),
                Ok((answer::Plan::Reply(text), _)) => Answer::Ready(Ok(vec![AnswerPart { text, citations: vec![] }])),
                Err(error) => Answer::Ready(Err(error)),
            },
            Answer::Searching(worker) => Answer::Ready(worker.take()?),
            _ => return None,
        };
        Some((id, next))
    }).collect();
    for (id, answer) in updates {
        if let Some(turn) = s.ask_me.chats.get_mut(&id).and_then(|chat| chat.turns.last_mut()) {
            turn.answer = answer;
        }
        if s.ask_me.open && s.ask_me.active == Some(id) {
            if s.ask_me.source.citation().is_none() { tx.scroll_to_bottom_instant(HISTORY); }
            changed = true;
        }
    }
    changed
}

fn start_search(s: &State, config: model_connection::Config, request: serde_json::Value, queries: Vec<String>) -> Answer {
    let Screen::Main { session, .. } = &s.screen else {
        return Answer::Ready(Err("Sign in to search Teams messages.".into()));
    };
    let session = session.clone();
    let context = answer::Context::collect(s);
    let worker = answer_worker(s);
    worker.request((request, queries), move |(request, queries)| {
        context.search(&session, &config, request, queries)
    });
    Answer::Searching(worker)
}

fn answer_worker<T: Send + 'static>(s: &State) -> Latest<T> {
    let worker = Latest::default();
    if let Some(waker) = &s.waker { worker.set_waker(waker.clone()); }
    worker
}

pub(super) fn close_source(s: &mut State, tx: &mut Tx) -> bool {
    if !s.ask_me.open || s.ask_me.source.citation().is_none() { return false; }
    source::close(s, tx);
    true
}

pub(super) fn source_panel(s: &State, compact: bool, accent: rune::Color, tint: rune::Color) -> Option<El> {
    source::panel(s, compact, accent, tint)
}

pub(super) const SOURCE_AND_TRANSCRIPT_WIDTH: f32 = source::PANEL_WIDTH + 320.0;

fn submit(s: &mut State, tx: &mut Tx) {
    let conversation = s.ask_me.current();
    let question = conversation.draft.text.trim();
    if question.is_empty() || conversation.pending() {
        return;
    }
    let question = question.to_owned();
    let answer = match (&s.screen, s.model_connection.saved.clone()) {
        (Screen::Main { .. }, Some(config)) => {
            let worker = answer_worker(s);
            let history = conversation.model_history();
            let planner_config = config.clone();
            worker.request(question.clone(), move |question| answer::plan(&planner_config, &question, &history));
            Answer::Planning(worker, config)
        }
        _ => Answer::Ready(Err("Connect a model in Settings → Ask Me to get answers.".into())),
    };
    let conversation = s.ask_me.current_mut();
    conversation.turns.push(Turn { question, answer });
    conversation.draft.clear();
    if s.ask_me.active.is_none() {
        let id = s.ask_me.next_id;
        s.ask_me.next_id += 1;
        s.ask_me.chats.insert(id, std::mem::take(&mut s.ask_me.new_chat));
        s.ask_me.active = Some(id);
        tx.scroll_to_instant("ask-me-chat-list", 0.0, 0.0);
    }
    tx.focus(INPUT, false);
    tx.scroll_to_bottom_instant(HISTORY);
}

pub(crate) fn demo_history(s: &mut State) {
    s.ask_me.open = true;
    s.ask_me.chats = [
        "Which questions are still open?",
        "Catch me up on my chats",
        "What did we decide in my last meeting?",
    ].into_iter().map(|question| Conversation {
        turns: vec![Turn { question: question.into(), answer: Answer::Demo(vec![]) }],
        ..Default::default()
    }).enumerate().collect();
    s.ask_me.next_id = s.ask_me.chats.len();
    s.ask_me.active = s.ask_me.chats.len().checked_sub(1);
}

pub(crate) fn demo_citations(s: &mut State, show_source: bool) {
    demo_history(s);
    let id = s.ask_me.next_id;
    s.ask_me.next_id += 1;
    let cite = |conversation: &str, message: &str| source::Citation {
        conversation: conversation.into(), message: message.into(),
    };
    let chat = cite("a", "2026-11-14T10:31:00Z");
    let channel = cite("backend", "101");
    let answer = vec![
        AnswerPart {
            text: "Charles suggested adding another column of gears to the engine.".into(),
            citations: vec![chat.clone()],
        },
        AnswerPart {
            text: "In Backend, Ada confirmed the schema and rollback checks passed. She’ll post the final checklist after the review.".into(),
            citations: vec![channel],
        },
    ];
    s.ask_me.chats.insert(id, Conversation {
        turns: vec![Turn { question: "What are the latest engineering updates?".into(), answer: Answer::Demo(answer) }],
        ..Default::default()
    });
    s.ask_me.active = Some(id);
    if show_source {
        source::open(s, &mut Tx::default(), chat, "ask-me-citation/0/0".into());
    }
}

pub(super) fn sidebar_width(s: &State, available: f32) -> (f32, f32) {
    let maximum = (available - DIVIDER_WIDTH).max(MIN_SIDEBAR_WIDTH);
    let default = (available * 0.8).clamp(MIN_SIDEBAR_WIDTH, 240.0);
    (s.ask_me.sidebar_width.unwrap_or(default).min(maximum), maximum)
}

pub(super) fn divider(width: f32, maximum: f32, surface: rune::Color) -> El {
    let t = tok::theme(dark());
    let clamp = move |width: f32| width.clamp(MIN_SIDEBAR_WIDTH, maximum);
    col().key("ask-me-resize").w(DIVIDER_WIDTH).h_full().center().fill(surface)
        .motion(Spring::NONE).hover(t.hover).cursor(rune::Cursor::ResizeColumn)
        .label("Resize Recent chats with Left and Right arrows").focusable(true)
        .help("Drag to resize. Double-click to expand or restore.")
        .on_double_press(move |s: &mut State| {
            s.ask_me.sidebar_width = (width < maximum).then_some(maximum);
        })
        .on_key(move |s: &mut State, e: &rune::KeyEvent, _| {
            let delta = match e.key {
                rune::Key::Left => -20.0,
                rune::Key::Right => 20.0,
                _ => return false,
            };
            let resized = clamp(width + delta);
            if resized != width { s.ask_me.sidebar_width = Some(resized); }
            true
        })
        .on_drag(move |s: &mut State, d: &mut Drag, _| {
            // A negative starting width marks the automatic size so cancellation can restore it.
            if d.phase == Phase::Down { d.data = s.ask_me.sidebar_width.unwrap_or(-width); }
            let start = clamp(d.data.abs());
            let resized = clamp(start + d.dx);
            s.ask_me.sidebar_width = if d.cancelled() || resized == start {
                (d.data > 0.0).then_some(d.data)
            } else {
                Some(resized)
            };
        })
        .child(el().w(1.0).pin(Some(3.5), Some(0.0), None, Some(0.0)).fill(t.separator))
        .child(el().size(3.0, 24.0).radius(2.0).fill(t.text_secondary))
}

pub(super) fn sidebar(s: &State, width: f32, surface: rune::Color, accent: rune::Color, tint: rune::Color) -> El {
    let t = tok::theme(dark());
    let list = col().w_full().grow(1.0).min_h(0.0).gap(3.0)
        .label("Recent Ask Me chats").role("list").focus_group()
        .scroll(ScrollSpec { stick_bottom: false })
        .children(s.ask_me.chats.iter().rev().map(|(&id, chat)| {
            let title = chat.title().unwrap_or("New chat");
            let selected = s.ask_me.active == Some(id);
            row().w_full().items_center().radius(7.0).fill(if selected { tint } else { surface })
                .child(row().key(format!("ask-me-chat/{id}")).role("button").label(title)
                    .checked(selected).focusable(true).grow(1.0).min_w(0.0).min_h(44.0).pl(10.0)
                    .items_center().radius(7.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                    .on_press_tx(move |s: &mut State, tx| {
                        s.ask_me.active = Some(id);
                        s.ask_me.deleting = None;
                        s.ask_me.source.dismiss();
                        tx.scroll_to_bottom_instant(HISTORY);
                    })
                    .child(text(title).font(type_::BODY).color(t.text).shrink().min_w(0.0)
                        .truncate().help_when_truncated(title)))
                .child(el().key(format!("ask-me-delete/{id}")).role("button")
                    .label(format!("Delete chat: {title}")).help("Delete chat").focusable(true)
                    .size(32.0, 36.0).center().radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                    .on_press_tx(move |s: &mut State, tx| {
                        s.ask_me.deleting = Some(id);
                        tx.focus("ask-me-cancel-delete", true);
                    })
                    .child(rune::icon(&TRASH, 16.0).color(t.text_secondary)))
        }));
    col().key("ask-me-sidebar").w(width).h_full().min_h(0.0).px(10.0).py(12.0).gap(8.0)
        .motion(Spring::NONE).fill(surface)
        .child(row().key("ask-me-new").w_full().role("button").label("New chat").focusable(true)
            .items_center().min_h(44.0).gap(8.0).px(10.0).radius(8.0).fill(tint)
            .hover(t.hover).cursor(rune::Cursor::Pointer)
            .on_press_tx(|s: &mut State, tx| {
                s.ask_me.active = None;
                s.ask_me.deleting = None;
                s.ask_me.source.dismiss();
                tx.focus(INPUT, false);
            })
            .child(rune::icon(&PLUS, 17.0).color(accent))
            .child(text("New chat").font(type_::TITLE).color(accent)))
        .child(text("Recent chats").role("heading").font(type_::LABEL).color(t.text_secondary).px(8.0).pt(6.0))
        .children(s.ask_me.deleting.and_then(|id| {
            let title = s.ask_me.chats.get(&id)?.title()?;
            Some(col().key("ask-me-delete-confirmation").w_full().gap(6.0).padding(8.0)
                .radius(7.0).fill(t.card).border(1.0, t.separator)
                .child(text("Delete this chat?").font(type_::TITLE).color(t.text).wrap())
                .child(text(title).font(type_::LABEL).color(t.text_secondary).truncate().help_when_truncated(title))
                .child(row().w_full().gap(4.0).flex_wrap().justify_end()
                    .child(el().key("ask-me-cancel-delete").role("button").label("Cancel").focusable(true)
                        .min_h(32.0).px(6.0).center().radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                        .on_press_tx(move |s: &mut State, tx| {
                            s.ask_me.deleting = None;
                            tx.focus(format!("ask-me-delete/{id}"), true);
                        })
                        .child(text("Cancel").font(type_::LABEL).color(t.text)))
                    .child(el().key("ask-me-confirm-delete").role("button").label("Delete").focusable(true)
                        .min_h(32.0).px(6.0).center().radius(6.0).hover(t.hover).cursor(rune::Cursor::Pointer)
                        .on_press_tx(move |s: &mut State, tx| {
                            s.ask_me.chats.remove(&id);
                            s.ask_me.deleting = None;
                            if s.ask_me.active == Some(id) {
                                s.ask_me.active = None;
                                s.ask_me.source.dismiss();
                            }
                            tx.focus("ask-me-new", true);
                        })
                        .child(text("Delete").font(type_::LABEL).color(t.text)))))
        }))
        .children(s.ask_me.chats.is_empty().then(|| {
            text("No chats yet").font(type_::BODY).color(t.text_secondary).wrap().px(8.0)
        }))
        .child(with_bar("ask-me-chat-list", list, &s.ask_me.sidebar_bar))
}

pub(super) fn view(s: &State, accent: rune::Color, tint: rune::Color, compact: bool) -> El {
    let t = tok::theme(dark());
    let conversation = s.ask_me.current();
    let empty = conversation.turns.is_empty();
    let can_submit = !conversation.pending() && !conversation.draft.text.trim().is_empty();
    let column = || col().w_full().max_w(680.0).min_w(0.0);
    let mark = |size, icon_size| {
        el().size(size, size).radius(size * 0.28).center().fill(tint)
            .child(rune::icon(&ASK_ME_ICON, icon_size).color(accent))
    };
    let header = row().w_full().min_h(52.0).gap(8.0).px(16.0).items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.separator)
        .child(text(conversation.title().unwrap_or("Ask Me")).role("heading").font(type_::TITLE)
            .color(t.text).min_w(0.0).shrink().truncate());

    let welcome = column().items_center().gap(8.0).py(6.0)
        .child(if compact { mark(38.0, 22.0) } else { mark(48.0, 26.0) })
        .child(text("What would you like to know?").key("ask-me-welcome").role("heading")
            .font(rune::TextStyle::new(if compact { 24.0 } else { 30.0 }, 650.0))
            .color(t.text).align(Align::Center).wrap())
        .child(text("Ask a question about your Teams chats or meetings.")
            .font(type_::BODY).color(t.text_secondary).align(Align::Center).wrap());
    let status = |message: &str, role| {
        text(message).role(role).font(type_::BODY).color(t.text_secondary).wrap()
    };
    let transcript = column().gap(16.0).py(24.0).role("log").label("Ask Me conversation")
        .children(conversation.turns.iter().enumerate().map(|(i, turn)| {
            col().w_full().gap(12.0)
                .child(col().key(format!("ask-me-question/{i}")).w_full().items_end().gap(5.0)
                    .child(text("You").font(type_::LABEL).color(t.text_secondary).pr(4.0))
                    .child(col().max_w(510.0).min_w(0.0).px(16.0).py(11.0).radius(14.0).fill(tint)
                        .child(text(turn.question.as_str()).font(type_::BODY).color(t.text).wrap().selectable(true))))
                .child({
                    let (title, parts, notice) = match &turn.answer {
                        Answer::Demo(parts) => ("Ask Me · Demo answer", parts.as_slice(), None),
                        Answer::Planning(..) => ("Ask Me", &[][..], Some(status("Preparing an answer…", "status"))),
                        Answer::Searching(_) => ("Ask Me", &[][..], Some(status("Searching Teams and preparing an answer…", "status"))),
                        Answer::Ready(Ok(parts)) if parts.is_empty() => ("Ask Me", &[][..], Some(status("I couldn’t find enough evidence in the Teams messages I checked.", "status"))),
                        Answer::Ready(Ok(parts)) => ("Ask Me", parts.as_slice(), None),
                        Answer::Ready(Err(error)) => ("Ask Me", &[][..], Some(status(error, "alert"))),
                    };
                    let mut citations = 0;
                    row().w_full().items_start().gap(10.0).child(mark(32.0, 18.0))
                        .child(col().key(format!("ask-me-answer/{i}")).grow(1.0).min_w(0.0).gap(8.0)
                            .child(text(title).font(type_::TITLE).color(t.text))
                            .children(notice)
                            .children(parts.iter().map(|part| {
                                let buttons = part.citations.iter().map(|citation| {
                                    let key = format!("ask-me-citation/{i}/{citations}");
                                    citations += 1;
                                    let selected = s.ask_me.source.citation() == Some(citation);
                                    let target = citation.clone();
                                    let title = notifications::conversation(s, &target.conversation)
                                        .map_or("Teams message", |chat| chat.title.as_str());
                                    el().key(key.clone()).role("button").label(format!("View source in {title}"))
                                        .focusable(true).checked(selected).min_w(0.0).min_h(34.0)
                                        .radius(7.0).px(8.0).center().fill(if selected { tint } else { t.card })
                                        .border(1.0, t.separator).hover(t.hover).cursor(rune::Cursor::Pointer)
                                        .on_press_tx(move |s: &mut State, tx| source::open(s, tx, target.clone(), key.clone()))
                                        .child(text(title).font(type_::LABEL).color(accent).truncate().help_when_truncated(title))
                                });
                                col().w_full().min_w(0.0).items_start().gap(4.0)
                                    .child(text(part.text.as_str()).font(type_::BODY).color(t.text).wrap().selectable(true))
                                    .children((!part.citations.is_empty()).then(|| row().w_full().flex_wrap().gap(4.0).children(buttons)))
                            })))
                })
        }));
    let composer = column().gap(8.0)
        .child(row().key("ask-me-composer").w_full().items_center().gap(8.0)
            .pl(14.0).pr(8.0).py(8.0).radius(16.0).fill(t.card).border(1.0, t.separator)
            .child(auto_grow(text_input(&conversation.draft, type_::BODY).key(INPUT)
                .label("Your question. Enter to send, Shift+Enter for a new line.")
                .grow(1.0).min_w(0.0).color(t.text)
                .placeholder("Ask a question…", t.text_secondary)
                .caret(t.text, accent.alpha(0.3)).focus_ring(2.0, 2.0, accent)
                .on_edit(|s: &mut State| &mut s.ask_me.current_mut().draft).on_submit(submit), 1, 5))
            .child(el().key("ask-me-send").role("button").label("Send question").help("Send question")
                .focusable(true).disabled(!can_submit).size(40.0, 40.0).center().radius(11.0)
                .fill(if can_submit { accent } else { t.control_off })
                .cursor(if can_submit { rune::Cursor::Pointer } else { rune::Cursor::Default })
                .on_press_tx(submit)
                .child(rune::icon(&UP_ARROW, 20.0).color(if can_submit { t.bg } else { t.text_secondary }))))
        .children(empty.then(|| {
            row().w_full().flex_wrap().justify_center().gap(8.0)
                .children(PROMPTS.into_iter().enumerate().map(|(i, (label, prompt))| {
                    el().key(format!("ask-me-prompt/{i}")).role("button").label(label)
                        .focusable(true).min_h(40.0).min_w(0.0).max_w(680.0).px(12.0).py(8.0)
                        .center().radius(8.0).border(1.0, t.separator).hover(t.hover)
                        .cursor(rune::Cursor::Pointer).on_press_tx(move |s: &mut State, tx| {
                            s.ask_me.current_mut().draft = TextEdit::new(prompt);
                            tx.focus(INPUT, false);
                        })
                        .child(text(label).font(type_::LABEL).color(t.text).wrap())
                }))
        }))
        .child(text("Answers use recent and loaded Teams messages.").key("ask-me-preview-note").role("note")
            .font(type_::LABEL).color(t.text_secondary).align(Align::Center).wrap().w_full());
    let body = if empty {
        col().key("ask-me-intro").w_full().grow(1.0).min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().h_full().items_center().gap(8.0).px(20.0).py(12.0)
                .child(el().grow(1.0))
                .child(welcome)
                .child(composer)
                .child(el().grow(1.2)))
    } else {
        col().w_full().grow(1.0).min_h(0.0)
            .child(col().key(HISTORY).w_full().grow(1.0).min_h(0.0).px(20.0).items_center()
                .scroll(ScrollSpec { stick_bottom: true }).child(transcript))
            .child(col().w_full().items_center().px(20.0).pt(10.0).pb(14.0).child(composer))
    };
    col().key("ask-me").grow(1.0).shrink().min_w(0.0).min_h(0.0).h_full().fill(t.bg)
        .child(header).child(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_me_preview_is_separate_from_the_teams_conversation() {
        let mut s = init();
        demo(&mut s);
        s.draft = TextEdit::new("Unsent Teams message");
        s.window_focused = true;
        let mut rt = rune::Runtime::new(s, crate::view, (640.0, 400.0), rune::Env::new(false, 1.0));
        let settle = |rt: &mut rune::Runtime<State>| {
            for _ in 0..45 {
                rt.frame(1.0 / 60.0);
            }
        };
        let click = |rt: &mut rune::Runtime<State>, key| {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            settle(rt);
        };
        settle(&mut rt);
        assert!(rt.find("nav-ask-me").unwrap().1 < rt.find("nav-chat").unwrap().1);
        click(&mut rt, "nav-ask-me");
        let elements = rt.elements();
        let rect = |key| elements.iter().find(|element| element.key.as_deref() == Some(key)).unwrap().rect;
        assert!(rect("ask-me-welcome")[1] >= rect("ask-me-intro")[1]);
        let [_, top, _, height] = rect("ask-me-preview-note");
        assert!(top + height <= 400.0);
        assert!(rt.find("sidebar").is_none());
        assert!(rt.find("ask-me-sidebar").is_some());
        assert!(!notifications::chat_open(&rt.state, "a"));

        click(&mut rt, "ask-me-prompt/1");
        rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
        settle(&mut rt);
        assert_eq!(rt.state.ask_me.current().turns[0].question, PROMPTS[1].1);
        assert!(rt.find("ask-me-question/0").is_some());

        click(&mut rt, "nav-chat");
        assert_eq!(rt.state.selected.as_deref(), Some("a"));
        assert_eq!(rt.state.draft.text, "Unsent Teams message");
        assert!(notifications::chat_open(&rt.state, "a"));
        click(&mut rt, "nav-ask-me");
        assert!(rt.find("ask-me-question/0").is_some());
        click(&mut rt, "ask-me-new");
        assert!(rt.state.ask_me.current().turns.is_empty());
        assert!(rt.find("ask-me-welcome").is_some());
        click(&mut rt, "ask-me-prompt/0");
        click(&mut rt, "ask-me-send");
        assert_eq!(rt.state.ask_me.current().turns[0].question, PROMPTS[0].1);
        assert!(rt.find("ask-me-chat/1").unwrap().1 < rt.find("ask-me-chat/0").unwrap().1);
        click(&mut rt, "ask-me-chat/0");
        assert_eq!(rt.state.ask_me.current().turns[0].question, PROMPTS[1].1);

        let (x, y) = rt.find("ask-me-resize").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_move(x + 26.0, y);
        rt.pointer_up(x + 26.0, y);
        settle(&mut rt);
        assert!(rt.find("ask-me-resize").unwrap().0 > x + 20.0);

        click(&mut rt, "ask-me-delete/1");
        click(&mut rt, "ask-me-cancel-delete");
        assert!(rt.find("ask-me-chat/1").is_some());
        click(&mut rt, "ask-me-delete/1");
        click(&mut rt, "ask-me-confirm-delete");
        assert!(rt.find("ask-me-chat/1").is_none());
        assert_eq!(rt.state.ask_me.current().turns[0].question, PROMPTS[1].1);
        click(&mut rt, "ask-me-delete/0");
        click(&mut rt, "ask-me-confirm-delete");
        assert!(rt.find("ask-me-chat/0").is_none());
        assert!(rt.find("ask-me-welcome").is_some());
    }

    #[test]
    fn citations_follow_two_conversations_and_keep_the_ask_me_draft() {
        for width in [640.0, 1200.0] {
            let mut s = init();
            ask_me_citation_demo(&mut s, false);
            s.ask_me.current_mut().draft = TextEdit::new("A follow-up draft");
            let mut rt = rune::Runtime::new(s, crate::view, (width, 700.0), rune::Env::new(false, 1.0));
            rt.on_key = Some(crate::app_key);
            let settle = |rt: &mut rune::Runtime<State>| {
                for _ in 0..45 { rt.frame(1.0 / 60.0); }
            };
            let click = |rt: &mut rune::Runtime<State>, key| {
                let (x, y) = rt.find(key).unwrap();
                rt.pointer_down(x, y);
                rt.pointer_up(x, y);
                settle(rt);
            };
            settle(&mut rt);
            click(&mut rt, "ask-me-citation/0/0");
            assert_eq!(rt.state.selected.as_deref(), Some("a"));
            assert!(rt.find("ask-me-source-cited").is_some());
            assert_eq!(rt.find("ask-me-answer/0").is_some(), width > 640.0);
            rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() });
            settle(&mut rt);
            assert!(rt.find("ask-me-source").is_none());
            assert!(rt.find("ask-me-answer/0").is_some());
            if width > 640.0 {
                rt.update(|s, _| {
                    let history = s.messages.take().unwrap().unwrap();
                    s.history.insert("a".into(), history);
                    s.messages = Some(Err("Earlier chat request failed".into()));
                    s.draft.replace_text("An unsent Teams message");
                });
                settle(&mut rt);
            }
            click(&mut rt, "ask-me-citation/0/0");
            click(&mut rt, "ask-me-source-open-chat");
            assert_eq!(rt.state.reply_highlight.as_ref().unwrap().0, "2026-11-14T10:31:00Z");
            if width > 640.0 {
                assert_eq!(rt.state.draft.text, "An unsent Teams message");
            }
            click(&mut rt, "nav-ask-me");
            click(&mut rt, "ask-me-citation/0/1");
            assert_eq!(rt.state.selected.as_deref(), Some("a"));
            assert!(rt.find("ask-me-source-cited").is_some());
            click(&mut rt, "ask-me-source-open-chat");
            assert_eq!(rt.state.selected.as_deref(), Some("backend"));
            assert_eq!(rt.state.reply_highlight.as_ref().unwrap().0, "channel-update");
            click(&mut rt, "nav-ask-me");
            assert!(rt.find("ask-me-answer/0").is_some());
            assert_eq!(rt.state.ask_me.current().draft.text, "A follow-up draft");
        }
    }
}
