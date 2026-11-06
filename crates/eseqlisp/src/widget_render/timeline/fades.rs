//! Audio-region fades on the timeline (docs/audio-tracks-spec.md §8.1):
//! fade-in, fade-out and crossfade curves drawn over an item's body, and the
//! handles that drag them, after Ableton's arrangement clips. A square at
//! the top of the body sets a fade's length; a diamond on the curve sets its
//! shape. Where two items abut, the later one's `:crossfade` window spans
//! their shared edge and its two ends are the handles.
//!
//! The widget stays dumb: the host passes each item's fades in beats
//! (`:fades {:in :out :in-curve :out-curve :xfade-pre :xfade-post
//! :xfade-next}`) and receives handle drags as `:fade-item` actions with the
//! handle's new song time (length handles) or the 0..1 level its curve
//! should pass at its midpoint (curve handles).

use super::*;

/// One item's fades, in beats.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ItemFades {
    pub fade_in: f64,
    pub fade_out: f64,
    /// Ramp exponents (`audio::fade::curve_power`).
    pub in_power: f32,
    pub out_power: f32,
    /// The crossfade with the item before: beats before and after this
    /// item's start.
    pub crossfade: Option<(f64, f64)>,
    /// Beats before this item's end where the next item's crossfade starts.
    pub crossfade_next: Option<f64>,
}

pub(super) use crate::audio::fade::FadeHandle;
use crate::audio::fade::{crossfade_gains, curve_power, fade_level};

/// The keyword a handle is reported as.
pub(super) fn handle_keyword(handle: FadeHandle) -> Value {
    keyword(handle.name())
}

/// `:fades` of an item map, when it has any.
pub(super) fn parse_fades(value: &Value) -> Option<ItemFades> {
    let map = get_map(value)?;
    let number = |key: &str| map.get(key).and_then(as_number).filter(|n| n.is_finite());
    let beats = |key: &str| number(key).map(|n| n.max(0.0));
    let crossfade = match (beats("xfade-pre"), beats("xfade-post")) {
        (None, None) => None,
        (pre, post) => Some((pre.unwrap_or(0.0), post.unwrap_or(0.0))),
    };
    Some(ItemFades {
        fade_in: beats("in").unwrap_or(0.0),
        fade_out: beats("out").unwrap_or(0.0),
        in_power: curve_power(number("in-curve").unwrap_or(0.0) as f32),
        out_power: curve_power(number("out-curve").unwrap_or(0.0) as f32),
        crossfade,
        crossfade_next: beats("xfade-next"),
    })
}

/// Half a handle's hit box, in cells: generous enough to grab at any zoom.
const HIT_HALF_COLS: f32 = 0.55;
const HIT_HALF_ROWS: f32 = 0.42;
/// Half a handle's drawn side, in design pixels.
const DRAWN_HALF_PX: f32 = 4.0;

/// Where one handle sits: its center in cells and, for a length handle, the
/// song time it marks.
#[derive(Clone, Copy, Debug)]
pub(super) struct HandleSpot {
    pub handle: FadeHandle,
    pub center: [f32; 2],
    pub time: f64,
}

/// The item's body as `(top, height)` rows, and its true (unclamped) x
/// extent.
struct Geometry {
    body_top: f32,
    body_height: f32,
    x0: f32,
    x1: f32,
    visible: (f32, f32),
}

fn geometry(view: &TimelineView, item: &TimelineItem) -> Option<Geometry> {
    let rect = view.item_rect(item)?;
    let bar_bottom = view.item_title_bar_bottom(&rect)?;
    let body_height = rect.row + rect.height - bar_bottom;
    if body_height < 0.6 {
        return None;
    }
    let axis = UnclampedTimeAxis::new(view);
    let content = view.content_rect();
    Some(Geometry {
        body_top: bar_bottom,
        body_height,
        x0: axis.x(item.start),
        x1: axis.x(item.end),
        visible: (content.col, content.col + content.width),
    })
}

/// The handles of `item`, in hit-test priority order.
pub(super) fn handle_spots(view: &TimelineView, item: &TimelineItem, fades: &ItemFades) -> Vec<HandleSpot> {
    match geometry(view, item) {
        Some(g) => spots_in(view, item, fades, &g),
        None => Vec::new(),
    }
}

