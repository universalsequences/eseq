//! `scale-editor`: a track's tuned scale, one column per degree
//! (docs/microtonal-scales-spec.md §5.2).
//!
//! - Top strip: the staircase. Each degree's sounding pitch (`:pitches`,
//!   cents above the root) is a step on a `0..:period` axis drawn over faint
//!   12-TET lines, so the shape of the scale reads at a glance.
//! - Middle: detune bars. The centre line is the degree's base pitch
//!   (`:base`); a bar runs to its `:offsets` entry on a `±:range` cent axis
//!   (sharp up in `:sharp-color`, flat down in `:flat-color`), a dot marks the
//!   detune that actually sounds after morph, and a faint tick marks where
//!   the nearest 12-TET semitone sits.
//! - Bottom: one dot per degree (`:enabled`) and an optional `:labels` row.
//!
//! The widget is stateless; it reports `(kind degree value)` to `on-change`:
//! `:set` for a press/drag in the bars (value = cents, whole cents, 5 with
//! shift), `:finish` on release, `:clear` on double-click or alt-press,
//! `:toggle` for a press on a degree's dot and `:select` for a press in the
//! staircase.
//!
//! Plain primitives only (rects, circles, text), so no shader pair to keep.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuCirclePrimitive, GpuCircleVisibleHalf, GpuPrimitive,
    GpuProportionalTextPrimitive, GpuRectPrimitive, MouseEventOutcome, WidgetDefinition,
    WidgetEvent, WidgetViewport, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct ScaleEditorWidget;

pub static SCALE_EDITOR_WIDGET: ScaleEditorWidget = ScaleEditorWidget;

const DEFAULT_HEIGHT: f32 = 8.0;
const DEFAULT_RANGE_CENTS: f32 = 100.0;
/// Share of the height the staircase takes.
const STAIR_SHARE: f32 = 0.34;
/// Rows reserved under the bars for the dot row and the label row.
const DOT_ROW: f32 = 0.8;
const LABEL_ROW: f32 = 0.9;
/// Narrowest column (cells) that still gets a text label.
const MIN_LABEL_COL: f32 = 1.5;

fn numbers(props: &HashMap<String, Value>, key: &str) -> Vec<f32> {
    let Some(Value::List(items)) = props.get(key) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::Number(value) if value.is_finite() => *value as f32,
            Value::Bool(true) => 1.0,
            _ => 0.0,
        })
        .collect()
}

fn strings(props: &HashMap<String, Value>, key: &str) -> Vec<String> {
    let Some(Value::List(items)) = props.get(key) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::String(text) => text.clone(),
            _ => String::new(),
        })
        .collect()
}

/// The props the widget draws from, normalized to one entry per degree.
#[derive(Debug, PartialEq)]
struct Degrees {
    base: Vec<f32>,
    offsets: Vec<f32>,
    pitches: Vec<f32>,
    enabled: Vec<bool>,
    period: f32,
    range: f32,
}

impl Degrees {
    fn count(&self) -> usize {
        self.base.len()
    }
}

fn degrees(props: &HashMap<String, Value>) -> Degrees {
    let base = numbers(props, "base");
    let count = base.len();
    let padded = |mut values: Vec<f32>, fill: &dyn Fn(usize) -> f32| {
        values.truncate(count);
        while values.len() < count {
            values.push(fill(values.len()));
        }
        values
    };
    let offsets = padded(numbers(props, "offsets"), &|_| 0.0);
    let pitches = padded(numbers(props, "pitches"), &|idx| base[idx] + offsets[idx]);
    let enabled = match props.get("enabled") {
        Some(Value::List(_)) => padded(numbers(props, "enabled"), &|_| 1.0),
        _ => vec![1.0; count],
    }
    .into_iter()
    .map(|flag| flag != 0.0)
    .collect();
    let period = get_f32_prop(props, "period", 1200.0);
    let range = get_f32_prop(props, "range", DEFAULT_RANGE_CENTS);
    Degrees {
        base,
        offsets,
        pitches,
        enabled,
        period: if period > 0.0 { period } else { 1200.0 },
        range: if range > 0.0 { range } else { DEFAULT_RANGE_CENTS },
    }
}

