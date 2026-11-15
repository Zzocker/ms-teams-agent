//! Scroll viewports with a draggable bar and a gliding wheel.

use crate::*;

/// A scroll bar's thumb, where it's drawn, and where its viewport was last scrolled to, for dragging.
#[derive(Clone)]
pub(crate) struct Bar {
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
pub(crate) fn with_bar(key: &'static str, list: El, bar: &Bar) -> El {
    let b = bar.clone();
    let list = list.key(key).on_scroll_offset(move |at: ScrollPos| {
        b.at.set(at);
        b.rect.snap(thumb(at));
    })
    .on_wheel({
        let (at, aim) = (bar.at.clone(), Cell::new(0.0_f32));
        // A notched wheel jumps 120pt a click on Linux: glide half that instead. Trackpads stay 1:1.
        move |_: &mut State, e: WheelEvent, tx: &mut Tx| {
            if e.precise || e.dx != 0.0 {
                return false;
            }
            let now = at.get();
            let (y, max) = (now.offset[1], now.max[1]);
            // Mid-glide, add to where it's headed so quick clicks add up; a target far off is stale.
            let from = if (aim.get() - y).abs() < now.viewport[1] { aim.get() } else { y };
            aim.set((from - e.dy * 0.5).clamp(0.0, max));
            tx.scroll_to_with(key, 0.0, aim.get(), Spring::new(0.2, 0.0));
            true
        }
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
pub(crate) fn offset_at(at: ScrollPos, top: f32) -> f32 {
    let [_, _, _, h] = thumb(at);
    let room = at.viewport[1] - 4.0 - h;
    if room <= 0.0 { 0.0 } else { ((top - 2.0) / room).clamp(0.0, 1.0) * at.max[1] }
}

/// The thumb's `[x, y, w, h]` in the viewport: as tall as the share of the content in view, at least 24pt; none when nothing scrolls.
pub(crate) fn thumb(at: ScrollPos) -> [f32; 4] {
    let ([w, vh], max, y) = (at.viewport, at.max[1], at.offset[1]);
    if max <= 0.5 {
        return [0.0; 4];
    }
    let track = vh - 4.0;
    let h = (track * vh / (vh + max)).clamp(24.0_f32.min(track), track);
    [w - 6.0, 2.0 + (y / max).clamp(0.0, 1.0) * (track - h), 4.0, h]
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_notched_wheel_glides_half_a_click() {
        fn view(s: &State, _: &mut Cx<State>) -> El {
            let rows = (0..100).map(|i| el().key(format!("r{i}")).h(40.0));
            col().h(400.0).child(with_bar("list", col().children(rows).scroll(ScrollSpec { stick_bottom: false }).grow(1.0).min_h(0.0), &s.msg_bar))
        }
        let mut rt = rune::Runtime::new(init(), view, (300.0, 400.0), rune::Env::new(true, 1.0));
        rt.frame(1.0 / 120.0);
        let y = |rt: &rune::Runtime<State>| rt.state.msg_bar.at.get().offset[1];
        rt.wheel_event(WheelEvent::new(0.0, -120.0).at(100.0, 200.0));
        rt.wheel_event(WheelEvent::new(0.0, -120.0).at(100.0, 200.0));
        rt.frame(1.0 / 120.0);
        assert!(y(&rt) > 0.0 && y(&rt) < 120.0, "glides, not jumps: {}", y(&rt));
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        assert!((y(&rt) - 120.0).abs() < 0.5, "two clicks add up to 2 × 60: {}", y(&rt));
    }
}