fn spots_in(view: &TimelineView, item: &TimelineItem, fades: &ItemFades, g: &Geometry) -> Vec<HandleSpot> {
    let axis = UnclampedTimeAxis::new(view);
    let half = HIT_HALF_COLS * 0.7;
    // A length handle sits on its fade's end but never outside the item, so
    // a zero-length fade's square sits in the body's top corner.
    let clamp_x = |x: f32| x.clamp(g.x0 + half, (g.x1 - half).max(g.x0 + half));
    let top = g.body_top + HIT_HALF_ROWS * 0.8;
    let bottom = g.body_top + g.body_height;
    let mut spots = Vec::new();
    let (in_handle, in_time) = match fades.crossfade {
        Some((_, post)) => (FadeHandle::CrossfadeEnd, item.start + post),
        None => (FadeHandle::In, item.start + fades.fade_in),
    };
    let (out_handle, out_time) = match fades.crossfade_next {
        Some(pre) => (FadeHandle::CrossfadeStart, item.end - pre),
        None => (FadeHandle::Out, item.end - fades.fade_out),
    };
    spots.push(HandleSpot { handle: in_handle, center: [clamp_x(axis.x(in_time)), top], time: in_time });
    spots.push(HandleSpot { handle: out_handle, center: [clamp_x(axis.x(out_time)), top], time: out_time });
    // A curve handle needs room between the fade's ends to be grabbed apart
    // from its length handle.
    let room = HIT_HALF_COLS * 4.0;
    if fades.crossfade.is_none() && fades.fade_in > 0.0 && axis.x(in_time) - g.x0 > room {
        let level = fade_level(0.5, fades.in_power);
        spots.push(HandleSpot {
            handle: FadeHandle::InCurve,
            center: [axis.x(item.start + fades.fade_in * 0.5), bottom - level * g.body_height],
            time: item.start + fades.fade_in * 0.5,
        });
    }
    if fades.crossfade_next.is_none() && fades.fade_out > 0.0 && g.x1 - axis.x(out_time) > room {
        let level = fade_level(0.5, fades.out_power);
        spots.push(HandleSpot {
            handle: FadeHandle::OutCurve,
            center: [axis.x(item.end - fades.fade_out * 0.5), bottom - level * g.body_height],
            time: item.end - fades.fade_out * 0.5,
        });
    }
    spots.retain(|spot| spot.center[0] >= g.visible.0 && spot.center[0] <= g.visible.1);
    spots
}

/// The fade handle under the pointer: its item, the spot and the body rows
/// (top, height) a curve drag measures against.
pub(super) fn hit(
    view: &TimelineView,
    local_col: f32,
    local_row: f32,
) -> Option<(TimelineItem, HandleSpot, (f32, f32))> {
    let mut best: Option<(f32, usize, HandleSpot, (f32, f32))> = None;
    for (index, item) in view.items.iter().enumerate().rev() {
        let Some(fades) = item.fades.as_ref() else {
            continue;
        };
        let Some(g) = geometry(view, item) else {
            continue;
        };
        // Handles sit inside the item: skip the ones the pointer is nowhere near.
        if local_col < g.x0 - HIT_HALF_COLS || local_col > g.x1 + HIT_HALF_COLS {
            continue;
        }
        for spot in spots_in(view, item, fades, &g) {
            let dx = (local_col - spot.center[0]).abs();
            let dy = (local_row - spot.center[1]).abs();
            if dx > HIT_HALF_COLS || dy > HIT_HALF_ROWS {
                continue;
            }
            // The nearest handle wins: abutting items put the earlier one's
            // fade-out square right beside the later one's fade-in square.
            let distance = dx / HIT_HALF_COLS + dy / HIT_HALF_ROWS;
            if best.as_ref().is_none_or(|(d, ..)| distance < *d) {
                best = Some((distance, index, spot, (g.body_top, g.body_height)));
            }
        }
    }
    best.map(|(_, index, spot, body)| (view.items[index].clone(), spot, body))
}

