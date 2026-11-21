//! The Insights graph, its camera and the date controls above it.
use crate::insights::{self, Counts, Range};
use crate::*;
use rune::{Color, LineCap, Path, Shape, canvas, color::hex};
use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_4, PI, TAU};
use std::sync::LazyLock;

type Point = [f32; 2];
const PROFILE_DIAMETER: f32 = 66.0;
const LABEL_WIDTH: f32 = 124.0;
const PROFILE_KEY: &str = "insights-profile";
const MAX_CHAT_NODES: usize = 30;

struct Palette {
    background: Color,
    surface: Color,
    text: Color,
    text_secondary: Color,
    border: Color,
    hover: Color,
    selected: Color,
    accent: Color,
}

fn palette() -> &'static Palette {
    static PALETTES: LazyLock<[Palette; 2]> = LazyLock::new(|| {
        [false, true].map(|dark| {
            let color = |light, night| hex(if dark { night } else { light }).unwrap();
            Palette {
                background: color("#E9E7FB", "#252344"),
                surface: color("#F4EEFF", "#302A53"),
                text: color("#32265C", "#EDE0FF"),
                text_secondary: color("#54457C", "#CBBDE8"),
                border: color("#8771AF", "#8370A9"),
                hover: color("#E3D7FA", "#44366C"),
                selected: color("#D9C8F5", "#4C3B77"),
                accent: color("#6245A3", "#BEA4FF"),
            }
        })
    });
    &PALETTES[usize::from(dark())]
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn scaled([x, y]: Point, scale: f32) -> Point {
    [x * scale, y * scale]
}

fn length([x, y]: Point) -> f32 {
    x.hypot(y)
}

fn radial(angle: f32, distance: f32) -> Point {
    [angle.cos() * distance, angle.sin() * distance]
}

#[derive(Clone, Copy, Default)]
struct Particle {
    at: Point,
    velocity: Point,
}

impl Particle {
    fn settle(&mut self, target: Point) {
        *self = Self {
            at: target,
            velocity: [0.0; 2],
        };
    }

    fn step(&mut self, target: Point, stiffness: f32, damping: f32, dt: f32) -> bool {
        let delta = sub(target, self.at);
        if length(delta) < 0.12 && length(self.velocity) < 1.2 {
            self.settle(target);
            return false;
        }
        self.velocity = scaled(
            add(self.velocity, scaled(delta, stiffness * dt)),
            (-damping * dt).exp(),
        );
        self.at = add(self.at, scaled(self.velocity, dt));
        true
    }
}

struct Node {
    id: String,
    angle: f32,
    distance: f32,
    diameter: f32,
    particle: Particle,
}

struct Grab {
    id: Option<String>,
    start: Point,
    target: Option<Point>,
}

pub(crate) struct Graph {
    nodes: Vec<Node>,
    hub: Particle,
    hub_target: Point,
    pan: Point,
    zoom: f32,
    viewport: Point,
    dots: rune::Pixels,
    drag: Option<Grab>,
    previous_frame: Option<f64>,
    reduced_motion: Cell<bool>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            nodes: vec![],
            hub: Particle::default(),
            hub_target: [0.0; 2],
            pan: [0.0; 2],
            zoom: 1.0,
            viewport: [0.0; 2],
            dots: Default::default(),
            drag: None,
            previous_frame: None,
            reduced_motion: Cell::new(false),
        }
    }
}

