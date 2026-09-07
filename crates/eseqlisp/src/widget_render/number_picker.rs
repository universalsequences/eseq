use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_bool_prop, get_f32_prop, plock_active, plock_color, resolve_named_color,
    should_trigger_integer_haptic, styled_cell, trigger_level_change_haptic,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

use super::knob_number::{knob_taper, taper_denormalize, taper_normalize};

use super::{
    FocusCornerStyle, FocusDecoration, GpuPrimitive, GpuProportionalTextPrimitive,
    GpuRectPrimitive, WidgetInstance, WidgetViewport, ndc_bounds,
};
use crate::backend::Color;

// ── Constants ────────────────────────────────────────────────────────────────

const TEXT_PADDING_H: f32 = 0.5;
const RING_WIDTH: f32 = 0.15;
const TRIANGLE_WIDTH: f32 = 1.4;
/// Full-travel drag distance (in layout rows) used when a hidden-cursor drag
/// is active and the widget does not pin its own `:drag-rows`.
const HIDDEN_DRAG_ROWS: f32 = 8.0;

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct NumberPickerEditState {
    /// When true, the user is typing a value. The display shows edit_text
    /// instead of the formatted prop value.
    pub editing: bool,
    pub edit_text: String,
    pub cursor_pos: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_display_aliases_match_model_values_before_scaling() {
        let mut runtime = crate::Runtime::new();
        let labels = runtime.eval_str("'((-1 \"infinite\"))").unwrap().unwrap();
        let props = HashMap::from([
            ("value-labels".to_string(), labels),
            ("value-scale".to_string(), Value::Number(1000.0)),
        ]);
        assert_eq!(formatted_model_value(&props, -1.0, 2), "infinite");
        assert_eq!(formatted_model_value(&props, 0.5, 2), "500.00");
        assert_eq!(formatted_model_value(&props, -0.999, 2), "-999.00");
    }

    fn color_value(r: f64, g: f64, b: f64, a: f64) -> Value {
        Value::List(
            [r, g, b, a]
                .into_iter()
                .map(|value| std::rc::Rc::new(RefCell::new(Value::Number(value))))
                .collect(),
        )
    }

    fn test_viewport() -> WidgetViewport {
        WidgetViewport {
            vp_w: 100.0,
            vp_h: 100.0,
            cell_w: 10.0,
            cell_h: 10.0,
            scroll_top: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 10.0,
            inherited_hover: false,
            time_seconds: 0.0,
            scroll_left: 0.0,
        }
    }

    fn test_number_picker_node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "number-picker".to_string(),
            rect: Rect {
                row: 2.0,
                col: 4.0,
                width: 8.0,
                height: 1.4,
            },
            props,
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        }
    }

    #[test]
    fn value_is_bindable_but_not_size_affecting() {
        assert_eq!(
            NUMBER_PICKER_WIDGET.bindable_props(),
            &[
                "value",
                "active",
                "border-width",
                "corner-radius",
                "plock-active",
                "plock-color-r",
                "plock-color-g",
                "plock-color-b",
            ]
        );
        assert!(
            !NUMBER_PICKER_WIDGET
                .size_affecting_props()
                .contains(&"value")
        );
        assert!(
            !NUMBER_PICKER_WIDGET
                .size_affecting_props()
                .contains(&"active")
        );
    }

    #[test]
    fn zero_decimal_values_snap_to_integers() {
        let props = HashMap::from([("decimals".to_string(), Value::Number(0.0))]);
        let step = quantize_step(&props, 0);

        assert_eq!(quantize_value(2.49, -12.0, 12.0, step), 2.0);
        assert_eq!(quantize_value(2.50, -12.0, 12.0, step), 3.0);
    }

    #[test]
    fn zero_decimal_scaled_values_snap_in_display_units() {
        let props = HashMap::from([
            ("decimals".to_string(), Value::Number(0.0)),
            ("value-scale".to_string(), Value::Number(100.0)),
        ]);
        let step = quantize_step(&props, 0);

        assert_eq!(step, Some(0.01));
        assert!((quantize_value(0.124, -0.5, 0.5, step) - 0.12).abs() < f32::EPSILON);
        assert!((quantize_value(0.125, -0.5, 0.5, step) - 0.13).abs() < f32::EPSILON);
    }

    fn drag_to(
        props: HashMap<String, Value>,
        start_value: f64,
        start_row: f32,
        to_row: f32,
    ) -> f32 {
        let node = test_number_picker_node(props);
        let gesture = Value::List(vec![
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(start_value))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(start_row as f64))),
        ]);
        let outcome = NUMBER_PICKER_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            0.0,
            to_row,
            None,
            Some(&gesture),
            KeyModifiers::empty(),
            10.0,
            10.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(value))) = outcome else {
            panic!("drag should dispatch a value");
        };
        value as f32
    }

    /// Linear pickers keep their historical feel: no taper, no drag-rows,
    /// five rows of travel from a short strip spans the whole range.
    #[test]
    fn linear_drag_from_a_short_strip_spans_the_range_in_five_rows() {
        let mut props = HashMap::new();
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(100.0));
        props.insert("decimals".to_string(), Value::Number(0.0));
        assert_eq!(drag_to(props.clone(), 0.0, 1.0, -4.0), 100.0);
        assert_eq!(drag_to(props, 0.0, 1.0, -1.5), 50.0);
    }

    /// A log picker walks normalized position, and `:drag-rows` pins the
    /// full-travel distance regardless of where in the strip the drag began.
    #[test]
    fn log_taper_with_drag_rows_moves_geometrically_over_the_pinned_distance() {
        let mut props = HashMap::new();
        props.insert("min".to_string(), Value::Number(1.0));
        props.insert("max".to_string(), Value::Number(1024.0));
        props.insert("decimals".to_string(), Value::Number(0.0));
        props.insert("taper".to_string(), Value::String("log".to_string()));
        props.insert("drag-rows".to_string(), Value::Number(20.0));
        // From 1 (t = 0): half the pinned travel is the geometric midpoint.
        assert_eq!(drag_to(props.clone(), 1.0, 1.0, -9.0), 32.0);
        // Five rows, which used to be the whole range, is now 1 -> 32^(1/2)·..:
        // t = 0.25 -> 1024^0.25 ≈ 5.66 -> snapped to 6.
        assert_eq!(drag_to(props.clone(), 1.0, 1.0, -4.0), 6.0);
        // The top of the pinned travel still reaches max.
        assert_eq!(drag_to(props.clone(), 1.0, 1.0, -19.0), 1024.0);
        // Dragging down from 32 (t = 0.5): half the travel halves the
        // position (t = 0.25 -> 6), the full travel lands on min.
        assert_eq!(drag_to(props.clone(), 32.0, 1.0, 11.0), 6.0);
        assert_eq!(drag_to(props, 32.0, 1.0, 21.0), 1.0);
    }

    #[test]
    fn square_taper_reaches_one_without_sacrificing_full_range() {
        let mut props = HashMap::new();
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(127.0));
        props.insert("decimals".to_string(), Value::Number(0.0));
        props.insert("taper".to_string(), Value::String("square".to_string()));
        props.insert("drag-rows".to_string(), Value::Number(24.0));

        // Two rows reaches the first repeat (the old cube curve needed five).
        assert_eq!(drag_to(props.clone(), 0.0, 1.0, -1.0), 1.0);
        assert_eq!(drag_to(props, 0.0, 1.0, -23.0), 127.0);
    }

    #[test]
    fn explicit_step_snaps_decimal_values() {
        let props = HashMap::from([
            ("decimals".to_string(), Value::Number(2.0)),
            ("step".to_string(), Value::Number(0.25)),
        ]);
        let step = quantize_step(&props, 2);

        assert_eq!(quantize_value(0.62, 0.0, 1.0, step), 0.5);
        assert_eq!(quantize_value(0.63, 0.0, 1.0, step), 0.75);
    }

    #[test]
    fn number_picker_metal_background_uses_button_surface_detail_colors() {
        let node = test_number_picker_node(HashMap::from([
            ("value".to_string(), Value::Number(0.5)),
            (
                "background-color".to_string(),
                color_value(0.1, 0.2, 0.3, 1.0),
            ),
            ("border-color".to_string(), color_value(0.4, 0.5, 0.6, 0.7)),
            (
                "highlight-color".to_string(),
                color_value(0.8, 0.85, 0.9, 0.2),
            ),
            ("shadow-color".to_string(), color_value(0.0, 0.0, 0.0, 0.3)),
        ]));

        let prims = NumberPickerWidget.build_primitives("number-picker", &node, test_viewport());
        let instance = prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background: true,
                } if widget_type == "number-picker" => Some(instance),
                _ => None,
            })
            .expect("number picker should emit a detailed background primitive");

        assert_eq!(instance.color_a, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(instance.color_b, [0.4, 0.5, 0.6, 0.7]);
        assert_eq!(instance.color_c, [0.8, 0.85, 0.9, 0.2]);
        assert_eq!(instance.color_d, [0.0, 0.0, 0.0, 0.3]);
        assert!(instance.corner_radius > 0.0);
    }

    #[test]
    fn slider_mode_renders_normalized_fill_without_triangle() {
        let node = test_number_picker_node(HashMap::from([
            ("mode".to_string(), Value::Keyword("slider".to_string())),
            ("value".to_string(), Value::Number(25.0)),
            ("min".to_string(), Value::Number(0.0)),
            ("max".to_string(), Value::Number(100.0)),
            ("corner-radius".to_string(), Value::Number(0.0)),
            ("border-width".to_string(), Value::Number(2.5)),
            ("fill-color".to_string(), color_value(0.1, 0.6, 0.7, 0.8)),
        ]));

        let focus_color = [1.0, 0.7, 0.2, 1.0];
        let focused_viewport = WidgetViewport {
            focused_widget_id: Some(node.widget_id),
            ..test_viewport()
        };
        let mut node = node;
        node.props
            .insert("focus-color".to_string(), color_value(1.0, 0.7, 0.2, 1.0));
        let primitives =
            NumberPickerWidget.build_primitives("number-picker", &node, focused_viewport);
        let fill = primitives
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } if widget_type == "number-picker-slider" => Some(instance),
                _ => None,
            })
            .expect("slider mode should emit a fill primitive");

        assert!((fill.value_t - 0.25).abs() < f32::EPSILON);
        assert_eq!(fill.color_b, focus_color);
        assert_eq!(fill.color_c, [0.1, 0.6, 0.7, 0.8]);
        assert_eq!(fill.uniform_a[0], 2.5);
        assert_eq!(fill.uniform_a[1], 0.0);
        assert_eq!(fill.corner_radius, 0.001);
        assert_eq!(
            primitives
                .iter()
                .filter(|primitive| matches!(
                    primitive,
                    GpuPrimitive::WidgetInstance { widget_type, .. }
                        if widget_type == "number-picker"
                ))
                .count(),
            0,
            "focused slider mode should not render the button surface or its outer focus ring"
        );
        assert!(primitives.iter().all(|primitive| !matches!(
            primitive,
            GpuPrimitive::WidgetInstance { widget_type, .. }
                if widget_type == "number-picker-tri"
        )));
    }

    #[test]
    fn bipolar_slider_mode_uses_zero_as_the_fill_origin() {
        let node = test_number_picker_node(HashMap::from([
            ("mode".to_string(), Value::Keyword("slider".to_string())),
            ("value".to_string(), Value::Number(-5.0)),
            ("min".to_string(), Value::Number(-10.0)),
            ("max".to_string(), Value::Number(10.0)),
        ]));

        let primitives =
            NumberPickerWidget.build_primitives("number-picker", &node, test_viewport());
        let slider = primitives
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } if widget_type == "number-picker-slider" => Some(instance),
                _ => None,
            })
            .expect("bipolar slider mode should emit a slider primitive");

        assert!((slider.value_t - 0.25).abs() < f32::EPSILON);
        assert!((slider.uniform_a[1] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn focused_noui_number_picker_uses_shared_focus_corners_only() {
        let rect = Rect {
            row: 2.0,
            col: 4.0,
            width: 8.0,
            height: 1.4,
        };
        let noui_node = test_number_picker_node(HashMap::from([
            ("value".to_string(), Value::Number(34.0)),
            ("noui".to_string(), Value::Bool(true)),
        ]));
        let focused_viewport = WidgetViewport {
            focused_widget_id: Some(noui_node.widget_id),
            ..test_viewport()
        };
        let primitives =
            crate::widget_render::widget_primitives_for_node(&noui_node, focused_viewport);
        let corners: Vec<&GpuRectPrimitive> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                GpuPrimitive::ForegroundRect(corner) => Some(corner),
                _ => None,
            })
            .collect();

        assert_eq!(corners.len(), 8);
        for corner in corners {
            assert!(corner.rect.col >= rect.col);
            assert!(corner.rect.row >= rect.row);
            assert!(corner.rect.col + corner.rect.width <= rect.col + rect.width);
            assert!(corner.rect.row + corner.rect.height <= rect.row + rect.height);
        }

        let graphical_node = test_number_picker_node(HashMap::from([
            ("value".to_string(), Value::Number(34.0)),
            ("noui".to_string(), Value::Bool(false)),
        ]));
        let graphical_primitives = crate::widget_render::widget_primitives_for_node(
            &graphical_node,
            WidgetViewport {
                focused_widget_id: Some(graphical_node.widget_id),
                ..test_viewport()
            },
        );
        assert!(
            graphical_primitives
                .iter()
                .all(|primitive| !matches!(primitive, GpuPrimitive::ForegroundRect(_)))
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberPickerEditOutcome {
    StateChanged,
    Commit(f64),
}

thread_local! {
    static STATES: RefCell<HashMap<u64, NumberPickerEditState>> = RefCell::new(HashMap::new());
    /// Per-character cell widths for digits, '.', '-' at a given font size.
    /// Key: font_size_bits → HashMap<char, cell_width>.
    static CHAR_WIDTHS: RefCell<HashMap<u32, HashMap<char, f32>>> =
        RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> NumberPickerEditState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

pub fn number_picker_edit_state(widget_id: u64) -> NumberPickerEditState {
    get_state(widget_id)
}

fn set_state(widget_id: u64, state: NumberPickerEditState) {
    STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    // Own-widget-only edit/drag state (eseq-eeng): see
    // `bump_widget_state_revision`.
    super::bump_widget_state_revision(widget_id);
}

pub fn clear_number_picker_edit_state(widget_id: u64) {
    set_state(widget_id, NumberPickerEditState::default());
}

fn format_value(value: f64, decimals: u32) -> String {
    format!("{:.*}", decimals as usize, value)
}

// Optional display aliases for sentinel values, e.g. ((-1 "inf")). Values
// remain numeric for dragging, automation and text editing. Match in the
// model domain, before value-scale is applied.
fn formatted_model_value(props: &HashMap<String, Value>, value: f32, decimals: u32) -> String {
    if let Some(Value::List(labels)) = props.get("value-labels") {
        for entry in labels {
            let entry = entry.borrow();
            let Value::List(pair) = &*entry else { continue };
            if pair.len() != 2 { continue; }
            if let (Value::Number(key), Value::String(label)) = (&*pair[0].borrow(), &*pair[1].borrow()) {
                if *key as f32 == value { return label.clone(); }
            }
        }
    }
    format_value(display_value(props, value) as f64, decimals)
}

fn value_scale(props: &HashMap<String, Value>) -> f32 {
    get_f32_prop(props, "value-scale", 1.0).max(0.000001)
}

fn display_value(props: &HashMap<String, Value>, value: f32) -> f32 {
    value * value_scale(props)
}

fn number_picker_edit_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(
        props,
        "edit-color",
        Color {
            r: 1.0,
            g: 0.95,
            b: 0.25,
            a: 1.0,
        },
    )
}

fn model_value_from_display(props: &HashMap<String, Value>, value: f32) -> f32 {
    value / value_scale(props)
}

fn quantize_step(props: &HashMap<String, Value>, decimals: u32) -> Option<f32> {
    let explicit_step = get_f32_prop(props, "step", 0.0);
    if explicit_step > 0.0 {
        Some(explicit_step)
    } else if decimals == 0 {
        Some(1.0 / value_scale(props))
    } else {
        None
    }
}

fn quantize_value(value: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    let clamped = value.clamp(min, max);
    let Some(step) = step.filter(|step| *step > 0.0) else {
        return clamped;
    };
    let steps_from_min = ((clamped - min) / step).round();
    (min + steps_from_min * step).clamp(min, max)
}

pub fn number_picker_display_text(state: &NumberPickerEditState) -> String {
    match state.edit_text.as_str() {
        "." => "0.".to_string(),
        "-." => "-0.".to_string(),
        text => text.to_string(),
    }
}

pub fn handle_number_picker_edit_key(
    state: &mut NumberPickerEditState,
    key: WidgetKeyEvent,
    value: f64,
    min: f64,
    max: f64,
    decimals: u32,
) -> Option<NumberPickerEditOutcome> {
    match key.code {
        KeyCode::Char(c)
            if (c.is_ascii_digit() || c == '-' || (state.editing && c == '.'))
                && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
        {
            if !state.editing {
                state.editing = true;
                state.edit_text.clear();
                state.cursor_pos = 0;
            }
            state.edit_text.insert(state.cursor_pos, c);
            state.cursor_pos += 1;
            Some(NumberPickerEditOutcome::StateChanged)
        }
        KeyCode::Backspace if state.editing => {
            if state.cursor_pos > 0 {
                state.cursor_pos -= 1;
                state.edit_text.remove(state.cursor_pos);
            }
            if state.edit_text.is_empty() {
                state.editing = false;
            }
            Some(NumberPickerEditOutcome::StateChanged)
        }
        KeyCode::Left if state.editing => {
            state.cursor_pos = state.cursor_pos.saturating_sub(1);
            Some(NumberPickerEditOutcome::StateChanged)
        }
        KeyCode::Right if state.editing => {
            state.cursor_pos = (state.cursor_pos + 1).min(state.edit_text.len());
            Some(NumberPickerEditOutcome::StateChanged)
        }
        KeyCode::Enter => {
            if state.editing {
                let parsed = state
                    .edit_text
                    .parse::<f64>()
                    .unwrap_or(value)
                    .clamp(min, max);
                state.editing = false;
                state.edit_text.clear();
                state.cursor_pos = 0;
                Some(NumberPickerEditOutcome::Commit(parsed))
            } else {
                state.editing = true;
                state.edit_text = format_value(value, decimals);
                state.cursor_pos = state.edit_text.len();
                Some(NumberPickerEditOutcome::StateChanged)
            }
        }
        KeyCode::Esc => {
            state.editing = false;
            state.edit_text.clear();
            state.cursor_pos = 0;
            Some(NumberPickerEditOutcome::StateChanged)
        }
        _ => None,
    }
}

pub fn handle_number_picker_edit_key_for_widget(
    widget_id: u64,
    key: WidgetKeyEvent,
    value: f64,
    min: f64,
    max: f64,
    decimals: u32,
) -> Option<NumberPickerEditOutcome> {
    let mut state = get_state(widget_id);
    let outcome = handle_number_picker_edit_key(&mut state, key, value, min, max, decimals)?;
    set_state(widget_id, state);
    Some(outcome)
}

/// Compute text X offset in cells by summing cached per-character widths.
fn cursor_x_from_cache(text: &str, cursor_pos: usize, font_size: f32, cell_w: f32) -> f32 {
    let key = font_size.to_bits();
    CHAR_WIDTHS.with(|cw| {
        let cache = cw.borrow();
        if let Some(widths) = cache.get(&key) {
            let fallback = font_size * 0.55 / cell_w;
            text.chars()
                .take(cursor_pos)
                .map(|ch| widths.get(&ch).copied().unwrap_or(fallback))
                .sum()
        } else {
            cursor_pos as f32 * font_size * 0.55 / cell_w
        }
    })
}

fn number_picker_bg(props: &HashMap<String, Value>) -> Color {
    props
        .get("background-color")
        .and_then(theme::parse_color_value)
        .or_else(|| props.get("bg-color").and_then(theme::parse_color_value))
        .unwrap_or_else(theme::DROPDOWN_BG)
}

fn number_picker_border(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "border-color", theme::BUTTON_BORDER())
}

fn number_picker_highlight(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "highlight-color", theme::BUTTON_HIGHLIGHT())
}