/// Points of a curve from `t0` to `t1` (song time) at `level(fraction)`,
/// clipped to the visible content.
fn curve_points(
    axis: &UnclampedTimeAxis,
    g: &Geometry,
    t0: f64,
    t1: f64,
    level: impl Fn(f64) -> f32,
) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    const STEPS: usize = 32;
    let bottom = g.body_top + g.body_height;
    let (mut curve, mut tops) = (Vec::with_capacity(STEPS + 1), Vec::with_capacity(STEPS + 1));
    if t1 <= t0 {
        return (curve, tops);
    }
    // Sample only the visible stretch, from exactly the view's edge: points
    // spread over the whole fade and then culled would start the curve a
    // scroll-dependent distance inside the edge, and it would jiggle.
    let (v0, v1) = (axis.time(g.visible.0), axis.time(g.visible.1));
    let (from, to) = (t0.max(v0), t1.min(v1));
    if to <= from {
        return (curve, tops);
    }
    for step in 0..=STEPS {
        let time = from + (to - from) * step as f64 / STEPS as f64;
        let x = axis.x(time).clamp(g.visible.0, g.visible.1);
        let fraction = (time - t0) / (t1 - t0);
        curve.push([x, bottom - level(fraction).clamp(0.0, 1.0) * g.body_height]);
        tops.push([x, g.body_top]);
    }
    (curve, tops)
}

const CURVE_INK: crate::backend::Color = crate::backend::Color { r: 0.03, g: 0.03, b: 0.04, a: 0.9 };
const FADE_SHADE: crate::backend::Color = crate::backend::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.24 };
const HANDLE_ACTIVE: crate::backend::Color = crate::backend::Color { r: 0.56, g: 0.86, b: 1.0, a: 1.0 };