impl Graph {
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.nodes.iter().any(|node| node.id == id)
    }

    pub fn set_counts(&mut self, counts: &HashMap<String, Counts>) {
        let mut ranked: Vec<_> = counts.iter().filter(|(_, count)| count.total > 0).collect();
        ranked.sort_unstable_by(|(a_id, a), (b_id, b)| {
            b.sent
                .cmp(&a.sent)
                .then_with(|| b.total.cmp(&a.total))
                .then_with(|| a_id.cmp(b_id))
        });
        let max_sent = ranked.first().map_or(1, |(_, count)| count.sent.max(1)) as f32;
        let visible: HashSet<_> = ranked
            .into_iter()
            .take(MAX_CHAT_NODES)
            .map(|(id, _)| id.as_str())
            .collect();
        let old_len = self.nodes.len();
        self.nodes.retain(|node| visible.contains(node.id.as_str()));
        if self
            .drag
            .as_ref()
            .and_then(|drag| drag.id.as_deref())
            .is_some_and(|id| !self.contains(id))
        {
            self.stop_drag();
        }
        let present: HashSet<_> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        let added: HashSet<_> = visible
            .into_iter()
            .filter(|id| !present.contains(id))
            .collect();
        let membership_changed = old_len != self.nodes.len() || !added.is_empty();
        self.nodes.extend(added.iter().map(|id| Node {
            id: (*id).to_owned(),
            angle: 0.0,
            distance: 0.0,
            diameter: 0.0,
            particle: Particle::default(),
        }));
        if membership_changed {
            self.nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        }
        let count_nodes = self.nodes.len();
        let spread = spread(count_nodes);
        for (index, node) in self.nodes.iter_mut().enumerate() {
            let count = counts[&node.id];
            if membership_changed {
                node.angle = -FRAC_PI_4 + index as f32 * TAU / count_nodes as f32;
            }
            node.distance = (113.0 + 76.0 * (1.0 - count.sent as f32 / max_sent)) * spread;
            node.diameter = (38.0 + 42.0 * (count.total as f32 / 850.0).sqrt()).min(86.0);
            if added.contains(node.id.as_str()) {
                node.particle
                    .settle(add(self.hub.at, radial(node.angle, node.distance)));
            }
        }
        if self.viewport == [0.0; 2] || self.reduced_motion.get() {
            self.settle();
        }
    }

    fn settle(&mut self) {
        self.hub.settle(self.hub_target);
        for node in &mut self.nodes {
            node.particle
                .settle(add(self.hub.at, radial(node.angle, node.distance)));
        }
        self.previous_frame = None;
    }

    pub fn tick(&mut self, now: f64, tx: &mut Tx) -> bool {
        let dt = self
            .previous_frame
            .map_or(1.0 / 60.0, |last| (now - last).clamp(0.0, 0.025)) as f32;
        let mut changed = self.hub.at != self.hub_target;
        let mut moving = if self.reduced_motion.get() {
            self.hub.settle(self.hub_target);
            false
        } else {
            self.hub.step(self.hub_target, 350.0, 24.0, dt)
        };
        for (index, node) in self.nodes.iter_mut().enumerate() {
            let held = self
                .drag
                .as_ref()
                .and_then(|grab| grab.target.filter(|_| grab.id.as_ref() == Some(&node.id)));
            let target =
                held.unwrap_or_else(|| add(self.hub.at, radial(node.angle, node.distance)));
            changed |= node.particle.at != target;
            if self.reduced_motion.get() {
                node.particle.settle(target);
            } else {
                let (stiffness, damping) = if held.is_some() {
                    (440.0, 26.0)
                } else {
                    (115.0 + (index % 3) as f32 * 16.0, 13.0)
                };
                moving |= node.particle.step(target, stiffness, damping, dt);
            }
        }
        self.previous_frame = moving.then_some(now);
        if moving {
            tx.wake_after(std::time::Duration::ZERO);
        }
        changed || moving
    }

    fn resize(&mut self, size: Point) {
        if self.viewport == size {
            return;
        }
        self.viewport = size;
        // Rasterize the stationary dot grid only when the viewport changes size.
        let [w, h] = size.map(|side| side.ceil().clamp(1.0, 2048.0) as u32);
        let mut pixels = vec![0; w as usize * h as usize * 4];
        for y in (8..h).step_by(16) {
            for x in (8..w).step_by(16) {
                let i = (y * w + x) as usize * 4;
                pixels[i..i + 4].copy_from_slice(&[137, 113, 184, 89]);
            }
        }
        self.dots.set(w, h, pixels);
    }

    fn scale(&self) -> f32 {
        let radius = 189.0 * spread(self.nodes.len());
        let [w, h] = self.viewport;
        (w / (radius * 2.0 + LABEL_WIDTH + 24.0))
            .min(h / (radius * 2.0 + 96.0))
            .clamp(0.02, 1.2)
            * self.zoom
    }

    fn screen(&self, at: Point) -> Point {
        add(
            add(scaled(self.viewport, 0.5), self.pan),
            scaled(at, self.scale()),
        )
    }

    fn zoom_at(&mut self, factor: f32, pointer: Point) {
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.65, 6.0);
        let from_center = sub(pointer, scaled(self.viewport, 0.5));
        self.pan = sub(
            from_center,
            scaled(sub(from_center, self.pan), self.zoom / old),
        );
    }

    fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = scaled(self.hub_target, -self.scale());
    }

    pub fn reveal(&mut self, id: &str) {
        let Some(node) = self.nodes.iter().find(|node| node.id == id) else {
            return;
        };
        let [x, y] = self.screen(node.particle.at);
        let [w, h] = self.viewport;
        if x < 62.0 || x > w - 62.0 || y < 46.0 || y > h - 70.0 {
            self.pan = scaled(node.particle.at, -self.scale());
        }
    }

    pub fn stop_drag(&mut self) {
        self.drag = None;
        self.previous_frame = None;
    }

    fn drop_chat(&mut self, id: &str, position: Point) {
        let [x, y] = sub(position, self.hub.at);
        if x.hypot(y) <= 0.1 {
            return;
        }
        let Some(dropped) = self.nodes.iter().position(|node| node.id == id) else {
            return;
        };
        let angle = y.atan2(x);
        let turn = (angle - self.nodes[dropped].angle + PI).rem_euclid(TAU) - PI;
        let preferred = if turn < 0.0 { -1.0 } else { 1.0 };
        let gap = TAU / self.nodes.len() as f32;
        let layout = |direction: f32| {
            let lead = (turn.abs() * 0.08).min(gap * 0.08);
            let heading = angle + direction * lead;
            let heading = if x * heading.cos() < 0.0 {
                angle
            } else {
                heading
            };
            self.drop_layout(dropped, heading, direction, gap)
        };
        let (positions, longest) = layout(preferred);
        let (opposite, opposite_longest) = layout(-preferred);
        let positions = if opposite_longest + 0.001 < longest {
            opposite
        } else {
            positions
        };
        for (index, angle) in positions {
            self.nodes[index].angle = angle;
        }
    }

    fn drop_layout(
        &self,
        dropped: usize,
        heading: f32,
        direction: f32,
        gap: f32,
    ) -> (Vec<(usize, f32)>, f32) {
        let mut neighbors: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != dropped)
            .map(|(index, node)| (index, ((node.angle - heading) * direction).rem_euclid(TAU)))
            .collect();
        neighbors.sort_by(|(_, a), (_, b)| a.total_cmp(b));

        // Advance the whole chain to equally spaced slots without moving any neighbor backward.
        let skipped = neighbors
            .iter()
            .enumerate()
            .map(|(rank, &(_, offset))| {
                let first_slot = ((offset - 0.0001) / gap).ceil() as usize;
                first_slot.saturating_sub(rank + 1)
            })
            .max()
            .unwrap_or(0);
        let count = neighbors.len();
        let mut longest: f32 = 0.0;
        for (rank, (_, offset)) in neighbors.iter_mut().enumerate() {
            let slot = rank + skipped + 1;
            // A chain that passes the drop skips the slot reserved for the dragged bubble.
            let distance = (slot + (slot - 1) / count) as f32 * gap;
            longest = longest.max(distance - *offset);
            *offset = heading + direction * distance;
        }
        neighbors.push((dropped, heading));
        (neighbors, longest)
    }

    fn gesture(&mut self, target: Target<'_>, event: &Drag, tx: &mut Tx) -> bool {
        if event.phase == Phase::Down {
            let start = match target {
                Target::Canvas => self.pan,
                Target::Profile => self.hub.at,
                Target::Chat(id) => {
                    let Some(node) = self.nodes.iter().find(|node| node.id == id) else {
                        return false;
                    };
                    node.particle.at
                }
            };
            let id = match target {
                Target::Chat(id) => Some(id.to_owned()),
                _ => None,
            };
            self.drag = Some(Grab {
                id,
                start,
                target: None,
            });
            return false;
        }

        let Some(grab) = &self.drag else {
            return false;
        };
        let moved = grab.target.is_some() || event.dx.hypot(event.dy) >= 5.0;
        if moved {
            let delta = [event.dx, event.dy];
            let next = if event.cancelled() {
                grab.start
            } else {
                add(
                    grab.start,
                    if matches!(target, Target::Canvas) {
                        delta
                    } else {
                        scaled(delta, 1.0 / self.scale())
                    },
                )
            };
            self.drag.as_mut().unwrap().target = Some(next);
            match target {
                Target::Canvas => self.pan = next,
                Target::Profile => self.hub_target = next,
                Target::Chat(id) if event.phase == Phase::Up && !event.cancelled() => {
                    self.drop_chat(id, next);
                }
                Target::Chat(_) => {}
            }
            tx.wake_after(std::time::Duration::ZERO);
        }
        if event.phase == Phase::Up {
            self.stop_drag();
        }
        if self.reduced_motion.get() {
            self.tick(tx.clock(), tx);
        }
        moved
    }

    fn dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|grab| grab.target.is_some())
    }
}

