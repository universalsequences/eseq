//! The arrangement's loop brace (Ableton's loop): a bar in the ruler's band
//! row from `:loop-start` to `:loop-end`, with a triangle at each end
//! pointing in. Hollow while looping is off (`:loop-on` 0), filled while on.
//! Dragging its body moves it, dragging an end resizes it; the host hears
//! `:loop-brace (:start :end)` every drag frame and `:finish-loop-brace`
//! once on release, both snapped to the visible grid.

use super::*;
use crate::widget_render::GpuTrianglePrimitive;

/// `:loop-brace true` turns the ruler's band into the brace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LoopBrace {
    pub start: f64,
    pub end: f64,
    pub on: bool,
}

/// Which part of the brace a press grabbed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BracePart {
    Start,
    End,
    Body,
}

impl BracePart {
    pub fn keyword(self) -> Value {
        keyword(match self {
            BracePart::Start => ":start",
            BracePart::End => ":end",
            BracePart::Body => ":body",
        })
    }

    fn parse(value: &Value) -> Option<Self> {
        match value {
            Value::Keyword(k) if k == "start" => Some(BracePart::Start),
            Value::Keyword(k) if k == "end" => Some(BracePart::End),
            Value::Keyword(k) if k == "body" => Some(BracePart::Body),
            _ => None,
        }
    }
}

/// How far an end's triangle points into the bar, in cells: also the grab
/// zone for that end.
const HANDLE_WIDTH: f32 = 0.9;

pub(super) fn parse(props: &HashMap<String, Value>) -> Option<LoopBrace> {
    if !props.get("loop-brace").and_then(as_bool).unwrap_or(false) {
        return None;
    }
    let start = get_num(props, "loop-start", f64::NAN);
    let end = get_num(props, "loop-end", f64::NAN);
    (start.is_finite() && end > start).then(|| LoopBrace {
        start,
        end,
        on: get_num(props, "loop-on", 0.0) > 0.5,
    })
}

impl TimelineView {
    /// The brace's x extent on screen, clipped to the content, or `None`
    /// while it is scrolled out of view.
    pub(super) fn loop_brace_span(&self, brace: LoopBrace) -> Option<(f32, f32)> {
        let view_end = self.view_start + self.view_duration;
        if brace.end <= self.view_start || brace.start >= view_end {
            return None;
        }
        let content = self.content_rect();
        let x0 = self.x_for_time(brace.start.max(self.view_start)).max(content.col);
        let x1 = self.x_for_time(brace.end.min(view_end)).min(content.col + content.width);
        (x1 > x0).then_some((x0, x1 - x0))
    }

    /// The brace part under a header point, if any.
    pub(super) fn loop_brace_hit(&self, local_col: f32, local_row: f32) -> Option<BracePart> {
        let brace = self.loop_brace?;
        let (top, bottom) = self.loop_band_rows()?;
        if local_row < top || local_row >= bottom {
            return None;
        }
        let (x0, x1) = (self.x_for_time(brace.start), self.x_for_time(brace.end));
        let handle = HANDLE_WIDTH.min((x1 - x0) / 2.0);
        let slop = 0.3;
        if local_col < x0 - slop || local_col > x1 + slop {
            return None;
        }
        Some(if local_col <= x0 + handle {
            BracePart::Start
        } else if local_col >= x1 - handle {
            BracePart::End
        } else {
            BracePart::Body
        })
    }

    /// The gesture a press on brace part `part` at `raw_time` starts.
    pub(super) fn loop_brace_gesture(&self, part: BracePart, raw_time: f64) -> Option<Value> {
        let brace = self.loop_brace?;
        Some(map_value(vec![
            ("kind", keyword(":loop-brace")),
            ("part", part.keyword()),
            ("anchor-time", Value::Number(raw_time)),
            ("start", Value::Number(brace.start)),
            ("end", Value::Number(brace.end)),
        ]))
    }

