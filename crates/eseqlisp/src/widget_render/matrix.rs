use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MetalPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    WidgetInstance, WidgetViewport, bump_widget_state_generation, get_bool_prop, get_f32_prop,
    ndc_bounds, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct MatrixWidget;

pub static MATRIX_WIDGET: MatrixWidget = MatrixWidget;

const DEFAULT_ROWS: usize = 8;
const DEFAULT_COLS: usize = 8;
const CLICK_ANIMATION_SECONDS: f32 = 0.2;

thread_local! {
    static MATRIX_STATES: RefCell<HashMap<u64, MatrixInteractionState>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MatrixInteractionState {
    hovered_cell: Option<usize>,
    active_cell: Option<usize>,
    clicked_cell: Option<usize>,
    release_time: f32,
    drag_start_row: f32,
    drag_start_value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatrixControl {
    Circle,
    Line,
    Grid,
}

fn value_cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn matrix_value(matrix: Vec<Vec<f32>>) -> Value {
    Value::List(
        matrix
            .into_iter()
            .map(|row| {
                value_cell(Value::List(
                    row.into_iter()
                        .map(|value| value_cell(Value::Number(value as f64)))
                        .collect(),
                ))
            })
            .collect(),
    )
}

fn prop_usize(node: &Value, key: &str, default: usize) -> usize {
    get_prop_num(node, key)
        .map(|value| value.max(1.0).round() as usize)
        .unwrap_or(default)
}

fn matrix_rows(node: &Value) -> usize {
    prop_usize(node, "rows", DEFAULT_ROWS)
}

fn matrix_cols(node: &Value) -> usize {
    get_prop_num(node, "cols")
        .or_else(|| get_prop_num(node, "columns"))
        .map(|value| value.max(1.0).round() as usize)
        .unwrap_or(DEFAULT_COLS)
}

fn matrix_rows_from_props(props: &HashMap<String, Value>) -> usize {
    props
        .get("rows")
        .and_then(number_value)
        .map(|value| value.max(1.0).round() as usize)
        .unwrap_or(DEFAULT_ROWS)
}

fn matrix_cols_from_props(props: &HashMap<String, Value>) -> usize {
    props
        .get("cols")
        .or_else(|| props.get("columns"))
        .and_then(number_value)
        .map(|value| value.max(1.0).round() as usize)
        .unwrap_or(DEFAULT_COLS)
}

fn number_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) if value.is_finite() => Some(*value),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot)),
        _ => None,
    }
}

fn control_from_props(props: &HashMap<String, Value>) -> MatrixControl {
    match props.get("control").or_else(|| props.get("control-type")) {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "line" => {
            MatrixControl::Line
        }
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if matches!(
                value.as_str(),
                "grid" | "square" | "squares" | "cell" | "cells"
            ) =>
        {
            MatrixControl::Grid
        }
        _ => MatrixControl::Circle,
    }
}

fn flatten_numbers(value: &Value, out: &mut Vec<f32>) {
    match value {
        Value::Number(number) if number.is_finite() => out.push(*number as f32),
        Value::ReactiveRef { slot, .. } => out.push(crate::reactive::read_float_slot(slot) as f32),
        Value::List(items) => {
            for item in items {
                flatten_numbers(&item.borrow(), out);
            }
        }
        _ => {}
    }
}

fn parse_matrix_value(props: &HashMap<String, Value>, rows: usize, cols: usize) -> Vec<Vec<f32>> {
    let min = get_f32_prop(props, "min", 0.0);
    let mut matrix = vec![vec![min; cols]; rows];
    let Some(value) = props.get("value") else {
        return matrix;
    };

    if let Value::List(items) = value {
        let nested_rows = items
            .iter()
            .all(|item| matches!(&*item.borrow(), Value::List(_)));
        if nested_rows {
            for (row_idx, row_value) in items.iter().take(rows).enumerate() {
                let Value::List(row_items) = &*row_value.borrow() else {
                    continue;
                };
                for (col_idx, cell) in row_items.iter().take(cols).enumerate() {
                    if let Some(number) = number_value(&cell.borrow()) {
                        matrix[row_idx][col_idx] = number as f32;
                    }
                }
            }
            return matrix;
        }
    }

    let mut flat = Vec::with_capacity(rows * cols);
    flatten_numbers(value, &mut flat);
    for (idx, value) in flat.into_iter().take(rows * cols).enumerate() {
        matrix[idx / cols][idx % cols] = value;
    }
    matrix
}