fn spread(chats: usize) -> f32 {
    // Allow roughly a bubble's width between adjacent chat nodes on larger graphs.
    (chats as f32 * 0.083).max(1.0)
}

#[derive(Clone, Copy)]
enum Target<'a> {
    Canvas,
    Profile,
    Chat(&'a str),
}

const RANGES: [(Range, &str, &str); 6] = [
    (Range::Day, "1 day", "insights-1-day"),
    (Range::Week, "7 days", "insights-7-days"),
    (Range::Month, "30 days", "insights-30-days"),
    (Range::SixMonths, "6 months", "insights-6-months"),
    (Range::AllTime, "All time", "insights-all-time"),
    (Range::Custom, "Custom range", "insights-custom"),
];

fn control(
    key: &str,
    label: &str,
    title: &str,
    action: impl Fn(&mut State, &mut Tx) + 'static,
) -> El {
    let t = palette();
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .focus_ring(2.0, 1.0, t.accent)
        .min_w(34.0)
        .min_h(34.0)
        .px(9.0)
        .center()
        .radius(7.0)
        .fill(t.surface)
        .border(1.0, t.border)
        .hover(t.hover)
        .cursor(rune::Cursor::Pointer)
        .on_press_tx(action)
        .child(text(title).font(type_::LABEL).color(t.text))
}

fn range_toolbar(state: &insights::Insights) -> El {
    let t = palette();
    let custom = state.range == Range::Custom;
    let (_, title, _) = RANGES
        .iter()
        .find(|(range, _, _)| *range == state.range)
        .unwrap();
    let range = row()
        .items_center()
        .gap(8.0)
        .child(text("Time range").font(type_::BODY).color(t.text))
        .child(
            control(
                "insights-range",
                &format!("Time range, {title}"),
                &format!("{title}  ▾"),
                |s, tx| {
                    s.insights.range_menu = !s.insights.range_menu;
                    if s.insights.range_menu {
                        let (_, _, key) = RANGES
                            .iter()
                            .find(|(range, _, _)| *range == s.insights.range)
                            .unwrap();
                        tx.focus(*key, true);
                    }
                },
            )
            .expanded(state.range_menu),
        );
    col()
        .gap(6.0)
        .child(
            row()
                .key("insights-toolbar")
                .items_center()
                .flex_wrap()
                .gap(8.0)
                .child(range)
                .children(custom.then(|| {
                    row()
                        .items_center()
                        .gap(8.0)
                        .flex_wrap()
                        .child(date_input(true, &state.from))
                        .child(date_input(false, &state.to))
                        .child(control(
                            "insights-apply",
                            "Apply date range",
                            "Apply",
                            |s, tx| {
                                insights::apply_custom(s);
                                tx.wake_after(std::time::Duration::ZERO);
                            },
                        ))
                }))
                .children((!custom).then(|| el().grow(1.0)))
                .children((state.failed() == 0).then(|| {
                    control(
                        "insights-refresh",
                        "Refresh Insights",
                        if custom { "↻" } else { "Refresh" },
                        |s, _| insights::refresh(s),
                    )
                    .help("Refresh Insights")
                })),
        )
        .children(state.date_error.map(|error| {
            text(error)
                .role("alert")
                .wrap()
                .font(type_::LABEL)
                .color(t.text)
        }))
}

fn date_input(from: bool, value: &TextEdit) -> El {
    let t = palette();
    let (key, label) = if from {
        ("insights-from", "From")
    } else {
        ("insights-to", "To")
    };
    row()
        .items_center()
        .gap(6.0)
        .child(text(label).font(type_::LABEL).color(t.text))
        .child(
            text_input(value, type_::LABEL)
                .key(key)
                .label(format!("{label} date, YYYY-MM-DD"))
                .w(96.0)
                .h(34.0)
                .px(8.0)
                .py(8.0)
                .color(t.text)
                .fill(t.surface)
                .radius(7.0)
                .border(1.0, t.border)
                .focus_ring(2.0, 1.0, t.accent)
                .caret(t.text, t.text.alpha(0.24))
                .placeholder("YYYY-MM-DD", t.text_secondary)
                .on_edit(move |s: &mut State| {
                    if from {
                        &mut s.insights.from
                    } else {
                        &mut s.insights.to
                    }
                })
                .on_submit(|s: &mut State, tx| {
                    insights::apply_custom(s);
                    tx.wake_after(std::time::Duration::ZERO);
                }),
        )
}

fn range_menu(state: &insights::Insights) -> El {
    let t = palette();
    col()
        .key("insights-range-menu")
        .role("group")
        .label("Time range")
        .absolute(76.0, 48.0)
        .w(154.0)
        .padding(4.0)
        .gap(2.0)
        .fill(t.surface)
        .radius(8.0)
        .border(1.0, t.border)
        .modal_tx(true, close_menu)
        .on_key(|_: &mut State, event, tx| {
            let delta = match event.key {
                rune::Key::Up => RANGES.len() - 1,
                rune::Key::Down => 1,
                _ => return false,
            };
            let index = RANGES
                .iter()
                .position(|(_, _, key)| tx.focused() == Some(*key))
                .unwrap_or(0);
            tx.focus(RANGES[(index + delta) % RANGES.len()].2, true);
            true
        })
        .children(RANGES.into_iter().map(|(range, label, key)| {
            el().key(key)
                .role("button")
                .label(label)
                .checked(range == state.range)
                .focusable(true)
                .focus_ring(2.0, 1.0, t.accent)
                .px(8.0)
                .min_h(33.0)
                .center_y()
                .radius(5.0)
                .fill(if range == state.range {
                    t.selected
                } else {
                    t.surface
                })
                .hover(t.hover)
                .cursor(rune::Cursor::Pointer)
                .on_press_tx(move |s: &mut State, tx| {
                    insights::select_range(s, range);
                    tx.focus(
                        if range == Range::Custom {
                            "insights-from"
                        } else {
                            "insights-range"
                        },
                        true,
                    );
                    tx.wake_after(std::time::Duration::ZERO);
                })
                .child(text(label).font(type_::LABEL).color(t.text))
        }))
}