/// Draw `item`'s fade curves (always) and its handles (while `hover` names
/// it; `Some(handle)` lights the one under the pointer).
pub(super) fn push_fades(
    primitives: &mut Vec<GpuPrimitive>,
    view: &TimelineView,
    item: &TimelineItem,
    fades: &ItemFades,
    item_color: crate::backend::Color,
    hover: Option<Option<FadeHandle>>,
    viewport: super::super::WidgetViewport,
) {
    let Some(g) = geometry(view, item) else {
        return;
    };
    let axis = UnclampedTimeAxis::new(view);
    let mut mesh = super::super::stroke::ShadedMesh::new();
    let fade = |mesh: &mut super::super::stroke::ShadedMesh,
                t0: f64,
                t1: f64,
                level: &dyn Fn(f64) -> f32,
                shade_above: bool| {
        let (curve, tops) = curve_points(&axis, &g, t0, t1, level);
        if curve.len() < 2 {
            return;
        }
        if shade_above {
            mesh.push_band(&tops, &curve, FADE_SHADE);
        }
        mesh.push_polyline(&curve, CURVE_INK, viewport, 0.6);
    };
    match fades.crossfade {
        Some((pre, post)) => {
            let (t0, t1) = (item.start - pre, item.start + post);
            // The whole window darkens, with both gain curves over it.
            let (curve, tops) = curve_points(&axis, &g, t0, t1, &|_| 0.0);
            mesh.push_band(&tops, &curve, FADE_SHADE);
            fade(&mut mesh, t0, t1, &|t| crossfade_gains(t).1, false);
            fade(&mut mesh, t0, t1, &|t| crossfade_gains(t).0, false);
        }
        None if fades.fade_in > 0.0 => {
            let power = fades.in_power;
            fade(&mut mesh, item.start, item.start + fades.fade_in, &|t| fade_level(t, power), true);
        }
        None => {}
    }
    if fades.crossfade_next.is_none() && fades.fade_out > 0.0 {
        let power = fades.out_power;
        fade(&mut mesh, item.end - fades.fade_out, item.end, &|t| fade_level(1.0 - t, power), true);
    }
    if let Some(hovered) = hover {
        let half_px = super::super::ui_design_px(DRAWN_HALF_PX);
        let (half_w, half_h) = (half_px / viewport.cell_w.max(1.0), half_px / viewport.cell_h.max(1.0));
        let lifted = crate::backend::Color {
            r: (item_color.r * 1.25).min(1.0),
            g: (item_color.g * 1.25).min(1.0),
            b: (item_color.b * 1.25).min(1.0),
            a: 1.0,
        };
        for spot in handle_spots(view, item, fades) {
            let [x, y] = spot.center;
            let fill = if hovered == Some(spot.handle) { HANDLE_ACTIVE } else { lifted };
            let outline: Vec<[f32; 2]> = if spot.handle.is_curve() {
                // A diamond on the curve.
                let (w, h) = (half_w * 1.2, half_h * 1.2);
                mesh.push_band(&[[x - w, y], [x, y - h], [x + w, y]], &[[x - w, y], [x, y + h], [x + w, y]], fill);
                vec![[x - w, y], [x, y - h], [x + w, y], [x, y + h], [x - w, y]]
            } else {
                // A square at the top of the body.
                mesh.push_band(&[[x - half_w, y - half_h], [x + half_w, y - half_h]], &[[x - half_w, y + half_h], [x + half_w, y + half_h]], fill);
                vec![
                    [x - half_w, y - half_h],
                    [x + half_w, y - half_h],
                    [x + half_w, y + half_h],
                    [x - half_w, y + half_h],
                    [x - half_w, y - half_h],
                ]
            };
            mesh.push_polyline(&outline, CURVE_INK, viewport, 0.5);
        }
    }
    mesh.push_into(primitives);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: f64, start: f64, end: f64, fades: Vec<(&str, f64)>) -> Value {
        map_value(vec![
            ("id", Value::Number(id)),
            ("lane", Value::Number(0.0)),
            ("start", Value::Number(start)),
            ("end", Value::Number(end)),
            (
                "fades",
                map_value(fades.into_iter().map(|(key, value)| (key, Value::Number(value))).collect()),
            ),
        ])
    }

    /// One cell per beat over 16 beats, an 8-row lane with a 2-row title
    /// bar: bodies span rows 2..8. A [0, 8) fades in over 3 beats and
    /// crossfades into B [8, 12) over a beat before their shared edge; B
    /// fades out over its last 3.
    fn view() -> TimelineView {
        let props = HashMap::from([
            (
                "items".to_string(),
                list_value(vec![
                    item(1.0, 0.0, 8.0, vec![("in", 3.0), ("xfade-next", 1.0)]),
                    item(2.0, 8.0, 12.0, vec![("xfade-pre", 1.0), ("xfade-post", 0.0), ("out", 3.0)]),
                ]),
            ),
            ("view-start".to_string(), Value::Number(0.0)),
            ("view-duration".to_string(), Value::Number(16.0)),
            ("header-height".to_string(), Value::Number(0.0)),
            ("title-bar-height".to_string(), Value::Number(2.0)),
        ]);
        TimelineView::from_props(&props, Rect { row: 0.0, col: 0.0, width: 16.0, height: 8.0 })
    }

    fn hit_at(view: &TimelineView, col: f32, row: f32) -> Option<(f64, FadeHandle)> {
        hit(view, col, row).map(|(item, spot, _)| (as_number(&item.id).unwrap(), spot.handle))
    }

    #[test]
    fn handles_sit_at_the_top_of_the_body_on_each_fade_end() {
        let view = view();
        let top = 2.0 + HIT_HALF_ROWS * 0.8;
        assert_eq!(hit_at(&view, 3.0, top), Some((1.0, FadeHandle::In)), "the fade-in's end");
        assert_eq!(hit_at(&view, 7.0, top), Some((1.0, FadeHandle::CrossfadeStart)), "the crossfade's start");
        // B's crossfade ends at its start (no post window): the square sits
        // just inside B, beside A's corner.
        assert_eq!(hit_at(&view, 8.4, top), Some((2.0, FadeHandle::CrossfadeEnd)));
        assert_eq!(hit_at(&view, 9.0, top), Some((2.0, FadeHandle::Out)), "the fade-out's start");
        assert_eq!(hit_at(&view, 5.0, top), None, "between fades the body is a plain surface");
        assert_eq!(hit_at(&view, 3.0, 6.0), None, "below the square");
    }

    #[test]
    fn a_curve_handle_sits_on_the_curve_midpoint() {
        let view = view();
        // A linear 3-beat fade-in passes half level at beat 1.5: row 8 - 3.
        assert_eq!(hit_at(&view, 1.5, 5.0), Some((1.0, FadeHandle::InCurve)));
        // B's 3-beat fade-out has room for its diamond too.
        assert_eq!(hit_at(&view, 10.5, 5.0), Some((2.0, FadeHandle::OutCurve)));
        // A crossfade has no curve handle.
        assert_eq!(hit_at(&view, 7.5, 5.0), None);
    }

    #[test]
    fn dragging_a_square_moves_its_time_and_a_diamond_sets_a_level() {
        let view = view();
        let top = 2.0 + HIT_HALF_ROWS * 0.8;
        let gesture = view.begin_gesture(3.0, top).expect("a fade gesture");
        let action = view.handle_pointer_drag(4.5, top + 3.0, Some(&gesture)).expect("a fade action");
        let action = get_map(&action).unwrap();
        assert_eq!(action.get("type"), Some(&keyword(":fade-item")));
        assert_eq!(action.get("handle"), Some(&keyword(":in")));
        assert_eq!(action.get("value").and_then(as_number), Some(4.5), "follows the pointer, not the row");

        let gesture = view.begin_gesture(1.5, 5.0).expect("a curve gesture");
        let action = view.handle_pointer_drag(9.0, 3.5, Some(&gesture)).expect("a curve action");
        let action = get_map(&action).unwrap();
        assert_eq!(action.get("handle"), Some(&keyword(":in-curve")));
        // Row 3.5 of a body spanning 2..8 is three quarters up.
        assert_eq!(action.get("value").and_then(as_number), Some(0.75));
        let finish = view.handle_pointer_up(9.0, 3.5, Some(&gesture)).expect("a finish");
        assert_eq!(get_map(&finish).unwrap().get("type"), Some(&keyword(":finish-fade-item")));
    }

    #[test]
    fn a_fade_in_square_never_drags_past_its_item_end() {
        let view = view();
        let top = 2.0 + HIT_HALF_ROWS * 0.8;
        let gesture = view.begin_gesture(3.0, top).unwrap();
        let action = view.handle_pointer_drag(14.0, top, Some(&gesture)).unwrap();
        assert_eq!(get_map(&action).unwrap().get("value").and_then(as_number), Some(8.0));
    }

    #[test]
    fn a_fade_cut_by_the_view_edge_draws_from_exactly_that_edge() {
        // Scrolled 1.3 beats in: a 4-beat fade-in from beat 0 is cut at the
        // left edge, a fade-out ending at beat 20 at the right.
        for view_start in [1.3, 1.7, 2.05] {
            let props = HashMap::from([
                ("items".to_string(), list_value(vec![item(1.0, 0.0, 20.0, vec![("in", 4.0), ("out", 6.0)])])),
                ("view-start".to_string(), Value::Number(view_start)),
                ("view-duration".to_string(), Value::Number(16.0)),
                ("header-height".to_string(), Value::Number(0.0)),
                ("title-bar-height".to_string(), Value::Number(2.0)),
            ]);
            let view = TimelineView::from_props(&props, Rect { row: 0.0, col: 0.0, width: 16.0, height: 8.0 });
            let item = view.items[0].clone();
            let g = geometry(&view, &item).expect("geometry");
            let axis = UnclampedTimeAxis::new(&view);
            let (fade_in, _) = curve_points(&axis, &g, 0.0, 4.0, |t| t as f32);
            assert!((fade_in[0][0] - g.visible.0).abs() < 1e-3, "{view_start}: starts at the left edge");
            // The level there is the fade's own level at that time.
            let expected = 8.0 - (view_start / 4.0) as f32 * g.body_height;
            assert!((fade_in[0][1] - expected).abs() < 1e-3, "{view_start}: {} vs {expected}", fade_in[0][1]);
            let (fade_out, _) = curve_points(&axis, &g, 14.0, 20.0, |t| 1.0 - t as f32);
            assert!((fade_out.last().unwrap()[0] - g.visible.1).abs() < 1e-3, "{view_start}: ends at the right edge");
        }
    }

    #[test]
    fn fades_parse_from_beats_and_a_crossfade_needs_either_side() {
        let map = |entries: Vec<(&str, f64)>| {
            map_value(entries.into_iter().map(|(key, value)| (key, Value::Number(value))).collect())
        };
        let fades = parse_fades(&map(vec![("in", 1.0), ("out", 0.5), ("in-curve", 1.0)])).unwrap();
        assert_eq!((fades.fade_in, fades.fade_out, fades.crossfade), (1.0, 0.5, None));
        assert!((fades.in_power - 0.125).abs() < 1e-6);
        let crossfaded = parse_fades(&map(vec![("xfade-pre", 2.0)])).unwrap();
        assert_eq!(crossfaded.crossfade, Some((2.0, 0.0)));
    }
}