/// The widget's three bands, in the same row/col space as `node.rect`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bands {
    stair: Rect,
    bars: Rect,
    dots: Rect,
    labels: Option<Rect>,
}

fn bands(rect: Rect, show_labels: bool) -> Bands {
    let label_h = if show_labels { LABEL_ROW.min(rect.height * 0.15) } else { 0.0 };
    let dot_h = DOT_ROW.min(rect.height * 0.12);
    let stair_h = rect.height * STAIR_SHARE;
    let gap = (rect.height * 0.04).min(0.3);
    let bars_h = (rect.height - stair_h - gap - dot_h - label_h).max(0.5);
    let stair = Rect { height: stair_h, ..rect };
    let bars = Rect {
        row: rect.row + stair_h + gap,
        height: bars_h,
        ..rect
    };
    let dots = Rect {
        row: bars.row + bars_h,
        height: dot_h,
        ..rect
    };
    let labels = show_labels.then_some(Rect {
        row: dots.row + dot_h,
        height: label_h,
        ..rect
    });
    Bands {
        stair,
        bars,
        dots,
        labels,
    }
}

fn column_at(rect: Rect, count: usize, col: f32) -> Option<usize> {
    if count == 0 || rect.width <= 0.0 {
        return None;
    }
    let t = (col - rect.col) / rect.width;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some(((t * count as f32) as usize).min(count - 1))
}

/// Row of `cents` on the bars' `±range` axis (sharp is up).
fn bar_row(bars: Rect, range: f32, cents: f32) -> f32 {
    let mid = bars.row + bars.height * 0.5;
    mid - (cents / range).clamp(-1.0, 1.0) * bars.height * 0.5
}

fn cents_for_row(bars: Rect, range: f32, row: f32, modifiers: KeyModifiers) -> f32 {
    let mid = bars.row + bars.height * 0.5;
    let half = (bars.height * 0.5).max(1e-3);
    let cents = ((mid - row) / half).clamp(-1.0, 1.0) * range;
    let step = if modifiers.contains(KeyModifiers::SHIFT) { 5.0 } else { 1.0 };
    (cents / step).round() * step
}

fn editor_event(kind: &str, degree: usize, value: f32) -> WidgetEvent {
    WidgetEvent::Custom(Value::List(vec![
        Rc::new(RefCell::new(Value::Keyword(kind.to_string()))),
        Rc::new(RefCell::new(Value::Number(degree as f64))),
        Rc::new(RefCell::new(Value::Number(f64::from(value)))),
    ]))
}

fn selected(props: &HashMap<String, Value>) -> Option<usize> {
    match props.get("selected") {
        Some(Value::Number(value)) if *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha,
        ..color
    }
}

fn shows_labels(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("labels"), Some(Value::List(items)) if !items.is_empty())
}