fn number_picker_shadow(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "shadow-color", theme::BUTTON_SHADOW())
}

fn number_picker_slider_mode(props: &HashMap<String, Value>) -> bool {
    matches!(
        props.get("mode"),
        Some(Value::Keyword(mode) | Value::String(mode)) if mode == "slider"
    )
}

fn number_picker_fill(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "fill-color", theme::WIDGET_SLIDER_FILLED())
}

fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((super::ui_design_px(radius_px) * 2.0) / px_h).clamp(0.001, 0.5)
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct NumberPickerWidget;
pub static NUMBER_PICKER_WIDGET: NumberPickerWidget = NumberPickerWidget;

impl WidgetDefinition for NumberPickerWidget {
    fn names(&self) -> &'static [&'static str] {
        &["number-picker", "number-picker-tri", "number-picker-slider"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "font-size", "decimals", "noui", "unit", "value-labels"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "value",
            "active",
            "border-width",
            "corner-radius",
            "plock-active",
            "plock-color-r",
            "plock-color-g",
            "plock-color-b",
        ]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "value",
            "min",
            "max",
            "step",
            "decimals",
            "unit",
            "value-scale",
            "value-labels",
            "taper",
            "drag-rows",
            "width",
            "height",
            "font-size",
            "noui",
            "mode",
            "text-align",
            "text-color",
            "active-color",
            "background-color",
            "bg-color",
            "border-color",
            "cursor-color",
            "edit-color",
            "fill-color",
            "focus-color",
            "highlight-color",
            "ring-color",
            "shadow-color",
            "tri-color",
            "border-width",
            "corner-radius",
            "active",
            "on-change",
            "on-release",
            "plock-active",
            "plock-color-r",
            "plock-color-g",
            "plock-color-b",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Cache per-character widths for digit rendering
        if let Some(measurer) = ctx.text_measurer {
            let font_size = get_prop_num(node, "font-size")
                .map(f64_to_f32)
                .unwrap_or(ctx.inherited_font_size);
            let key = font_size.to_bits();
            CHAR_WIDTHS.with(|cw| {
                let mut cache = cw.borrow_mut();
                if !cache.contains_key(&key) {
                    let mut widths = HashMap::new();
                    for ch in
                        "0123456789.- %abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars()
                    {
                        let px = measurer.measure_text_px(&ch.to_string(), font_size);
                        widths.insert(ch, px / ctx.cell_w);
                    }
                    cache.insert(key, widths);
                }
            });
        }
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(8.0),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.4),
        })
    }

    fn baseline_offset(&self, node: &Value, size: Size, ctx: &MeasureCtx<'_>) -> Option<f32> {
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        Some(super::proportional_text_baseline_offset(
            font_size,
            (size.height - 1.0) * 0.5,
            ctx,
        ))
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn hidden_drag(&self) -> bool {
        true
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        // Store start value and start row (layout-space).
        // local_row ≈ screen_row_from_content_top + scroll, so for non-scrolled
        // views it equals the screen distance from the content area top.
        let value = get_f32_prop(&node.props, "value", 0.0);
        Some(Value::List(vec![
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(value as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_row as f64))),
        ]))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Exit any existing edit mode on fresh click
                let mut state = get_state(node.widget_id);
                state.editing = false;
                set_state(node.widget_id, state);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(Value::List(gesture_list)) = gesture else {
                    return MouseEventOutcome::Consume;
                };
                let (start_value, start_row) = match (gesture_list.first(), gesture_list.get(1)) {
                    (Some(v), Some(r)) => {
                        let sv = match &*v.borrow() {
                            Value::Number(n) => *n as f32,
                            _ => return MouseEventOutcome::Consume,
                        };
                        let sr = match &*r.borrow() {
                            Value::Number(n) => *n as f32,
                            _ => return MouseEventOutcome::Consume,
                        };
                        (sv, sr)
                    }
                    _ => return MouseEventOutcome::Consume,
                };

                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;
                let step = quantize_step(&node.props, decimals);
                let delta_rows = start_row - local_row; // positive = dragging up

                // Dynamic sensitivity: dragging up to the content-area top
                // (start_row rows above) reaches max; equal distance below → min.
                // Minimum of 5 rows to avoid division-by-near-zero. `:drag-rows`
                // pins the full-travel distance instead, for pickers that sit
                // in a short strip but span a wide range (the *step* panel's
                // retrig pickers): there the content-area rule made five rows
                // of motion cover the whole range.
                // Under a hidden-cursor drag the pointer is virtual and can
                // walk far past the window, so "distance to the content-area
                // top" is meaningless; use a fixed travel distance instead.
                let room = match get_f32_prop(&node.props, "drag-rows", 0.0) {
                    rows if rows > 0.0 => rows,
                    _ if super::hidden_drag_event_active() => HIDDEN_DRAG_ROWS,
                    _ => start_row.max(5.0),
                };

                // `:taper` (see knob_number): the drag walks normalized
                // position, so a log/cube picker spends its travel where the
                // musical action is instead of racing to max.
                let taper = knob_taper(&node.props);
                let start_t = taper_normalize(taper, min, max, start_value);
                let new_t = if delta_rows >= 0.0 {
                    // Dragging up → towards max
                    let frac = (delta_rows / room).min(1.0);
                    start_t + frac * (1.0 - start_t)
                } else {
                    // Dragging down → towards min
                    let frac = ((-delta_rows) / room).min(1.0);
                    start_t - frac * start_t
                };
                let new_value = taper_denormalize(taper, min, max, new_t);
                let new_value = quantize_value(new_value, min, max, step);

                MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(new_value as f64)))
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Gesture end: fire the optional on-release callback (the
                // drag capture routes the Up here even when the pointer left
                // the widget). Number payloads mean value changes, so the
                // release rides a keyword sentinel.
                if node
                    .props
                    .get("on-release")
                    .is_some_and(|v| !matches!(v, Value::Nil | Value::Bool(false)))
                {
                    MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Keyword(
                        "release".to_string(),
                    )))
                } else {
                    MouseEventOutcome::Consume
                }
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let mut state = get_state(node.widget_id);
        let value = get_f32_prop(&node.props, "value", 0.0) as f64;
        let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;
        let min = get_f32_prop(&node.props, "min", 0.0) as f64;
        let max = get_f32_prop(&node.props, "max", 1.0) as f64;
        let displayed_value = display_value(&node.props, value as f32) as f64;
        let displayed_min = display_value(&node.props, min as f32) as f64;
        let displayed_max = display_value(&node.props, max as f32) as f64;

        match handle_number_picker_edit_key(
            &mut state,
            key,
            displayed_value,
            displayed_min,
            displayed_max,
            decimals,
        )? {
            NumberPickerEditOutcome::StateChanged => {
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            NumberPickerEditOutcome::Commit(value) => {
                let value = model_value_from_display(&node.props, value as f32) as f64;
                let step = quantize_step(&node.props, decimals);
                let value = quantize_value(value as f32, min as f32, max as f32, step) as f64;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Number(value)))
            }
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(ref value) = event else {
            return None;
        };
        // Nil = cursor/edit state change only, no callback
        if matches!(value, Value::Nil) {
            return None;
        }
        // :release sentinel = the drag gesture ended; route to on-release.
        if matches!(value, Value::Keyword(keyword) if keyword == "release") {
            let callback = node
                .props
                .get("on-release")
                .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
                .cloned()?;
            return Some(EventOutput {
                callback,
                args: Vec::new(),
            });
        }
        let Value::Number(new_value) = value else {
            return None;
        };
        let previous = get_f32_prop(&node.props, "value", 0.0);
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;
        let step = quantize_step(&node.props, decimals);
        let new_value = quantize_value(*new_value as f32, min, max, step);
        if should_trigger_integer_haptic(node.widget_id, previous, new_value, min, max) {
            trigger_level_change_haptic();
        }
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput {
            callback,
            args: vec![Value::Number(new_value as f64)],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let value = get_f32_prop(props, "value", 0.0);
        let decimals = get_f32_prop(props, "decimals", 2.0) as u32;
        let unit = match props.get("unit") {
            Some(Value::String(unit)) => unit.as_str(),
            _ => "",
        };
        let value_text = formatted_model_value(props, value, decimals);
        let display_value = if unit.is_empty() {
            value_text
        } else {
            format!("{value_text} {unit}")
        };
        let text = if number_picker_slider_mode(props) {
            display_value
        } else {
            format!("▶ {display_value}")
        };
        let fg = resolve_named_color(props, "text-color", theme::BUTTON_SECONDARY_FG());
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        let max_col = col_start + rect.width.round() as u16;
        for (i, ch) in text.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= max_col {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn fragment_shader(
        &self,
        widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        match widget_type {
            "number-picker" => super::button::BUTTON_SURFACE_SHADER.source(backend),
            "number-picker-tri" => NUMBER_PICKER_TRI_SHADER.source(backend),
            "number-picker-slider" => NUMBER_PICKER_SLIDER_SHADER.source(backend),
            _ => None,
        }
    }

    fn focus_decoration(&self, node: &LayoutNode) -> FocusDecoration {
        if !get_bool_prop(&node.props, "noui", false) {
            return FocusDecoration::None;
        }
        FocusDecoration::Corners(FocusCornerStyle::new(resolve_named_color(
            &node.props,
            "focus-color",
            number_picker_edit_color(&node.props),
        )))
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let value = get_f32_prop(&node.props, "value", 0.0);
        let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);
        let effective_focused = is_focused || state.editing;
        let noui = get_bool_prop(&node.props, "noui", false);
        let slider_mode = number_picker_slider_mode(&node.props);
        let corner_radius = get_f32_prop(&node.props, "corner-radius", 12.0);
        let unit = match node.props.get("unit") {
            Some(Value::String(unit)) => unit.as_str(),
            _ => "",
        };
        let text_align = match node.props.get("text-align") {
            Some(Value::Keyword(align)) => align.as_str(),
            _ => "start",
        };

        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);

        let active = get_bool_prop(&node.props, "active", false);
        let plocked = plock_active(&node.props);
        let plock_color = plock_color(&node.props);
        let active_color = resolve_named_color(
            &node.props,
            "active-color",
            Color::rgba(113.0 / 255.0, 191.0 / 255.0, 207.0 / 255.0, 1.0),
        );
        let text_color = if active {
            active_color
        } else if plocked {
            plock_color
        } else {
            resolve_named_color(&node.props, "text-color", theme::BUTTON_SECONDARY_FG())
        };
        let edit_color = number_picker_edit_color(&node.props);
        let focus_color = resolve_named_color(&node.props, "focus-color", edit_color);
        let cursor_color = resolve_named_color(
            &node.props,
            "cursor-color",
            Color {
                r: 1.0,
                g: 0.95,
                b: 0.25,
                a: 1.0,
            },
        );

        let mut prims = Vec::new();

        if !noui {
            let bg_color = number_picker_bg(&node.props);
            let ring_color = resolve_named_color(&node.props, "ring-color", theme::DROPDOWN_RING());
            let tri_color =
                resolve_named_color(&node.props, "tri-color", theme::BUTTON_SECONDARY_FG());
            let border_color = if slider_mode && effective_focused {
                focus_color
            } else if plocked {
                plock_color
            } else {
                number_picker_border(&node.props)
            };
            let highlight_color = number_picker_highlight(&node.props);
            let shadow_color = number_picker_shadow(&node.props);

            // ── Focus ring ──
            if effective_focused && !slider_mode {
                let ring_v = RING_WIDTH;
                let ring_h = RING_WIDTH * viewport.cell_h / viewport.cell_w;
                let ring_rect = Rect {
                    row: node.rect.row - ring_v,
                    col: node.rect.col - ring_h,
                    width: node.rect.width + ring_h * 2.0,
                    height: node.rect.height + ring_v * 2.0,
                };
                let (ndc_min, ndc_max) = ndc_bounds(ring_rect, viewport);
                let px_w = ring_rect.width * viewport.cell_w;
                let px_h = ring_rect.height * viewport.cell_h;
                prims.push(GpuPrimitive::WidgetInstance {
                    widget_type: "number-picker".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: ring_color.to_rgba(),
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: normalized_corner_radius(ring_rect, viewport, corner_radius),
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: true,
                });
            }

            // Slider mode uses one flat surface primitive below. The standard
            // mode keeps the softly lit button surface.
            if !slider_mode {
                let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
                let px_w = node.rect.width * viewport.cell_w;
                let px_h = node.rect.height * viewport.cell_h;
                prims.push(GpuPrimitive::WidgetInstance {
                    widget_type: "number-picker".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: bg_color.to_rgba(),
                        color_b: border_color.to_rgba(),
                        color_c: highlight_color.to_rgba(),
                        color_d: shadow_color.to_rgba(),
                        corner_radius: normalized_corner_radius(node.rect, viewport, corner_radius),
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: true,
                });
            }

            if slider_mode {
                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                let range = max - min;
                let normalized_value = if range > 0.0 {
                    ((value - min) / range).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let fill_origin = if min < 0.0 && max > 0.0 {
                    (-min / range).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
                let px_w = node.rect.width * viewport.cell_w;
                let px_h = node.rect.height * viewport.cell_h;
                prims.push(GpuPrimitive::WidgetInstance {
                    widget_type: "number-picker-slider".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: normalized_value,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [
                            get_f32_prop(&node.props, "border-width", 2.0).max(0.0),
                            fill_origin,
                            0.0,
                            0.0,
                        ],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: bg_color.to_rgba(),
                        color_b: border_color.to_rgba(),
                        color_c: number_picker_fill(&node.props).to_rgba(),
                        color_d: [0.0; 4],
                        corner_radius: normalized_corner_radius(node.rect, viewport, corner_radius),
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: true,
                });
            } else {
                // ── Triangle indicator ──
                let tri_h = node.rect.height * 0.5;
                let tri_w = tri_h * 1.5;
                let tri_rect = Rect {
                    row: node.rect.row + (node.rect.height - tri_h) * 0.5,
                    col: node.rect.col + TEXT_PADDING_H * 0.6,
                    width: tri_w,
                    height: tri_h,
                };
                let (ndc_min, ndc_max) = ndc_bounds(tri_rect, viewport);
                let px_w = tri_rect.width * viewport.cell_w;
                let px_h = tri_rect.height * viewport.cell_h;
                prims.push(GpuPrimitive::WidgetInstance {
                    widget_type: "number-picker-tri".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: tri_color.to_rgba(),
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }
        }

        // ── Value text ──
        let base_text_col = if noui || slider_mode {
            node.rect.col + TEXT_PADDING_H
        } else {
            node.rect.col + TEXT_PADDING_H + TRIANGLE_WIDTH
        };
        let text_row = node.rect.row + (node.rect.height - 1.0) * 0.5;
        let transparent = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        let value_text = formatted_model_value(&node.props, value, decimals);
        let display_value = if unit.is_empty() {
            value_text
        } else {
            format!("{value_text} {unit}")
        };
        let (display_text, fg) = if state.editing {
            (state.edit_text.clone(), edit_color)
        } else if effective_focused {
            (display_value, edit_color)
        } else {
            (display_value, text_color)
        };
        let text_width = cursor_x_from_cache(
            &display_text,
            display_text.chars().count(),
            font_size,
            viewport.cell_w,
        );
        let text_col = match text_align {
            "center" => node.rect.col + ((node.rect.width - text_width).max(0.0) * 0.5),
            "right" => node.rect.col + (node.rect.width - text_width - TEXT_PADDING_H).max(0.0),
            _ => base_text_col,
        };

        if !display_text.is_empty() {
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
                    row: text_row,
                    col: text_col,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: display_text.clone(),
                    font_size,
                    scale: 1.0,
                    fg,
                    bg: transparent,
                },
            ));
        }

        // ── Cursor (when editing) ──
        if effective_focused && state.editing {
            let cursor_pos = state.cursor_pos;
            let cursor_x =
                cursor_x_from_cache(&display_text, cursor_pos, font_size, viewport.cell_w);
            let cursor_col = text_col + cursor_x;
            let cursor_rect = Rect {
                row: node.rect.row + 0.15,
                col: cursor_col,
                width: 0.08,
                height: node.rect.height - 0.3,
            };
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: cursor_rect,
                color: cursor_color,
            }));
        }

        if plocked || active {
            let underline_rect = Rect {
                row: node.rect.row + node.rect.height - 0.08,
                col: node.rect.col,
                width: node.rect.width,
                height: 0.06,
            };
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: underline_rect,
                color: if active { active_color } else { plock_color },
            }));
        }

        prims
    }
}