fn close_menu(s: &mut State, tx: &mut Tx) {
    s.insights.range_menu = false;
    tx.focus("insights-range", true);
}

fn chat_name(chat: &Chat) -> &str {
    if chat.id == teams::NOTES {
        "You"
    } else {
        &chat.title
    }
}

fn duplicate_contexts<'a>(
    state: &insights::Insights,
    chats: &'a [Chat],
    me: &str,
) -> HashMap<&'a str, String> {
    let mut by_title = HashMap::<String, Vec<&Chat>>::new();
    for chat in chats.iter().filter(|chat| state.includes_chat(chat)) {
        by_title
            .entry(chat_name(chat).trim().to_lowercase())
            .or_default()
            .push(chat);
    }
    let mut labels = HashMap::new();
    for duplicates in by_title.into_values().filter(|chats| chats.len() > 1) {
        let mut by_kind = HashMap::<&str, Vec<&Chat>>::new();
        for chat in duplicates {
            let kind = if chat.id == teams::NOTES {
                "Self chat"
            } else if teams::peer(&chat.id, me).is_some() {
                "Direct chat"
            } else if chat.id.starts_with("19:meeting_") {
                "Meeting chat"
            } else if chat.id.ends_with("@thread.v2") || !chat.avatar_members.is_empty() {
                "Group chat"
            } else {
                "Chat"
            };
            by_kind.entry(kind).or_default().push(chat);
        }
        for (kind, chats) in by_kind {
            if chats.len() == 1 {
                labels.insert(chats[0].id.as_str(), kind.to_owned());
                continue;
            }
            // Same-named chats of the same kind can also share their last activity date.
            let mut by_date = HashMap::<Option<String>, Vec<&str>>::new();
            for chat in &chats {
                let date = chrono::DateTime::parse_from_rfc3339(&chat.time)
                    .ok()
                    .map(|time| {
                        time.with_timezone(&chrono::Local)
                            .format("%b %-d, %Y")
                            .to_string()
                    });
                by_date.entry(date).or_default().push(&chat.id);
            }
            for (date, ids) in &mut by_date {
                ids.sort_unstable();
                let base = date
                    .as_ref()
                    .map_or_else(|| kind.to_owned(), |day| format!("{kind} · {day}"));
                for (index, &id) in ids.iter().enumerate() {
                    let label = if ids.len() == 1 {
                        base.clone()
                    } else {
                        format!("{base} ({})", index + 1)
                    };
                    labels.insert(id, label);
                }
            }
        }
    }
    labels
}

pub(super) fn pane(s: &State, session: &Session, chats: &[Chat], cx: &Cx<State>) -> El {
    let state = &s.insights;
    state.graph.reduced_motion.set(cx.is_reduced_motion());
    let contexts = duplicate_contexts(state, chats, &session.user.id);
    let t = palette();
    let content = [
        range_toolbar(state),
        col()
            .key("insights-figure")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(t.surface)
            .radius(10.0)
            .padding(1.0)
            .clip_rounded(10.0)
            .border(1.0, t.border)
            .child(viewport(s, session, chats, &contexts))
            .child(caption(state, chats, &contexts)),
    ];
    col()
        .key("analytics-pane")
        .label("Insights")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(12.0)
        .gap(10.0)
        .fill(t.background)
        .children(content.into_iter().map(|child| {
            if state.range_menu {
                child.inert()
            } else {
                child
            }
        }))
        .children(state.range_menu.then(|| range_menu(state)))
}

fn viewport(s: &State, session: &Session, chats: &[Chat], contexts: &HashMap<&str, String>) -> El {
    let state = &s.insights;
    let graph = &state.graph;
    let t = palette();
    let ids: HashMap<_, _> = chats.iter().map(|chat| (chat.id.as_str(), chat)).collect();
    let shown_tip = if graph.dragging() {
        None
    } else {
        state.focused.as_ref().or(state.hovered.as_ref())
    };
    el().key("insights-viewport")
        .label("Conversation graph. Use arrow keys to pan and plus or minus to zoom.")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.background)
        .clip_rounded(0.0)
        .focusable(true)
        .focus_ring(2.0, 1.0, t.accent)
        .on_layout(|s: &mut State, [_, _, w, h]| s.insights.graph.resize([w, h]))
        .on_wheel(|s: &mut State, event, _| {
            s.insights.graph.zoom_at(
                (event.dy * 0.0015).clamp(-1.0, 1.0).exp(),
                [event.x, event.y],
            );
            true
        })
        .on_magnify(|s: &mut State, event, _| {
            s.insights
                .graph
                .zoom_at((1.0 + event.delta).max(0.1), [event.x, event.y]);
            true
        })
        .on_key(|s: &mut State, event, _| {
            let graph = &mut s.insights.graph;
            let offset = match event.key {
                rune::Key::Left => [36.0, 0.0],
                rune::Key::Right => [-36.0, 0.0],
                rune::Key::Up => [0.0, 36.0],
                rune::Key::Down => [0.0, -36.0],
                rune::Key::Text(ref value) if value == "+" || value == "=" => {
                    zoom(graph, 1.25);
                    return true;
                }
                rune::Key::Text(ref value) if value == "-" => {
                    zoom(graph, 0.8);
                    return true;
                }
                _ => return false,
            };
            graph.pan = add(graph.pan, offset);
            true
        })
        .on_drag(|s: &mut State, event, tx| {
            s.insights.graph.gesture(Target::Canvas, event, tx);
        })
        .cursor(if graph.dragging() {
            rune::Cursor::Grabbing
        } else {
            rune::Cursor::Grab
        })
        .child(
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .pixels(&graph.dots)
                .a11y_hidden(),
        )
        .child(
            canvas(edges(state))
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .a11y_hidden(),
        )
        .children(
            (graph.viewport[0] > 0.0 && graph.viewport[1] > 0.0)
                .then(|| graph_node(s, session, None, None, graph.hub.at, PROFILE_DIAMETER)),
        )
        .children(graph.nodes.iter().filter_map(|node| {
            let chat = *ids.get(node.id.as_str())?;
            (graph.viewport[0] > 0.0 && graph.viewport[1] > 0.0).then(|| {
                let context = contexts.get(chat.id.as_str()).map(String::as_str);
                graph_node(
                    s,
                    session,
                    Some(chat),
                    context,
                    node.particle.at,
                    node.diameter,
                )
            })
        }))
        .child(
            row()
                .pin(None, None, Some(10.0), Some(10.0))
                .items_center()
                .gap(4.0)
                .child(control("insights-zoom-out", "Zoom out", "−", |s, _| {
                    zoom(&mut s.insights.graph, 0.8)
                }))
                .child(
                    text(format!("{:.0}%", graph.zoom * 100.0))
                        .px(5.0)
                        .py(3.0)
                        .radius(4.0)
                        .fill(t.surface)
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                )
                .child(control("insights-zoom-in", "Zoom in", "+", |s, _| {
                    zoom(&mut s.insights.graph, 1.25)
                }))
                .child(control("insights-fit", "Fit graph", "Fit graph", |s, _| {
                    s.insights.graph.fit()
                })),
        )
        .children(shown_tip.and_then(|id| tooltip(state, id)))
}