    /// The brace a `:loop-brace` gesture puts at `raw_time`: the body moves
    /// by the pointer's travel snapped to the visible grid (so an off-grid
    /// brace keeps its offset, and a press that does not move leaves it be);
    /// an end lands on the grid line nearest the pointer, once the pointer
    /// moves. Never before beat 0, never shorter than one grid step.
    pub(super) fn loop_brace_drag(
        &self,
        gesture: &HashMap<String, Value>,
        raw_time: f64,
    ) -> Option<(f64, f64)> {
        let part = BracePart::parse(gesture.get("part")?)?;
        let anchor = as_number(gesture.get("anchor-time")?)?;
        let (start, end) = (as_number(gesture.get("start")?)?, as_number(gesture.get("end")?)?);
        let delta = raw_time - anchor;
        if delta == 0.0 {
            return Some((start, end));
        }
        let grid = self.alignment_helper_grid_step();
        Some(match part {
            BracePart::Body => {
                let next = (start + self.cursor_snap_time(delta)).max(0.0);
                (next, next + (end - start))
            }
            BracePart::Start => (
                self.cursor_snap_time(start + delta).clamp(0.0, (end - grid).max(0.0)),
                end,
            ),
            BracePart::End => (start, self.cursor_snap_time(end + delta).max(start + grid)),
        })
    }

    /// The live and terminal actions of a `:loop-brace` drag.
    pub(super) fn loop_brace_action(
        &self,
        gesture: &HashMap<String, Value>,
        raw_time: f64,
        finish: bool,
    ) -> Option<Value> {
        let (start, end) = self.loop_brace_drag(gesture, raw_time)?;
        Some(action_map(vec![
            (
                "type",
                keyword(if finish { ":finish-loop-brace" } else { ":loop-brace" }),
            ),
            ("start", Value::Number(start)),
            ("end", Value::Number(end)),
        ]))
    }
}