impl WidgetDefinition for ScaleEditorWidget {
    fn names(&self) -> &'static [&'static str] {
        &["scale-editor"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "base",
            "offsets",
            "pitches",
            "enabled",
            "labels",
            "period",
            "range",
            "selected",
            "width",
            "height",
            "color",
            "sharp-color",
            "flat-color",
            "grid-color",
            "background",
            "on-change",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[&Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(constraints.max_width)
                .min(constraints.max_width)
                .max(1.0),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_HEIGHT)
                .max(2.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let degrees = degrees(props);
        let count = degrees.count();
        if count == 0 {
            return;
        }
        let row = (rect.row + rect.height * 0.5).floor() as u16;
        let sharp = resolve_named_color(props, "sharp-color", theme::WIDGET_SLIDER_FILLED());
        let muted = theme::FG_MUTED();
        for degree in 0..count {
            let col = rect.col + (degree as f32 + 0.5) * rect.width / count as f32;
            let offset = degrees.offsets[degree];
            let (ch, color) = if !degrees.enabled[degree] {
                ('○', muted)
            } else if offset > 0.5 {
                ('▴', sharp)
            } else if offset < -0.5 {
                ('▾', sharp)
            } else {
                ('●', sharp)
            };
            buf.set(row, col.floor() as u16, styled_cell(ch, color, None));
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let degrees = degrees(&node.props);
        let count = degrees.count();
        let bands = bands(node.rect, shows_labels(&node.props));
        let in_bars = |row: f32| row >= bands.bars.row && row <= bands.bars.row + bands.bars.height;
        // A drag keeps editing the column (and band) the press landed in.
        let (anchor_col, anchor_row) = drag_start.unwrap_or((local_col, local_row));
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(degree) = column_at(node.rect, count, local_col) else {
                    return MouseEventOutcome::Ignore;
                };
                if local_row >= bands.dots.row && local_row <= bands.dots.row + bands.dots.height {
                    return MouseEventOutcome::Dispatch(editor_event("toggle", degree, 0.0));
                }
                if local_row < bands.bars.row {
                    return MouseEventOutcome::Dispatch(editor_event("select", degree, 0.0));
                }
                if !in_bars(local_row) {
                    return MouseEventOutcome::Consume;
                }
                if modifiers.contains(KeyModifiers::ALT) {
                    return MouseEventOutcome::Dispatch(editor_event("clear", degree, 0.0));
                }
                let cents = cents_for_row(bands.bars, degrees.range, local_row, modifiers);
                MouseEventOutcome::Dispatch(editor_event("set", degree, cents))
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(degree) = column_at(node.rect, count, anchor_col) else {
                    return MouseEventOutcome::Consume;
                };
                if !in_bars(anchor_row) || modifiers.contains(KeyModifiers::ALT) {
                    return MouseEventOutcome::Consume;
                }
                let cents = cents_for_row(bands.bars, degrees.range, local_row, modifiers);
                MouseEventOutcome::Dispatch(editor_event("set", degree, cents))
            }
            MouseEventKind::Up(MouseButton::Left) => {
                match column_at(node.rect, count, anchor_col) {
                    Some(degree) if in_bars(anchor_row) => MouseEventOutcome::Dispatch(
                        editor_event("finish", degree, degrees.offsets[degree]),
                    ),
                    _ => MouseEventOutcome::Consume,
                }
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        let degrees = degrees(&node.props);
        let bands = bands(node.rect, shows_labels(&node.props));
        if local_row < bands.bars.row || local_row > bands.bars.row + bands.bars.height {
            return None;
        }
        let degree = column_at(node.rect, degrees.count(), local_col)?;
        Some(editor_event("clear", degree, 0.0))
    }

    fn captures_drag(&self) -> bool {
        true
    }

    /// Cents clamp to the axis, so the pointer may leave the rect mid-drag
    /// and keep pinning the bar to an edge.
    fn unclamped_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let args = match &value {
            Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
            other => vec![other.clone()],
        };
        Some(EventOutput { callback, args })
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let rect = node.rect;
        if !(rect.width.is_finite() && rect.height.is_finite())
            || rect.width <= 0.0
            || rect.height <= 0.0
        {
            return Vec::new();
        }
        let props = &node.props;
        let degrees = degrees(props);
        let count = degrees.count();
        let labels = strings(props, "labels");
        let bands = bands(rect, !labels.is_empty());
        let accent = resolve_named_color(props, "color", Color::rgba(0.93, 0.58, 0.25, 1.0));
        let sharp = resolve_named_color(props, "sharp-color", Color::rgba(0.95, 0.52, 0.30, 1.0));
        let flat = resolve_named_color(props, "flat-color", Color::rgba(0.35, 0.62, 1.0, 1.0));
        let grid = resolve_named_color(props, "grid-color", Color::rgba(1.0, 1.0, 1.0, 0.08));
        let text = resolve_named_color(props, "text-color", Color::rgba(0.70, 0.72, 0.76, 1.0));
        let background = resolve_named_color(props, "background", Color::rgba(0.0, 0.0, 0.0, 0.22));
        let cell_h = viewport.cell_h.max(1.0);
        let hairline = (super::ui_design_px(1.0) / cell_h).min(rect.height * 0.05);
        let step_h = (super::ui_design_px(2.5) / cell_h).min(rect.height * 0.05);

        let mut prims = vec![GpuPrimitive::Rect(GpuRectPrimitive { rect, color: background })];
        if count == 0 {
            return prims;
        }
        let col_w = rect.width / count as f32;
        let column = |degree: usize| rect.col + degree as f32 * col_w;
        let inset = (col_w * 0.18).min(0.35);

        if let Some(degree) = selected(props).filter(|degree| *degree < count) {
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    col: column(degree),
                    width: col_w,
                    ..rect
                },
                color: with_alpha(accent, 0.10),
            }));
        }

        // Staircase: 12-TET lines behind one step per degree.
        let stair = bands.stair;
        let stair_row = |cents: f32| {
            let t = (cents / degrees.period).clamp(0.0, 1.0);
            stair.row + stair.height * (1.0 - t)
        };
        if degrees.period <= 2400.0 {
            let mut cents = 100.0;
            while cents < degrees.period {
                prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: Rect {
                        row: stair_row(cents) - hairline * 0.5,
                        height: hairline,
                        ..stair
                    },
                    color: grid,
                }));
                cents += 100.0;
            }
        }
        for degree in 0..count {
            let on = degrees.enabled[degree];
            let row = stair_row(degrees.pitches[degree]);
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: row - step_h * 0.5,
                    col: column(degree) + inset * 0.5,
                    width: (col_w - inset).max(0.05),
                    height: step_h,
                },
                color: if on { accent } else { with_alpha(accent, 0.25) },
            }));
        }

        // Detune bars.
        let bars = degrees_bars(&degrees, bands.bars, &column, col_w, inset, hairline);
        for (rect, kind) in bars {
            let color = match kind {
                BarPart::Centre => with_alpha(grid, 2.5),
                BarPart::EqualTick => with_alpha(text, 0.35),
                BarPart::Sharp(on) => if on { sharp } else { with_alpha(sharp, 0.3) },
                BarPart::Flat(on) => if on { flat } else { with_alpha(flat, 0.3) },
            };
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive { rect, color }));
        }
        let dot_px = super::ui_design_px(get_f32_prop(props, "dot-size", 3.0)).max(1.0);
        for degree in 0..count {
            // Where the degree sounds after morph, relative to its base.
            let sounding = degrees.pitches[degree] - degrees.base[degree];
            let color = if degrees.enabled[degree] { text } else { with_alpha(text, 0.3) };
            prims.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                center: [
                    column(degree) + col_w * 0.5,
                    bar_row(bands.bars, degrees.range, sounding),
                ],
                radius_px: dot_px,
                color,
                visible_half: GpuCircleVisibleHalf::Full,
            }));
        }

        // Enable dots.
        let dot_center_row = bands.dots.row + bands.dots.height * 0.5;
        let enable_px = super::ui_design_px(4.0).max(1.0);
        for degree in 0..count {
            let center = [column(degree) + col_w * 0.5, dot_center_row];
            if degrees.enabled[degree] {
                prims.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                    center,
                    radius_px: enable_px,
                    color: accent,
                    visible_half: GpuCircleVisibleHalf::Full,
                }));
            } else {
                prims.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                    center,
                    radius_px: enable_px,
                    color: with_alpha(text, 0.35),
                    visible_half: GpuCircleVisibleHalf::Full,
                }));
                prims.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                    center,
                    radius_px: (enable_px - super::ui_design_px(1.5)).max(0.5),
                    color: background,
                    visible_half: GpuCircleVisibleHalf::Full,
                }));
            }
        }

        if let Some(label_rect) = bands.labels.filter(|_| col_w >= MIN_LABEL_COL) {
            let font_size = get_f32_prop(props, "font-size", 7.0);
            for (degree, label) in labels.into_iter().take(count).enumerate() {
                if label.is_empty() {
                    continue;
                }
                prims.push(GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
                    row: label_rect.row + (label_rect.height - 1.0).max(0.0) * 0.5,
                    col: column(degree),
                    align_width: col_w,
                    h_align: 0.5,
                    text: label,
                    font_size,
                    scale: 1.0,
                    fg: if degrees.enabled[degree] { text } else { with_alpha(text, 0.4) },
                    bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
                    mono: false,
                }));
            }
        }
        prims
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BarPart {
    Centre,
    /// Where the nearest 12-TET semitone sits relative to the base pitch.
    EqualTick,
    Sharp(bool),
    Flat(bool),
}