fn zoom(graph: &mut Graph, factor: f32) {
    graph.zoom_at(factor, scaled(graph.viewport, 0.5));
}

fn edges(state: &insights::Insights) -> Shape {
    let graph = &state.graph;
    let ink = palette().accent;
    let scale = graph.scale();
    let start = graph.screen(graph.hub.at);
    let mut shape = Shape::new();
    for node in &graph.nodes {
        let end = graph.screen(node.particle.at);
        let [dx, dy] = sub(end, start);
        let distance = dx.hypot(dy).max(1.0);
        let normal = [-dy / distance, dx / distance];
        let velocity = add(
            scaled(node.particle.velocity, 0.55),
            scaled(graph.hub.velocity, 0.45),
        );
        let sway = -(velocity[0] * normal[0] + velocity[1] * normal[1]) * 0.09;
        let mid = scaled(add(start, end), 0.5);
        for (offset, width, alpha) in [(-2.0, 1.2, 0.29), (3.0, 0.8, 0.13)] {
            let point = add(
                mid,
                scaled(normal, (sway.clamp(-28.0, 28.0) + offset) * scale),
            );
            let path = Path::new().move_to(start).quad_to(point, end);
            let selected = state.selected.as_deref() == Some(&node.id);
            shape = shape.stroke_path(
                &path,
                (width * scale).max(0.7),
                LineCap::Round,
                ink.alpha(if selected {
                    (alpha * 2.8_f32).min(1.0)
                } else {
                    alpha
                }),
            );
        }
    }
    shape
}

fn graph_node(
    s: &State,
    session: &Session,
    chat: Option<&Chat>,
    context: Option<&str>,
    world: Point,
    diameter: f32,
) -> El {
    let state = &s.insights;
    let graph = &state.graph;
    let t = palette();
    let name = chat.map_or("My profile", chat_name);
    let id = chat.map(|chat| chat.id.clone());
    let key = id.as_ref().map_or_else(
        || PROFILE_KEY.to_owned(),
        |id| format!("insights-node/{id}"),
    );
    let picked = state.selected == id;
    let accessible = id.as_deref().map_or_else(
        || name.to_owned(),
        |id| match context {
            Some(context) => format!("{name}, {context}, {}", count_label(state, id)),
            None => format!("{name}, {}", count_label(state, id)),
        },
    );
    let scale = graph.scale();
    let d = diameter * scale;
    let named = picked
        || chat.is_none_or(|chat| chat.id == teams::NOTES)
        || scale >= 0.75
        || graph.nodes.len() <= 12
        || state.hovered == id
        || state.focused == id;
    let width = if named {
        let minimum = if graph.nodes.len() <= 12 { 96.0 } else { 72.0 };
        let needed = context.map_or(0.0, |label| {
            (label.chars().count() as f32 * 6.0 + 8.0).min(180.0)
        });
        (LABEL_WIDTH * scale).max(minimum).max(d).max(needed)
    } else {
        d
    };
    let [x, y] = graph.screen(world);
    let hover_id = id.clone();
    let press_id = id.clone();
    let key_id = id.clone();
    col()
        .key(key)
        .role("button")
        .label(accessible)
        .focusable(true)
        .focus_ring(2.0, 1.0, t.accent)
        .checked(picked)
        .absolute(x - width * 0.5, y - d * 0.5)
        .w(width)
        .items_center()
        .gap(5.0 * scale)
        .cursor(if graph.dragging() {
            rune::Cursor::Grabbing
        } else {
            rune::Cursor::Grab
        })
        .on_hover_change(move |s: &mut State, over, _| {
            if over {
                s.insights.hovered.clone_from(&hover_id);
            } else if s.insights.hovered == hover_id {
                s.insights.hovered = None;
            }
        })
        .on_drag(move |s: &mut State, event, tx| {
            let target = id.as_deref().map_or(Target::Profile, Target::Chat);
            let moved = s.insights.graph.gesture(target, event, tx);
            if event.phase == Phase::Up {
                s.insights.suppress_press = moved;
            }
        })
        .on_key(move |s: &mut State, event, tx| {
            if event.key != rune::Key::Enter || event.mods != rune::Mods::default() {
                return false;
            }
            if !tx.key_repeat() {
                s.insights.selected.clone_from(&key_id);
            }
            true
        })
        .on_press_tx(move |s: &mut State, tx| {
            if !std::mem::take(&mut s.insights.suppress_press) {
                s.insights.selected.clone_from(&press_id);
            }
            tx.instant();
        })
        .child(orb(s, session, chat, d, picked))
        .children(named.then(|| {
            text(name)
                .max_w(width)
                .min_w(0.0)
                .px(3.0)
                .truncate()
                .align(Align::Center)
                .font(rune::TextStyle::new(
                    (14.0 * scale).clamp(10.0, 30.0),
                    600.0,
                ))
                .color(t.text)
                .fill(t.background)
                .radius(3.0)
                .a11y_hidden()
        }))
        .children(context.filter(|_| named).map(|context| {
            text(context)
                .max_w(width)
                .px(3.0)
                .truncate()
                .align(Align::Center)
                .font(rune::TextStyle::new(
                    (12.0 * scale).clamp(10.0, 19.0),
                    500.0,
                ))
                .color(t.text_secondary)
                .fill(t.background)
                .radius(3.0)
                .a11y_hidden()
        }))
}