fn quantize_value(props: &HashMap<String, Value>, value: f32) -> f32 {
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let lo = min.min(max);
    let hi = min.max(max);
    let mut value = value.clamp(lo, hi);
    if get_bool_prop(props, "round", false) {
        value = value.round();
    }
    value
}

fn normalized_value(props: &HashMap<String, Value>, value: f32) -> f32 {
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    if range.abs() > f32::EPSILON {
        ((value - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn cell_index(node: &LayoutNode, local_col: f32, local_row: f32) -> usize {
    let rows = matrix_rows_from_props(&node.props);
    let cols = matrix_cols_from_props(&node.props);
    let col_t = ((local_col - node.rect.col) / node.rect.width.max(0.001)).clamp(0.0, 0.999_999);
    let row_t = ((local_row - node.rect.row) / node.rect.height.max(0.001)).clamp(0.0, 0.999_999);
    let col = (col_t * cols as f32).floor() as usize;
    let row = (row_t * rows as f32).floor() as usize;
    row.min(rows - 1) * cols + col.min(cols - 1)
}

fn get_state(widget_id: u64) -> MatrixInteractionState {
    MATRIX_STATES.with(|states| states.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn update_state(widget_id: u64, update: impl FnOnce(&mut MatrixInteractionState)) {
    MATRIX_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.entry(widget_id).or_default();
        let before = state.clone();
        update(state);
        if *state != before {
            bump_widget_state_generation();
        }
    });
}

fn state_animation_active(state: MatrixInteractionState, time_seconds: f32) -> bool {
    state.clicked_cell.is_some()
        && state.release_time > 0.0
        && time_seconds - state.release_time < CLICK_ANIMATION_SECONDS + 0.05
}

fn change_for_cell(
    node: &LayoutNode,
    cell: usize,
    new_value: f32,
    rows: usize,
    cols: usize,
) -> Vec<Vec<f32>> {
    let mut matrix = parse_matrix_value(&node.props, rows, cols);
    matrix[cell / cols][cell % cols] = quantize_value(&node.props, new_value);
    matrix
}

fn set_matrix_cell(
    props: &HashMap<String, Value>,
    matrix: &mut [Vec<f32>],
    cell: usize,
    cols: usize,
    value: f32,
) {
    matrix[cell / cols][cell % cols] = quantize_value(props, value);
}

fn handle_drag(node: &LayoutNode, local_row: f32) -> MouseEventOutcome {
    let rows = matrix_rows_from_props(&node.props);
    let cols = matrix_cols_from_props(&node.props);
    let state = get_state(node.widget_id);
    let Some(cell) = state.active_cell else {
        return MouseEventOutcome::Consume;
    };
    let min = get_f32_prop(&node.props, "min", 0.0);
    let max = get_f32_prop(&node.props, "max", 1.0);
    let range = max - min;
    let cell_height = (node.rect.height / rows as f32).max(0.001);
    let delta_cells = (state.drag_start_row - local_row) / cell_height;
    let new_value = state.drag_start_value + delta_cells * range;
    dispatch_cell_change(node, cell, new_value, rows, cols)
}

/// Build the change event for a single cell. When the widget declares an
/// `on-cell-change` callback we dispatch only the changed cell `(row col value)`
/// so the host can persist one entry instead of re-serializing (and re-applying)
/// the entire matrix on every drag frame. Without it we fall back to the legacy
/// `on-change` full-matrix payload.
fn dispatch_cell_change(
    node: &LayoutNode,
    cell: usize,
    new_value: f32,
    rows: usize,
    cols: usize,
) -> MouseEventOutcome {
    if has_callback(node, "on-cell-change") {
        let value = quantize_value(&node.props, new_value);
        MouseEventOutcome::Dispatch(WidgetEvent::Custom(cell_change_value(
            cell / cols,
            cell % cols,
            value,
        )))
    } else {
        let matrix = change_for_cell(node, cell, new_value, rows, cols);
        MouseEventOutcome::Dispatch(WidgetEvent::Custom(matrix_value(matrix)))
    }
}

fn has_callback(node: &LayoutNode, key: &str) -> bool {
    matches!(node.props.get(key), Some(value) if !matches!(value, Value::Nil | Value::Bool(false)))
}

fn cell_change_value(row: usize, col: usize, value: f32) -> Value {
    Value::List(vec![
        value_cell(Value::Number(row as f64)),
        value_cell(Value::Number(col as f64)),
        value_cell(Value::Number(value as f64)),
    ])
}

fn handle_toggle(node: &LayoutNode, local_col: f32, local_row: f32) -> MouseEventOutcome {
    let rows = matrix_rows_from_props(&node.props);
    let cols = matrix_cols_from_props(&node.props);
    let cell = cell_index(node, local_col, local_row);
    let mut matrix = parse_matrix_value(&node.props, rows, cols);
    let current = matrix[cell / cols][cell % cols];
    let min = get_f32_prop(&node.props, "min", 0.0);
    let max = get_f32_prop(&node.props, "max", 1.0);
    let midpoint = (min + max) * 0.5;
    let new_value = if current > midpoint { min } else { max };
    set_matrix_cell(&node.props, &mut matrix, cell, cols, new_value);
    update_state(node.widget_id, |state| {
        state.active_cell = Some(cell);
        state.clicked_cell = Some(cell);
        state.release_time = 0.0;
    });
    dispatch_cell_change(node, cell, new_value, rows, cols)
}

fn color_mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

fn default_cell_color(bg: Color) -> Color {
    color_mix(bg, Color::WHITE, 0.16)
}

fn default_hover_cell_color(bg: Color) -> Color {
    color_mix(bg, Color::WHITE, 0.34)
}

fn resolve_matrix_color(props: &HashMap<String, Value>, keys: &[&str], default: Color) -> Color {
    keys.iter()
        .find_map(|key| props.get(*key).and_then(theme::parse_color_value))
        .unwrap_or(default)
}

fn fill_color(props: &HashMap<String, Value>) -> Color {
    resolve_matrix_color(
        props,
        &["fill", "fill-color", "cell-color", "color"],
        Color::WHITE,
    )
}

fn background_color(props: &HashMap<String, Value>) -> Color {
    resolve_matrix_color(
        props,
        &["background", "background-color", "bg"],
        theme::BUTTON_GHOST_BG(),
    )
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let rows = matrix_rows_from_props(props);
    let cols = matrix_cols_from_props(props);
    let matrix = parse_matrix_value(props, rows, cols);
    let fg = fill_color(props);
    let bg = background_color(props);
    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;
    let width_u16 = rect.width.round().max(1.0) as u16;
    let height_u16 = rect.height.round().max(1.0) as u16;

    for y in 0..height_u16 {
        let matrix_row = ((y as f32 / height_u16 as f32) * rows as f32)
            .floor()
            .min((rows - 1) as f32) as usize;
        for x in 0..width_u16 {
            let matrix_col = ((x as f32 / width_u16 as f32) * cols as f32)
                .floor()
                .min((cols - 1) as f32) as usize;
            let t = normalized_value(props, matrix[matrix_row][matrix_col]);
            let ch = if t >= 0.66 {
                '#'
            } else if t >= 0.33 {
                '+'
            } else if t > 0.0 {
                '.'
            } else {
                ' '
            };
            buf.set(row_u16 + y, col_u16 + x, styled_cell(ch, fg, Some(bg)));
        }
    }
}

#[cfg(target_os = "macos")]
const MATRIX_FRAGMENT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float value = clamp(in.value_t, 0.0, 1.0);
    float control = in.uniform_a.x;
    float isClicked = in.uniform_a.y;
    float releaseTime = in.uniform_a.z;

    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (0.5 - uv.y) * 2.0);
    float2 halfCell = float2(aspect, 1.0);
    float cellDist = sdf_rounded_rect(p, halfCell, 0.0);
    float pix = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    float inCell = 1.0 - smoothstep(0.0, pix, cellDist);

    float scale = 1.0;
    if (isClicked > 0.5) {
        if (releaseTime <= 0.0) {
            scale = 1.05;
        } else {
            float elapsed = in.itime - releaseTime;
            if (elapsed < 0.2) {
                float t = clamp(elapsed / 0.2, 0.0, 1.0);
                float progress = 1.0 - pow(1.0 - t, 3.0);
                scale = mix(1.05, 1.0, progress);
            }
        }
    }

    float radius = min(aspect, 1.0) * 0.80 * scale;
    float ringThickness = radius * 0.12;
    float d = length(p);

    float ringMask = 0.0;
    float innerMask = 0.0;
    if (control > 1.5) {
        float fillAlpha = clamp(in.color_a.a * value, 0.0, 1.0);
        float bgAlpha = clamp(in.color_b.a, 0.0, 1.0);
        float outAlpha = fillAlpha + bgAlpha * (1.0 - fillAlpha);
        if (outAlpha <= 0.0) {
            return float4(0.0);
        }
        float3 outColor = (
            in.color_a.rgb * fillAlpha +
            in.color_b.rgb * bgAlpha * (1.0 - fillAlpha)
        ) / outAlpha;
        return float4(outColor, outAlpha);
    } else if (control < 0.5) {
        float ringDist = d - radius;
        ringMask = smoothstep(pix, 0.0, ringDist) * inCell;

        float innerRadius = radius * value;
        float innerDist = innerRadius - d;
        innerMask = smoothstep(-pix, 0.0, innerDist) * inCell;
    } else {
        float squareHalfSize = radius;
        float squareDist = sdf_rounded_rect(p, float2(squareHalfSize), squareHalfSize * 0.15);
        float outlineDist = abs(squareDist) - ringThickness * 0.5;
        ringMask = smoothstep(pix, 0.0, outlineDist) * inCell;

        float lineThickness = squareHalfSize * 0.15;
        float lineWidth = squareHalfSize * 1.6;
        float linePosY = mix(-squareHalfSize * 0.8, squareHalfSize * 0.8, value);
        float yDist = abs(p.y - linePosY) - lineThickness * 0.5;
        float xDist = abs(p.x) - lineWidth * 0.5;
        float lineDist = max(xDist, yDist);
        innerMask = smoothstep(-pix, 0.0, -lineDist) * inCell;
    }

    float3 color = in.color_b.rgb;
    float3 borderColor = in.color_c.rgb;
    color = mix(color, borderColor, ringMask);
    color = mix(color, in.color_a.rgb, innerMask);
    return float4(color, in.color_b.a * inCell);
}
"#;

impl WidgetDefinition for MatrixWidget {
    fn names(&self) -> &'static [&'static str] {
        &["matrix"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["rows", "cols", "columns", "width", "height", "size"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["value"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let rows = matrix_rows(node) as f32;
        let cols = matrix_cols(node) as f32;
        let default_size = get_prop_num(node, "size").map(f64_to_f32).unwrap_or(16.0);
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(default_size);
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or_else(|| (width * rows / cols) / constraints.aspect.max(f32::EPSILON));
        Some(Size {
            width: width.max(1.0),
            height: height.max(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Moved => {
                let cell = cell_index(node, local_col, local_row);
                update_state(node.widget_id, |state| state.hovered_cell = Some(cell));
                MouseEventOutcome::Consume
            }
            MouseEventKind::Down(MouseButton::Left)
                if get_bool_prop(&node.props, "toggle", false) =>
            {
                handle_toggle(node, local_col, local_row)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let rows = matrix_rows_from_props(&node.props);
                let cols = matrix_cols_from_props(&node.props);
                let cell = cell_index(node, local_col, local_row);
                let matrix = parse_matrix_value(&node.props, rows, cols);
                update_state(node.widget_id, |state| {
                    state.active_cell = Some(cell);
                    state.clicked_cell = Some(cell);
                    state.release_time = 0.0;
                    state.drag_start_row = local_row;
                    state.drag_start_value = matrix[cell / cols][cell % cols];
                });
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left)
                if get_bool_prop(&node.props, "toggle", false) =>
            {
                let cell = cell_index(node, local_col, local_row);
                update_state(node.widget_id, |state| state.hovered_cell = Some(cell));
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left)
                if !get_bool_prop(&node.props, "toggle", false) =>
            {
                handle_drag(node, local_row)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                update_state(node.widget_id, |state| {
                    state.active_cell = None;
                    state.release_time =
                        crate::widget_render::sdf_widget::current_sdf_time_fallback_seconds();
                });
                MouseEventOutcome::Consume
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        // Per-cell path: `dispatch_cell_change` packed a `(row col value)` list and
        // the widget declares `on-cell-change`. Spread it into three positional args.
        if has_callback(node, "on-cell-change") {
            let callback = node.props.get("on-cell-change")?.clone();
            let args = match &value {
                Value::List(items) => items.iter().map(|cell| cell.borrow().clone()).collect(),
                other => vec![other.clone()],
            };
            return Some(EventOutput { callback, args });
        }
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![value],
        })
    }

    fn wants_animation_frames(&self, node: &LayoutNode) -> bool {
        state_animation_active(
            get_state(node.widget_id),
            crate::widget_render::sdf_widget::current_sdf_time_fallback_seconds(),
        )
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(MATRIX_FRAGMENT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let rows = matrix_rows_from_props(&node.props);
        let cols = matrix_cols_from_props(&node.props);
        let matrix = parse_matrix_value(&node.props, rows, cols);
        let state = get_state(node.widget_id);
        let control = control_from_props(&node.props);
        let color = fill_color(&node.props);
        let bg = background_color(&node.props);
        let border = resolve_named_color(&node.props, "border-color", default_cell_color(bg));
        let hover_border = resolve_named_color(
            &node.props,
            "hover-border-color",
            default_hover_cell_color(bg),
        );
        let cell_w = node.rect.width / cols as f32;
        let cell_h = node.rect.height / rows as f32;
        let mut prims = Vec::with_capacity(rows * cols);

        for (row, row_values) in matrix.iter().enumerate() {
            for (col, value) in row_values.iter().enumerate() {
                let idx = row * cols + col;
                let is_clicked = state.clicked_cell == Some(idx);
                let cell_col = node.rect.col + col as f32 * cell_w;
                let cell_row = node.rect.row + row as f32 * cell_h;
                let cell_border = if state.hovered_cell == Some(idx) {
                    hover_border
                } else {
                    border
                };
                let t = normalized_value(&node.props, *value);
                let rect = Rect {
                    row: cell_row,
                    col: cell_col,
                    width: cell_w,
                    height: cell_h,
                };
                let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
                let px_w = cell_w * viewport.cell_w;
                let px_h = cell_h * viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: widget_type.to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: t,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [
                            match control {
                                MatrixControl::Circle => 0.0,
                                MatrixControl::Line => 1.0,
                                MatrixControl::Grid => 2.0,
                            },
                            if is_clicked { 1.0 } else { 0.0 },
                            state.release_time,
                            0.0,
                        ],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: color.to_rgba(),
                        color_b: bg.to_rgba(),
                        color_c: cell_border.to_rgba(),
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }
        }

        if state.clicked_cell.is_some()
            && state.release_time > 0.0
            && viewport.time_seconds - state.release_time >= CLICK_ANIMATION_SECONDS + 0.05
        {
            update_state(node.widget_id, |state| {
                state.clicked_cell = None;
                state.release_time = 0.0;
            });
        }

        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(values: Vec<Value>) -> Value {
        Value::List(values.into_iter().map(value_cell).collect())
    }

    fn matrix_node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 42,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "matrix".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 8.0,
                height: 8.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
        }
    }

    #[test]
    fn value_is_bindable_but_not_size_affecting() {
        assert_eq!(MATRIX_WIDGET.bindable_props(), &["value"]);
        assert!(!MATRIX_WIDGET.size_affecting_props().contains(&"value"));
    }

    #[test]
    fn constructor_accepts_reactive_value_binding() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let widget = crate::widgets::build_widget(
            "matrix",
            vec![
                Value::Keyword("value".to_string()),
                Value::ReactiveRef {
                    namespace: "APP".to_string(),
                    field: "weight".to_string(),
                    index: None,
                    kind: crate::vm::BindingKind::Float,
                    slot: slots.slot("APP", "weight"),
                },
            ],
        );

        let Value::Map(map) = widget else {
            panic!("expected matrix widget map");
        };
        assert!(!map.contains_key("__widget-diagnostic"));
    }

    #[test]
    fn flat_values_are_reshaped_row_major() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(2.0));
        props.insert("cols".to_string(), Value::Number(3.0));
        props.insert(
            "value".to_string(),
            list(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
                Value::Number(5.0),
                Value::Number(6.0),
            ]),
        );

        assert_eq!(
            parse_matrix_value(&props, 2, 3),
            vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]
        );
    }

    #[test]
    fn control_prop_accepts_grid_mode_aliases() {
        let mut props = HashMap::new();
        props.insert("control".to_string(), Value::Keyword("grid".to_string()));
        assert_eq!(control_from_props(&props), MatrixControl::Grid);

        props.insert("control".to_string(), Value::String("squares".to_string()));
        assert_eq!(control_from_props(&props), MatrixControl::Grid);
    }

    #[test]
    fn background_and_fill_color_props_accept_aliases() {
        let mut props = HashMap::new();
        props.insert(
            "background".to_string(),
            Value::Keyword("transparent".to_string()),
        );
        props.insert(
            "fill-color".to_string(),
            Value::String("#33669980".to_string()),
        );

        assert_eq!(background_color(&props), Color::rgba(0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            fill_color(&props),
            Color::rgba(
                0x33 as f32 / 255.0,
                0x66 as f32 / 255.0,
                0x99 as f32 / 255.0,
                0x80 as f32 / 255.0
            )
        );
    }

    #[test]
    fn drag_keeps_initial_cell_and_emits_nested_matrix() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(2.0));
        props.insert("cols".to_string(), Value::Number(2.0));
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(1.0));
        props.insert(
            "value".to_string(),
            list(vec![
                list(vec![Value::Number(0.0), Value::Number(0.25)]),
                list(vec![Value::Number(0.5), Value::Number(0.75)]),
            ]),
        );

        let node = matrix_node(props);
        let down = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            5.0,
            1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        assert!(matches!(down, MouseEventOutcome::Consume));

        let drag = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            5.0,
            -1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::List(rows))) = drag else {
            panic!("expected nested matrix dispatch");
        };
        let Value::List(first_row) = &*rows[0].borrow() else {
            panic!("expected first row");
        };
        assert_eq!(*first_row[0].borrow(), Value::Number(0.0));
        assert_eq!(*first_row[1].borrow(), Value::Number(0.75));
    }

    #[test]
    fn toggle_mode_click_flips_cell_between_min_and_max() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(2.0));
        props.insert("cols".to_string(), Value::Number(2.0));
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(1.0));
        props.insert("toggle".to_string(), Value::Bool(true));
        props.insert(
            "value".to_string(),
            list(vec![
                list(vec![Value::Number(0.0), Value::Number(0.25)]),
                list(vec![Value::Number(0.5), Value::Number(0.75)]),
            ]),
        );

        let node = matrix_node(props);
        let click = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            1.0,
            1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::List(rows))) = click else {
            panic!("expected nested matrix dispatch");
        };
        let Value::List(first_row) = &*rows[0].borrow() else {
            panic!("expected first row");
        };
        assert_eq!(*first_row[0].borrow(), Value::Number(1.0));
        assert_eq!(*first_row[1].borrow(), Value::Number(0.25));
    }

    #[test]
    fn toggle_mode_drag_does_not_dispatch_continuous_edits() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(2.0));
        props.insert("cols".to_string(), Value::Number(2.0));
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(1.0));
        props.insert("toggle".to_string(), Value::Bool(true));
        props.insert(
            "value".to_string(),
            list(vec![
                list(vec![Value::Number(0.0), Value::Number(0.0)]),
                list(vec![Value::Number(0.0), Value::Number(0.0)]),
            ]),
        );

        let node = matrix_node(props);
        let click = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            1.0,
            1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        assert!(matches!(
            click,
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(_))
        ));

        let drag = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            5.0,
            1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        assert!(matches!(drag, MouseEventOutcome::Consume));
    }

    #[test]
    fn on_cell_change_dispatches_only_the_edited_cell() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(2.0));
        props.insert("cols".to_string(), Value::Number(2.0));
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(1.0));
        props.insert(
            "on-cell-change".to_string(),
            Value::String("cb".to_string()),
        );
        props.insert(
            "value".to_string(),
            list(vec![
                list(vec![Value::Number(0.0), Value::Number(0.25)]),
                list(vec![Value::Number(0.5), Value::Number(0.75)]),
            ]),
        );

        let node = matrix_node(props);
        // Anchor on cell (row 0, col 1) whose current value is 0.25.
        let _ = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            5.0,
            1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        let drag = MATRIX_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            5.0,
            -1.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(payload)) = drag else {
            panic!("expected per-cell dispatch");
        };
        {
            let Value::List(cell) = &payload else {
                panic!("expected a single (row col value) cell, not a full matrix");
            };
            assert_eq!(cell.len(), 3, "row, col, value");
            assert_eq!(*cell[0].borrow(), Value::Number(0.0));
            assert_eq!(*cell[1].borrow(), Value::Number(1.0));
            assert_eq!(*cell[2].borrow(), Value::Number(0.75));
        }
        // handle_event spreads the cell into three positional args on on-cell-change.
        let output = MATRIX_WIDGET
            .handle_event(&node, WidgetEvent::Custom(payload))
            .expect("cell-change output");
        assert_eq!(output.callback, Value::String("cb".to_string()));
        assert_eq!(output.args.len(), 3);
        assert_eq!(output.args[0], Value::Number(0.0));
        assert_eq!(output.args[1], Value::Number(1.0));
        assert_eq!(output.args[2], Value::Number(0.75));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn metal_primitives_use_shader_instances_for_antialiased_cells() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(2.0));
        props.insert("cols".to_string(), Value::Number(2.0));
        props.insert(
            "value".to_string(),
            list(vec![
                list(vec![Value::Number(0.0), Value::Number(0.25)]),
                list(vec![Value::Number(0.5), Value::Number(1.0)]),
            ]),
        );
        let node = matrix_node(props);
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 640.0,
            vp_h: 360.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 18.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };

        let prims = MATRIX_WIDGET.build_metal_primitives("matrix", &node, viewport);
        assert_eq!(prims.len(), 4);
        assert!(prims.iter().all(|prim| {
            matches!(
                prim,
                MetalPrimitive::WidgetInstance { widget_type, .. } if widget_type == "matrix"
            )
        }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn grid_control_sets_square_alpha_shader_mode() {
        let mut props = HashMap::new();
        props.insert("rows".to_string(), Value::Number(1.0));
        props.insert("cols".to_string(), Value::Number(1.0));
        props.insert("control".to_string(), Value::Keyword("grid".to_string()));
        props.insert(
            "background".to_string(),
            Value::Keyword("transparent".to_string()),
        );
        props.insert("fill".to_string(), Value::String("#ff000080".to_string()));
        props.insert(
            "value".to_string(),
            list(vec![list(vec![Value::Number(0.5)])]),
        );
        let node = matrix_node(props);
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 640.0,
            vp_h: 360.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 18.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };

        let prims = MATRIX_WIDGET.build_metal_primitives("matrix", &node, viewport);
        let [MetalPrimitive::WidgetInstance { instance, .. }] = prims.as_slice() else {
            panic!("expected one widget instance");
        };
        assert_eq!(instance.value_t, 0.5);
        assert_eq!(instance.uniform_a[0], 2.0);
        assert_eq!(instance.color_b, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(instance.color_a, [1.0, 0.0, 0.0, 0x80 as f32 / 255.0]);
    }
}