/// Paint the brace into the band row `y..y + height`.
pub(super) fn push_loop_brace(
    primitives: &mut Vec<GpuPrimitive>,
    view: &TimelineView,
    brace: LoopBrace,
    (y, height): (f32, f32),
    viewport: super::super::WidgetViewport,
) {
    let Some((x, width)) = view.loop_brace_span(brace) else {
        return;
    };
    let fg = theme::TIMELINE_RULER_FG();
    // Ableton: off is an outline over the ruler; on is a light bar with a
    // dark outline. The triangles are outlined the same way, filled with
    // whatever the bar holds.
    let (fill, line) = if brace.on {
        (
            Some(crate::backend::Color { a: 0.85, ..fg }),
            crate::backend::Color { r: 0.05, g: 0.055, b: 0.06, a: 1.0 },
        )
    } else {
        (None, fg)
    };
    let quad = |primitives: &mut Vec<GpuPrimitive>, x: f32, y: f32, w: f32, h: f32, color| {
        primitives.push(GpuPrimitive::Quad(GpuQuadPrimitive { x, y, width: w, height: h, color }));
    };
    if let Some(fill) = fill {
        quad(primitives, x, y, width, height, fill);
    }
    let px_h = (1.0 / viewport.cell_h.max(1.0)).min(height);
    let px_w = (1.0 / viewport.cell_w.max(1.0)).min(width);
    quad(primitives, x, y, width, px_h, line);
    quad(primitives, x, y + height - px_h, width, px_h, line);
    let (x0, x1) = (view.x_for_time(brace.start), view.x_for_time(brace.end));
    let content = view.content_rect();
    let visible = |edge: f32| edge >= content.col - 0.01 && edge <= content.col + content.width + 0.01;
    for edge in [x0, x1] {
        if visible(edge) {
            quad(primitives, edge - px_w / 2.0, y, px_w, height, line);
        }
    }
    // Ableton's triangles fill the bar's height and point in by about two
    // thirds of it (converted from rows to columns via the cell aspect).
    let aspect = viewport.cell_h.max(1.0) / viewport.cell_w.max(1.0);
    let handle = (height * 0.65 * aspect).min(HANDLE_WIDTH * 1.4).min((x1 - x0) / 2.0);
    let inset = height * 0.1;
    let (top, bottom) = (y + inset, y + height - inset);
    let mid = (top + bottom) / 2.0;
    let inner_fill = fill.unwrap_or_else(theme::TIMELINE_RULER_BG);
    for (edge, dir) in [(x0, 1.0_f32), (x1, -1.0)] {
        if !visible(edge) {
            continue;
        }
        let tip = edge + dir * handle;
        primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
            points: [[edge, top], [edge, bottom], [tip, mid]],
            color: line,
        }));
        // The outline: the same triangle inset by about 1.5 px, in the fill.
        let (dx, dy) = (px_w * 1.5, px_h * 1.5);
        primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
            points: [
                [edge + dir * dx, top + dy * 2.2],
                [edge + dir * dx, bottom - dy * 2.2],
                [tip - dir * dx * 2.2, mid],
            ],
            color: inner_fill,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(extra: Vec<(&str, Value)>) -> TimelineView {
        let mut props = HashMap::from([
            ("view-start".to_string(), Value::Number(0.0)),
            ("view-duration".to_string(), Value::Number(64.0)),
            ("header-height".to_string(), Value::Number(2.0)),
            ("loop-brace".to_string(), Value::Bool(true)),
            ("loop-start".to_string(), Value::Number(8.0)),
            ("loop-end".to_string(), Value::Number(24.0)),
            (
                "time-ruler".to_string(),
                map_value(vec![("mode", keyword(":bars-beats")), ("beats-per-bar", Value::Number(4.0))]),
            ),
        ]);
        for (k, v) in extra {
            props.insert(k.to_string(), v);
        }
        let rect = Rect { row: 0.0, col: 0.0, width: 64.0, height: 8.0 };
        TimelineView::from_props(&props, rect)
    }

    #[test]
    fn the_brace_parses_off_by_default_and_needs_a_span() {
        assert_eq!(view(vec![]).loop_brace, Some(LoopBrace { start: 8.0, end: 24.0, on: false }));
        assert!(view(vec![("loop-on", Value::Number(1.0))]).loop_brace.unwrap().on);
        assert_eq!(view(vec![("loop-end", Value::Number(8.0))]).loop_brace, None);
    }

    #[test]
    fn a_press_grabs_an_end_or_the_body_inside_the_band_row() {
        let v = view(vec![]);
        let (top, bottom) = v.loop_band_rows().expect("band row");
        let row = (top + bottom) / 2.0;
        let (x0, x1) = (v.x_for_time(8.0), v.x_for_time(24.0));
        assert_eq!(v.loop_brace_hit(x0 + 0.2, row), Some(BracePart::Start));
        assert_eq!(v.loop_brace_hit(x1 - 0.2, row), Some(BracePart::End));
        assert_eq!(v.loop_brace_hit((x0 + x1) / 2.0, row), Some(BracePart::Body));
        assert_eq!(v.loop_brace_hit(x1 + 2.0, row), None, "past the brace");
        assert_eq!(v.loop_brace_hit((x0 + x1) / 2.0, top - 0.3), None, "the ruler row above scrubs");
    }

    #[test]
    fn drags_snap_to_the_grid_and_never_invert_or_go_negative() {
        let v = view(vec![]);
        let grid = v.alignment_helper_grid_step();
        let gesture = |part: BracePart| match v.loop_brace_gesture(part, 10.0) {
            Some(Value::Map(map)) => map.iter().map(|(k, v)| (k.clone(), v.borrow().clone())).collect(),
            other => panic!("{other:?}"),
        };
        let body = gesture(BracePart::Body);
        assert_eq!(v.loop_brace_drag(&body, 10.0 + 4.0 * grid + grid * 0.3), Some((8.0 + 4.0 * grid, 24.0 + 4.0 * grid)));
        assert_eq!(v.loop_brace_drag(&body, -100.0), Some((0.0, 16.0)), "the body stops at beat 0");
        let end = gesture(BracePart::End);
        assert_eq!(v.loop_brace_drag(&end, -100.0), Some((8.0, 8.0 + grid)), "an end never passes the start");
        assert_eq!(v.loop_brace_drag(&body, 10.0), Some((8.0, 24.0)), "a press that does not move");
        let start = gesture(BracePart::Start);
        assert_eq!(v.loop_brace_drag(&start, 100.0), Some((24.0 - grid, 24.0)));
    }
}