fn orb(s: &State, session: &Session, chat: Option<&Chat>, d: f32, picked: bool) -> El {
    let t = palette();
    let circle = |size: f32| el().size(size, size).radius(size * 0.5);
    let face = circle(d)
        .center()
        .fill(t.surface)
        .child(match chat {
            Some(chat) if chat.id == teams::NOTES => {
                portrait(s, &session.user.id, &session.user.name, d, false)
            }
            Some(chat) if chat.id.ends_with("@thread.v2") || !chat.avatar_members.is_empty() => {
                chat_avatar(s, chat, &session.user.id, d)
            }
            Some(chat) => portrait(
                s,
                chat_avatar_id(chat, &session.user.id),
                &chat.title,
                d,
                false,
            ),
            None => portrait(s, &session.user.id, &session.user.name, d, true),
        })
        .child(circle(d).absolute(0.0, 0.0).border(1.3, t.accent));
    el().size(d, d)
        .a11y_hidden()
        .child(
            circle(d)
                .absolute(1.5, 2.0)
                .border(1.0, t.accent.alpha(0.30)),
        )
        .child(face)
        .children(picked.then(|| {
            let radius = d * 0.5 + 4.0;
            let center = [radius, radius];
            let marks = (0..24).fold(Shape::new(), |shape, i| {
                let start = i as f32 * TAU / 24.0;
                shape.line(
                    add(center, radial(start, radius)),
                    add(center, radial(start + 0.15, radius)),
                    1.0,
                    LineCap::Round,
                    t.accent,
                )
            });
            canvas(marks)
                .size(radius * 2.0, radius * 2.0)
                .absolute(-4.0, -4.0)
        }))
}

fn portrait(s: &State, id: &str, name: &str, diameter: f32, profile: bool) -> El {
    let t = palette();
    let d = diameter.max(1.0);
    if let Some(image) = s.avatars.get(id) {
        el().size(d, d)
            .clip_rounded(d * 0.5)
            .pixels_fit(image.pixels(), rune::SurfaceFit::Fill)
    } else {
        let initials: String = name
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .take(2)
            .flat_map(char::to_uppercase)
            .collect();
        el().size(d, d)
            .radius(d * 0.5)
            .center()
            .fill(if profile { t.accent } else { t.selected })
            .child(
                text(if initials.is_empty() { "?" } else { &initials })
                    .font(rune::TextStyle::new((d * 0.32).clamp(6.0, 48.0), 650.0))
                    .color(if profile { t.background } else { t.text }),
            )
    }
}

fn count_label(state: &insights::Insights, id: &str) -> String {
    if let Some(count) = state.counts.get(id) {
        format!("{} sent · {} total", count.sent, count.total)
    } else if state.unavailable(id) {
        "History unavailable".into()
    } else {
        "Loading history…".into()
    }
}

fn tooltip(state: &insights::Insights, id: &str) -> Option<El> {
    let count = state.counts.get(id)?;
    let graph = &state.graph;
    let node = graph.nodes.iter().find(|node| node.id == id)?;
    let [x, y] = graph.screen(node.particle.at);
    let [w, h] = graph.viewport;
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return None;
    }
    let label = format!("{} sent · {} total", count.sent, count.total);
    let width = (label.chars().count() as f32 * 7.25 + 16.0).min(w);
    let r = node.diameter * graph.scale() * 0.5;
    let top = y - r - 34.0;
    let t = palette();
    Some(
        el().key("insights-tooltip")
            .absolute(
                (x - width * 0.5).clamp(0.0, w - width),
                if top >= 0.0 {
                    top
                } else {
                    (y + r + 24.0).min((h - 28.0).max(0.0))
                },
            )
            .size(width, 28.0)
            .center()
            .radius(7.0)
            .fill(t.surface)
            .border(1.0, t.border)
            .a11y_hidden()
            .child(text(label).font(type_::LABEL).color(t.text_secondary)),
    )
}