// ── GPU shaders ──────────────────────────────────────────────────────────────

const NUMBER_PICKER_SLIDER_SHADER: super::ShaderSources = super::ShaderSources::both(
    r#"
float number_picker_slider_rounded_rect(float2 p, float2 size, float radius)
{
    float2 q = abs(p) - (size - float2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);
    float2 size = float2(aspect, 1.0);
    float radius = min(in.corner_radius, min(aspect, 1.0));

    float outer_distance = number_picker_slider_rounded_rect(p, size, radius);
    float outer_edge = fwidth(outer_distance) * 1.2;
    float outer_mask = smoothstep(outer_edge, -outer_edge, outer_distance);

    float px = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    float border_width = in.uniform_a.x * px;
    float2 inner_size = max(size - float2(border_width), float2(0.001));
    float inner_distance = number_picker_slider_rounded_rect(
        p,
        inner_size,
        max(radius - border_width, 0.0)
    );
    float inner_edge = fwidth(inner_distance) * 1.2;
    float inner_mask = smoothstep(inner_edge, -inner_edge, inner_distance);
    float border_mask = clamp(outer_mask - inner_mask, 0.0, 1.0);

    float cutoff_edge = max(fwidth(in.uv.x), 0.0005);
    float segment_start = min(in.value_t, in.uniform_a.y);
    float segment_end = max(in.value_t, in.uniform_a.y);
    float left_mask = smoothstep(segment_start - cutoff_edge, segment_start + cutoff_edge, in.uv.x);
    float right_mask = smoothstep(segment_end + cutoff_edge, segment_end - cutoff_edge, in.uv.x);
    float segment_mask = left_mask * right_mask * step(0.0001, abs(in.value_t - in.uniform_a.y));
    float fill_alpha = in.color_c.a * segment_mask;
    float inner_alpha = fill_alpha + in.color_a.a * (1.0 - fill_alpha);
    float3 inner_rgb = (
        in.color_c.rgb * fill_alpha
        + in.color_a.rgb * in.color_a.a * (1.0 - fill_alpha)
    ) / max(inner_alpha, 0.0001);

    float surface_alpha = inner_alpha * inner_mask;
    float border_alpha = in.color_b.a * border_mask;
    float out_alpha = border_alpha + surface_alpha * (1.0 - border_alpha);
    if (out_alpha < 0.002) { discard_fragment(); }
    float3 out_rgb = (
        in.color_b.rgb * border_alpha
        + inner_rgb * surface_alpha * (1.0 - border_alpha)
    ) / out_alpha;
    return float4(out_rgb, out_alpha);
}
"#,
    super::wgsl::NUMBER_PICKER_SLIDER_SHADER,
);

const NUMBER_PICKER_TRI_SHADER: super::ShaderSources = super::ShaderSources::both(
    r#"
float number_picker_segment_distance(float2 p, float2 a, float2 b)
{
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
    return length(pa - ba * h);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float4 col = in.color_a;

    // Aspect-corrected coordinates
    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Right-pointing filled triangle (play button style)
    // Vertices: left-top (-0.5a, -0.7), left-bottom (-0.5a, 0.7), right (0.6a, 0)
    float2 a = float2(-0.5 * aspect, -0.7);
    float2 b = float2(-0.5 * aspect,  0.7);
    float2 c = float2( 0.6 * aspect,  0.0);

    float d1 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    float d2 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
    float d3 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);

    bool has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    bool has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    bool inside = !(has_neg && has_pos);
    float edge_distance = min(
        number_picker_segment_distance(p, a, b),
        min(number_picker_segment_distance(p, b, c), number_picker_segment_distance(p, c, a))
    );
    float signed_distance = inside ? -edge_distance : edge_distance;
    float aa = max(fwidth(signed_distance), 0.001) * 1.35;
    float mask = smoothstep(aa, -aa, signed_distance);

    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#,
    super::wgsl::NUMBER_PICKER_TRI_SHADER,
);