/// Rects of the detune band: the centre line, each column's 12-TET tick and
/// its offset bar.
fn degrees_bars(
    degrees: &Degrees,
    bars: Rect,
    column: &dyn Fn(usize) -> f32,
    col_w: f32,
    inset: f32,
    hairline: f32,
) -> Vec<(Rect, BarPart)> {
    let mid = bar_row(bars, degrees.range, 0.0);
    let mut parts = vec![(
        Rect {
            row: mid - hairline * 0.5,
            height: hairline,
            ..bars
        },
        BarPart::Centre,
    )];
    let bar_w = (col_w * 0.42).clamp(0.08, 1.2);
    for degree in 0..degrees.count() {
        let center = column(degree) + col_w * 0.5;
        let base = degrees.base[degree];
        let equal = (base / 100.0).round() * 100.0 - base;
        if equal.abs() > 0.5 && equal.abs() <= degrees.range {
            let row = bar_row(bars, degrees.range, equal);
            parts.push((
                Rect {
                    row: row - hairline,
                    col: column(degree) + inset,
                    width: (col_w - inset * 2.0).max(0.05),
                    height: hairline * 2.0,
                },
                BarPart::EqualTick,
            ));
        }
        let offset = degrees.offsets[degree];
        if offset.abs() < 0.5 {
            continue;
        }
        let end = bar_row(bars, degrees.range, offset);
        let on = degrees.enabled[degree];
        parts.push((
            Rect {
                row: end.min(mid),
                col: center - bar_w * 0.5,
                width: bar_w,
                height: (end - mid).abs().max(hairline),
            },
            if offset > 0.0 { BarPart::Sharp(on) } else { BarPart::Flat(on) },
        ));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(values: &[f64]) -> Value {
        Value::List(
            values
                .iter()
                .map(|value| Rc::new(RefCell::new(Value::Number(*value))))
                .collect(),
        )
    }

    fn node(props: Vec<(&str, Value)>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "scale-editor".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 12.0,
                height: 10.0,
            },
            props: props.into_iter().map(|(key, value)| (key.to_string(), value)).collect(),
            children: Vec::new(),
            focusable: false,
            animation: crate::layout::LayoutAnimationHints::default(),
        }
    }

    fn major() -> LayoutNode {
        node(vec![
            ("base", list(&[0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1100.0])),
            ("offsets", list(&[0.0, 0.0, -14.0])),
            ("on-change", Value::Keyword("cb".to_string())),
        ])
    }

    fn dispatched(outcome: MouseEventOutcome) -> (String, usize, f64) {
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::List(items))) = outcome else {
            panic!("expected a dispatch");
        };
        let items: Vec<Value> = items.iter().map(|item| item.borrow().clone()).collect();
        match items.as_slice() {
            [Value::Keyword(kind), Value::Number(degree), Value::Number(value)] => {
                (kind.clone(), *degree as usize, *value)
            }
            other => panic!("unexpected payload {other:?}"),
        }
    }

    #[test]
    fn missing_lists_pad_to_the_degree_count() {
        let degrees = degrees(&major().props);
        assert_eq!(degrees.count(), 7);
        assert_eq!(degrees.offsets[2], -14.0);
        assert_eq!(degrees.offsets[6], 0.0);
        assert_eq!(degrees.pitches[2], 386.0);
        assert!(degrees.enabled.iter().all(|on| *on));
    }

    #[test]
    fn press_and_drag_in_the_bars_set_cents_on_the_pressed_column() {
        let node = major();
        let bands = bands(node.rect, false);
        let col = 12.0 / 7.0 * 2.5;
        let top = bands.bars.row + 0.01;
        let (kind, degree, cents) = dispatched(ScaleEditorWidget.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            col,
            top,
            None,
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        ));
        assert_eq!((kind.as_str(), degree), ("set", 2));
        assert!(cents > 95.0, "{cents}");
        // Wandering sideways mid-drag keeps editing degree 2; leaving the
        // bottom of the rect pins the bar at −range.
        let (kind, degree, cents) = dispatched(ScaleEditorWidget.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            11.0,
            50.0,
            Some((col, top)),
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        ));
        assert_eq!((kind.as_str(), degree, cents), ("set", 2, -100.0));
        let (kind, degree, cents) = dispatched(ScaleEditorWidget.mouse_event(
            &node,
            MouseEventKind::Up(MouseButton::Left),
            11.0,
            50.0,
            Some((col, top)),
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        ));
        assert_eq!((kind.as_str(), degree, cents), ("finish", 2, -14.0));
    }

    #[test]
    fn dots_toggle_staircase_selects_and_alt_or_double_click_clears() {
        let node = major();
        let bands = bands(node.rect, false);
        let press = |row: f32, modifiers| {
            dispatched(ScaleEditorWidget.mouse_event(
                &node,
                MouseEventKind::Down(MouseButton::Left),
                0.5,
                row,
                None,
                None,
                modifiers,
                8.0,
                16.0,
            ))
        };
        assert_eq!(press(bands.dots.row + 0.2, KeyModifiers::NONE).0, "toggle");
        assert_eq!(press(bands.stair.row + 0.5, KeyModifiers::NONE).0, "select");
        let mid = bands.bars.row + bands.bars.height * 0.5;
        assert_eq!(press(mid, KeyModifiers::ALT).0, "clear");
        let shifted = press(bands.bars.row + bands.bars.height * 0.36, KeyModifiers::SHIFT);
        assert_eq!(shifted.2 % 5.0, 0.0);
        let Some(WidgetEvent::Custom(_)) = ScaleEditorWidget.double_click_event(&node, 0.5, mid)
        else {
            panic!("double-click in the bars clears");
        };
        assert!(ScaleEditorWidget.double_click_event(&node, 0.5, bands.stair.row).is_none());
    }

    #[test]
    fn bars_mark_detunes_and_the_twelve_tet_tick() {
        let node = node(vec![
            ("base", list(&[0.0, 386.3137])),
            ("offsets", list(&[0.0, 20.0])),
        ]);
        let degrees = degrees(&node.props);
        let bands = bands(node.rect, false);
        let parts = degrees_bars(&degrees, bands.bars, &|degree| degree as f32 * 6.0, 6.0, 0.5, 0.05);
        assert!(parts.iter().any(|(_, part)| *part == BarPart::EqualTick));
        let (bar, _) = parts
            .iter()
            .find(|(_, part)| matches!(part, BarPart::Sharp(true)))
            .expect("sharp bar");
        let mid = bar_row(bands.bars, 100.0, 0.0);
        assert!((bar.row + bar.height - mid).abs() < 1e-4);
        assert!(bar.row < mid);
    }
}