fn caption(state: &insights::Insights, chats: &[Chat], contexts: &HashMap<&str, String>) -> El {
    let t = palette();
    let failed = state.failed();
    let detail = if let Some(chat) = state
        .selected
        .as_ref()
        .and_then(|id| chats.iter().find(|chat| chat.id == *id))
    {
        let name = contexts.get(chat.id.as_str()).map_or_else(
            || chat_name(chat).to_owned(),
            |context| format!("{} ({context})", chat_name(chat)),
        );
        if let Some(count) = state.counts.get(&chat.id) {
            format!(
                "{name} · {} sent by you · {} total messages",
                count.sent, count.total
            )
        } else {
            format!("{name} · {}", count_label(state, &chat.id))
        }
    } else if state.dates.is_some()
        && (!state.counts.is_empty() || (!state.loading() && failed == 0))
    {
        format!("My profile · {} messages sent", state.sent)
    } else {
        "My profile".into()
    };
    row()
        .key("insights-caption")
        .w_full()
        .min_h(42.0)
        .items_center()
        .flex_wrap()
        .gap(8.0)
        .px(12.0)
        .py(5.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
        .child(
            text(detail)
                .key("insights-detail")
                .live_region(rune::LiveRegion::Polite)
                .font(type_::BODY)
                .color(t.text_secondary)
                .wrap(),
        )
        .child(el().grow(1.0))
        .children(state.loading().then(|| {
            text("Loading chat history…")
                .role("status")
                .font(type_::LABEL)
                .color(t.text_secondary)
        }))
        .children((failed > 0).then(|| {
            let status = if state.has_partial_history() {
                "History incomplete"
            } else {
                "History unavailable"
            };
            let chats = if failed == 1 { "chat" } else { "chats" };
            row()
                .items_center()
                .gap(8.0)
                .child(
                    text(format!("{status} for {failed} {chats}"))
                        .role("status")
                        .font(type_::LABEL)
                        .color(t.text_secondary),
                )
                .child(control(
                    "insights-retry",
                    "Retry chat history",
                    "Retry",
                    |s, _| insights::retry(s),
                ))
        }))
        .children(
            (!state.loading()
                && failed == 0
                && state.counts.values().all(|counts| counts.total == 0))
            .then(|| {
                text("No chat messages in this range")
                    .font(type_::LABEL)
                    .color(t.text_secondary)
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> rune::Runtime<State> {
        let mut s = init();
        SCENES
            .iter()
            .find(|(name, _)| *name == "analytics")
            .unwrap()
            .1(&mut s);
        let mut rt = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(false, 1.0));
        rt.poll = Some(crate::poll);
        frames(&mut rt, 4);
        rt
    }

    fn frames(rt: &mut rune::Runtime<State>, count: usize) {
        for _ in 0..count {
            rt.frame(1.0 / 60.0);
        }
    }

    fn press(rt: &mut rune::Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        frames(rt, 4);
    }

    #[test]
    fn ranges_and_counts_only_tooltips_work_with_pointer_and_keyboard() {
        let mut rt = runtime();
        let day = rt.state.insights.today();
        assert_eq!(
            rt.state.insights.dates,
            Some(insights::Dates::ending(day, 7))
        );
        for (key, from) in [
            ("insights-1-day", day),
            ("insights-7-days", day - chrono::Days::new(6)),
            ("insights-30-days", day - chrono::Days::new(29)),
            (
                "insights-6-months",
                chrono::NaiveDate::from_ymd_opt(2026, 5, 14).unwrap(),
            ),
            ("insights-all-time", chrono::NaiveDate::MIN),
        ] {
            press(&mut rt, "insights-range");
            press(&mut rt, key);
            assert_eq!(
                rt.state.insights.dates,
                Some(insights::Dates { from, to: day })
            );
        }
        press(&mut rt, "insights-range");
        press(&mut rt, "insights-custom");
        assert!(rt.find("insights-from").is_some());
        assert_eq!(rt.state.insights.from.text, "2026-05-14");
        rt.update(|s, _| {
            s.insights
                .from
                .replace_text(day.succ_opt().unwrap().to_string())
        });
        press(&mut rt, "insights-apply");
        assert_eq!(
            rt.state.insights.date_error,
            Some("From must be on or before To.")
        );
        rt.update(|s, _| {
            s.insights.from.replace_text(day.to_string());
            s.insights.to.replace_text(day.to_string());
        });
        press(&mut rt, "insights-apply");
        assert_eq!(
            rt.state.insights.dates,
            Some(insights::Dates { from: day, to: day })
        );
        assert!(rt.state.insights.date_error.is_none());
        frames(&mut rt, 100);

        let (x, y) = rt.find("insights-node/b").unwrap();
        rt.pointer_move(x, y);
        frames(&mut rt, 4);
        let elements = rt.elements();
        let tooltip = elements
            .iter()
            .position(|el| el.key.as_deref() == Some("insights-tooltip"))
            .unwrap();
        let counts = rt.state.insights.counts["b"];
        let expected = format!("{} sent · {} total", counts.sent, counts.total);
        assert_eq!(
            elements
                .iter()
                .find(|el| el.parent == Some(tooltip))
                .unwrap()
                .text
                .as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(rt.state.insights.hovered.as_deref(), Some("b"));

        rt.pointer_move(5.0, 5.0);
        rt.update(|_, tx| tx.focus("insights-node/a", true));
        frames(&mut rt, 4);
        assert!(rt.find("insights-tooltip").is_some());
        assert_eq!(rt.state.insights.focused.as_deref(), Some("a"));

        rt.key(rune::KeyEvent {
            key: rune::Key::Enter,
            mods: Default::default(),
        });
        frames(&mut rt, 4);
        assert_eq!(rt.state.insights.selected.as_deref(), Some("a"));
        press(&mut rt, PROFILE_KEY);
        let caption = format!("My profile · {} messages sent", rt.state.insights.sent);
        assert!(rt.elements().iter().any(|el| {
            el.key.as_deref() == Some("insights-detail")
                && el.text.as_deref() == Some(caption.as_str())
        }));
    }

    #[test]
    fn self_chat_has_a_you_bubble_with_its_own_counts() {
        use chrono::{Local, NaiveDate, TimeZone};

        let mut s = init();
        demo(&mut s);
        let Screen::Main { chats, session } = &mut s.screen else {
            unreachable!();
        };
        chats.extend(teams::parse_chats(
            &serde_json::json!({"conversations": [{"id": teams::NOTES}]}),
            &session.user,
        ));
        let day = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let time = Local
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap();
        s.conversation_section = ConversationSection::Analytics;
        insights::demo(&mut s, day, [(teams::NOTES, time, true)].into_iter());
        let mut rt = rune::Runtime::new(s, view, (640.0, 400.0), rune::Env::new(false, 1.0));
        rt.poll = Some(crate::poll);
        frames(&mut rt, 4);

        let key = format!("insights-node/{}", teams::NOTES);
        assert!(rt.find(PROFILE_KEY).is_some());
        assert!(rt.elements().iter().any(|node| {
            node.key.as_deref() == Some(key.as_str())
                && node.label.as_deref() == Some("You, 1 sent · 1 total")
        }));
        press(&mut rt, &key);
        assert!(rt.elements().iter().any(|node| {
            node.key.as_deref() == Some("insights-detail")
                && node.text.as_deref() == Some("You · 1 sent by you · 1 total messages")
        }));
    }

    #[test]
    fn same_named_direct_and_group_chats_have_distinct_labels_and_counts() {
        use chrono::{Local, NaiveDate, TimeZone};
        const DIRECT: &str = "19:me_taylor@unq.gbl.spaces";
        const GROUP_WITH_HISTORY: &str = "19:group-a@thread.v2";
        const GROUP_WITHOUT_HISTORY: &str = "19:group-b@thread.v2";
        const BOT: &str = "19:me_github@unq.gbl.spaces";

        let mut s = init();
        demo(&mut s);
        let Screen::Main { chats, .. } = &mut s.screen else {
            unreachable!()
        };
        for (chat, id) in chats
            .iter_mut()
            .zip([DIRECT, GROUP_WITH_HISTORY, GROUP_WITHOUT_HISTORY])
        {
            chat.id = id.into();
            chat.title = "Taylor".into();
        }
        chats[2].time.clear();
        chats[1].avatar_id = "28:github".into();
        chats.push(Chat {
            id: BOT.into(),
            ..chats[1].clone()
        });
        let day = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let time = Local
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap();
        s.conversation_section = ConversationSection::Analytics;
        insights::demo(
            &mut s,
            day,
            [
                (DIRECT, time, true),
                (DIRECT, time, false),
                (GROUP_WITH_HISTORY, time, false),
                (BOT, time, true),
            ]
            .into_iter(),
        );
        let mut rt = rune::Runtime::new(s, view, (800.0, 520.0), rune::Env::new(false, 1.0));
        rt.poll = Some(crate::poll);
        frames(&mut rt, 4);

        let nodes = rt.elements();
        let label = |id| {
            let key = format!("insights-node/{id}");
            nodes
                .iter()
                .find(|el| el.key.as_deref() == Some(&key))
                .and_then(|el| el.label.as_deref())
                .unwrap()
        };
        assert_eq!(label(DIRECT), "Taylor, Direct chat, 1 sent · 2 total");
        assert!(rt.find(&format!("insights-node/{BOT}")).is_none());
        assert_eq!(rt.state.insights.sent, 1);
        assert!(label(GROUP_WITH_HISTORY).starts_with("Taylor, Group chat · "));
        assert!(label(GROUP_WITH_HISTORY).ends_with("0 sent · 1 total"));
        assert!(
            rt.find(&format!("insights-node/{GROUP_WITHOUT_HISTORY}"))
                .is_none()
        );
        press(&mut rt, &format!("insights-node/{DIRECT}"));
        assert!(
            rt.elements()
                .iter()
                .any(|el| el.key.as_deref() == Some("insights-detail")
                    && el.text.as_deref()
                        == Some("Taylor (Direct chat) · 1 sent by you · 2 total messages"))
        );

        let previous = day.pred_opt().unwrap().to_string();
        rt.update(|s, _| {
            s.insights.from.replace_text(previous.clone());
            s.insights.to.replace_text(previous);
            insights::apply_custom(s);
        });
        frames(&mut rt, 4);
        assert!(rt.find(PROFILE_KEY).is_some());
        assert!(rt.state.insights.graph.nodes.is_empty());
        assert_eq!(rt.state.insights.counts[DIRECT].total, 0);
        assert!(rt.state.insights.selected.is_none());
    }

    #[test]
    fn dragging_profile_and_chat_settles_at_message_based_distances() {
        let mut rt = runtime();
        frames(&mut rt, 100);
        let (x, y) = rt.find(PROFILE_KEY).unwrap();
        let (dx, dy) = (46.0, 24.0);
        rt.pointer_down(x, y);
        frames(&mut rt, 1);
        rt.pointer_move(x + dx, y + dy);
        frames(&mut rt, 4);
        assert_ne!(rt.state.insights.graph.hub.at, [0.0; 2]);
        rt.pointer_up(x + dx, y + dy);
        frames(&mut rt, 100);
        let graph = &rt.state.insights.graph;
        assert!(length(sub(scaled(graph.hub.at, graph.scale()), [dx, dy])) < 0.5);

        let position = |graph: &Graph, id: &str| {
            graph
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap()
                .particle
                .at
        };
        for side in [1.0, -1.0] {
            let graph = &rt.state.insights.graph;
            let neighbor = graph
                .nodes
                .iter()
                .find(|node| node.id != "b" && (node.particle.at[0] - graph.hub.at[0]) * side > 0.0)
                .unwrap()
                .id
                .clone();
            let dragged_before = position(graph, "b");
            let neighbor_before = position(graph, &neighbor);
            let drop_ray = sub(neighbor_before, graph.hub.at);
            let [dx, dy] = scaled(sub(neighbor_before, dragged_before), graph.scale());
            let (x, y) = rt.find("insights-node/b").unwrap();
            rt.pointer_down(x, y);
            frames(&mut rt, 1);
            rt.pointer_move(x + dx, y + dy);
            frames(&mut rt, 15);
            let graph = &rt.state.insights.graph;
            assert_eq!(position(graph, &neighbor), neighbor_before);
            assert!(length(sub(position(graph, "b"), dragged_before)) > 1.0);
            rt.pointer_up(x + dx, y + dy);
            frames(&mut rt, 100);
            let graph = &rt.state.insights.graph;
            let [x, y] = sub(position(graph, "b"), graph.hub.at);
            assert!(x * side > 0.0);
            let difference = y.atan2(x) - drop_ray[1].atan2(drop_ray[0]);
            assert!(((difference + PI).rem_euclid(TAU) - PI).abs() < 0.15);
            assert!(length(sub(position(graph, &neighbor), neighbor_before)) > 1.0);
            assert_even_angles(graph);
            for node in &graph.nodes {
                assert!((length(sub(node.particle.at, graph.hub.at)) - node.distance).abs() < 0.12);
            }
            assert!(rt.state.insights.selected.is_none());
        }

        let graph = &rt.state.insights.graph;
        let before_cancel = position(graph, "b");
        let far_before_cancel = position(graph, "d");
        let (x, y) = rt.find("insights-node/b").unwrap();
        rt.pointer_down(x, y);
        frames(&mut rt, 1);
        rt.pointer_move(x + 24.0, y - 18.0);
        frames(&mut rt, 15);
        assert_ne!(position(&rt.state.insights.graph, "b"), before_cancel);
        rt.key(rune::KeyEvent {
            key: rune::Key::Escape,
            mods: Default::default(),
        });
        frames(&mut rt, 100);
        let graph = &rt.state.insights.graph;
        assert_eq!(position(graph, "b"), before_cancel);
        assert_eq!(position(graph, "d"), far_before_cancel);

        press(&mut rt, "insights-zoom-in");
        assert_eq!(rt.state.insights.graph.zoom, 1.25);
        rt.wheel_event(WheelEvent::new(0.0, -120.0).at(320.0, 200.0));
        frames(&mut rt, 4);
        assert!(rt.state.insights.graph.zoom < 1.25);
        press(&mut rt, "insights-fit");
        let graph = &rt.state.insights.graph;
        assert_eq!(graph.zoom, 1.0);
        assert!(length(sub(graph.screen(graph.hub.at), scaled(graph.viewport, 0.5))) < 0.5);
    }

    fn assert_even_angles(graph: &Graph) {
        let mut angles: Vec<_> = graph
            .nodes
            .iter()
            .map(|node| node.angle.rem_euclid(TAU))
            .collect();
        angles.sort_by(f32::total_cmp);
        let gap = TAU / angles.len() as f32;
        angles.push(angles[0] + TAU);
        assert!(
            angles
                .windows(2)
                .all(|pair| (pair[1] - pair[0] - gap).abs() < 0.001)
        );
    }

    #[test]
    fn drop_evenly_spaces_the_entire_chain_in_one_direction() {
        for heading in [-1.1, 1.25] {
            let mut graph = Graph::default();
            graph.set_counts(
                &"abcdefgh"
                    .chars()
                    .map(|id| (id.to_string(), Counts { sent: 1, total: 1 }))
                    .collect(),
            );
            let before: Vec<_> = graph.nodes.iter().map(|node| node.angle).collect();

            graph.drop_chat("b", radial(heading, 150.0));

            assert_even_angles(&graph);
            let shifts: Vec<_> = graph
                .nodes
                .iter()
                .zip(before)
                .filter(|(node, _)| node.id != "b")
                .map(|(node, angle)| (node.angle - angle + PI).rem_euclid(TAU) - PI)
                .collect();
            assert!(
                shifts
                    .iter()
                    .all(|shift| shift.abs() > 0.001 && shift.signum() == shifts[0].signum())
            );
        }
    }
}
