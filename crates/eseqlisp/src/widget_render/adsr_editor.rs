use std::collections::HashMap;
use std::{cell::RefCell, rc::Rc};

use super::{
    CellBuffer, EventOutput, GpuPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    WidgetInstance, WidgetViewport, ndc_bounds, resolve_named_color, styled_cell,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

pub struct AdsrEditorWidget;

pub static ADSR_EDITOR_WIDGET: AdsrEditorWidget = AdsrEditorWidget;

const ATTACK_MAX_DEFAULT: f32 = 5_000.0;
const DECAY_MAX_DEFAULT: f32 = 5_000.0;
const RELEASE_MAX_DEFAULT: f32 = 10_000.0;
const RELEASE_START: f32 = 0.68;
const ATTACK_ORIGIN: f32 = 0.03;
const ENVELOPE_Y_INSET: f32 = 0.08;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AdsrInteractionState {
    hovered_handle: Option<i32>,
    active_handle: Option<i32>,
    last_drag_envelope: Option<AdsrEnvelope>,
    last_drag_decay: Option<DecayEnvelope>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdsrEnvelope {
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
}

impl AdsrEnvelope {
    fn from_node(node: &LayoutNode) -> Self {
        Self {
            attack: prop_ms(&node.props, "attack", 4.0),
            decay: prop_ms(&node.props, "decay", 400.0),
            sustain: prop_unit(&node.props, "sustain", 0.5),
            release: prop_ms(&node.props, "release", 300.0),
        }
    }

    fn into_value(self, active: Option<&str>) -> Value {
        env_map(self.attack, self.decay, self.sustain, self.release, active)
    }
}

/// Bipolar initial value decaying to zero over a finite time. This mode
/// shares the ADSR editor's geometry, handles and gesture lifecycle.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DecayEnvelope {
    initial: f32,
    time: f32,
}

fn decay_mode(node: &LayoutNode) -> bool {
    matches!(node.props.get("mode"), Some(Value::Keyword(mode)) if mode == "decay")
}

fn initial_range(node: &LayoutNode) -> (f32, f32) {
    let min = super::get_f32_prop(&node.props, "initial-min", -1.0).min(0.0);
    let max = super::get_f32_prop(&node.props, "initial-max", 1.0).max(0.0).max(min + f32::EPSILON);
    (min, max)
}

impl DecayEnvelope {
    fn from_node(node: &LayoutNode) -> Self {
        let (min, max) = initial_range(node);
        Self { initial: super::get_f32_prop(&node.props, "initial", 0.0).clamp(min, max),
            time: prop_ms(&node.props, "time", 500.0) }
    }
    fn into_value(self, active: Option<&str>) -> Value {
        Value::Map(HashMap::from([
            ("initial".into(), Rc::new(RefCell::new(Value::Number(self.initial as f64)))),
            ("time".into(), Rc::new(RefCell::new(Value::Number(self.time as f64)))),
            ("active".into(), Rc::new(RefCell::new(active.map(|name| Value::Keyword(name.into())).unwrap_or(Value::Bool(false))))),
        ]))
    }
}

fn decay_time_norm(node: &LayoutNode, time: f32) -> f32 {
    let max = super::get_f32_prop(&node.props, "time-max", DECAY_MAX_DEFAULT).max(1.0);
    (adsr_log_weight(time) / adsr_log_weight(max)).clamp(0.0, 1.0)
}

fn decay_handle(node: &LayoutNode, handle: i32) -> Option<(f32, f32)> {
    let envelope = DecayEnvelope::from_node(node);
    let (min, max) = initial_range(node);
    let point = match handle {
        1 => (ATTACK_ORIGIN, (envelope.initial - min) / (max - min)),
        2 => (ATTACK_ORIGIN + (1.0 - ATTACK_ORIGIN) * decay_time_norm(node, envelope.time), -min / (max - min)),
        _ => return None,
    };
    Some(plot_point(point.0, point.1, node.rect))
}

fn decay_for_drag(node: &LayoutNode, handle: i32, col: f32, row: f32) -> DecayEnvelope {
    let mut envelope = DecayEnvelope::from_node(node);
    let (x, y) = data_from_local(node.rect, col, row);
    if handle == 1 {
        let (min, max) = initial_range(node);
        envelope.initial = min + y * (max - min);
    } else if handle == 2 {
        let max = super::get_f32_prop(&node.props, "time-max", DECAY_MAX_DEFAULT).max(1.0);
        envelope.time = adsr_ms_from_norm((x - ATTACK_ORIGIN) / (1.0 - ATTACK_ORIGIN), max);
    }
    envelope
}

thread_local! {
    static ADSR_STATES: RefCell<HashMap<u64, AdsrInteractionState>> = RefCell::new(HashMap::new());
}

fn interaction_state(widget_id: u64) -> AdsrInteractionState {
    ADSR_STATES.with(|states| states.borrow().get(&widget_id).copied().unwrap_or_default())
}

fn update_interaction_state(widget_id: u64, update: impl FnOnce(&mut AdsrInteractionState)) {
    ADSR_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.entry(widget_id).or_default();
        let before = *state;
        update(state);
        if *state != before {
            // Own-widget-only state: the drag envelope and handle highlight
            // never affect another widget's primitives, so a per-event
            // global generation bump would only defeat every other widget's
            // primitive cache for the whole gesture (eseq-eeng).
            super::bump_widget_state_revision(widget_id);
        }
    });
}

fn visual_envelope(node: &LayoutNode) -> AdsrEnvelope {
    interaction_state(node.widget_id)
        .last_drag_envelope
        .unwrap_or_else(|| AdsrEnvelope::from_node(node))
}

/// Test support: the local-cell center of a drag handle (1=attack, 2=decay,
/// 3=sustain, 4=release), so harnesses can aim real pointer events at a
/// handle. Mirrors the plot points `nearest_handle` hit-tests against.
pub fn adsr_handle_center(node: &LayoutNode, handle_idx: i32) -> Option<(f32, f32)> {
    if decay_mode(node) { return decay_handle(node, handle_idx); }
    let (x1, x2, x3, x4) = adsr_x_positions(&node.props);
    let sustain = prop_unit(&node.props, "sustain", 0.5);
    let (data_x, data_y) = match handle_idx {
        1 => (x1, 1.0),
        2 => (x2, sustain),
        3 => (x3, sustain),
        4 => (x4, 0.0),
        _ => return None,
    };
    Some(plot_point(data_x, data_y, node.rect))
}

/// Test support: the envelope the renderer would draw for this node right
/// now — live drag state first, layout props otherwise. Interaction state is
/// thread-local, so this only observes drags dispatched on the same thread.
pub fn adsr_visual_envelope(node: &LayoutNode) -> (f32, f32, f32, f32) {
    let envelope = visual_envelope(node);
    (
        envelope.attack,
        envelope.decay,
        envelope.sustain,
        envelope.release,
    )
}

fn clamp_measured_axis(requested: f32, min: f32, max: f32) -> f32 {
    if max.is_finite() {
        let upper = max.max(0.0);
        let lower = min.min(upper);
        requested.clamp(lower, upper)
    } else {
        requested.max(min)
    }
}

fn prop_ms(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    super::get_f32_prop(props, key, default)
        .clamp(0.0, 120_000.0)
        .max(0.0)
}

fn prop_unit(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    super::get_f32_prop(props, key, default).clamp(0.0, 1.0)
}

fn adsr_log_weight(ms: f32) -> f32 {
    (1.0 + (ms / 20.0)).ln().max(0.0)
}

fn adsr_ms_from_norm(norm: f32, max_ms: f32) -> f32 {
    let max_weight = adsr_log_weight(max_ms);
    20.0 * ((norm.clamp(0.0, 1.0) * max_weight).exp() - 1.0)
}

fn x_positions_from_norms(
    attack_norm: f32,
    decay_norm: f32,
    release_norm: f32,
) -> (f32, f32, f32, f32) {
    let attack_end = RELEASE_START * 0.42;
    let x1 = ATTACK_ORIGIN + (attack_end - ATTACK_ORIGIN) * attack_norm;
    let x2 = x1 + (RELEASE_START - x1) * decay_norm;
    let x3 = RELEASE_START;
    let x4 = x3 + (1.0 - x3) * release_norm;
    (x1, x2, x3, x4)
}

fn adsr_norms(props: &HashMap<String, Value>) -> (f32, f32, f32) {
    let attack = prop_ms(props, "attack", 4.0);
    let decay = prop_ms(props, "decay", 400.0);
    let release = prop_ms(props, "release", 300.0);
    let attack_max = super::get_f32_prop(props, "attack-max", ATTACK_MAX_DEFAULT).max(1.0);
    let decay_max = super::get_f32_prop(props, "decay-max", DECAY_MAX_DEFAULT).max(1.0);
    let release_max = super::get_f32_prop(props, "release-max", RELEASE_MAX_DEFAULT).max(1.0);
    let attack_norm = (adsr_log_weight(attack) / adsr_log_weight(attack_max)).clamp(0.0, 1.0);
    let decay_norm = (adsr_log_weight(decay) / adsr_log_weight(decay_max)).clamp(0.0, 1.0);
    let release_norm = (adsr_log_weight(release) / adsr_log_weight(release_max)).clamp(0.0, 1.0);
    (attack_norm, decay_norm, release_norm)
}

fn adsr_x_positions(props: &HashMap<String, Value>) -> (f32, f32, f32, f32) {
    let (attack_norm, decay_norm, release_norm) = adsr_norms(props);
    x_positions_from_norms(attack_norm, decay_norm, release_norm)
}

fn plot_point(data_x: f32, data_y: f32, rect: Rect) -> (f32, f32) {
    let pad_x = 0.055;
    let pad_y = 0.12;
    let envelope_y = ENVELOPE_Y_INSET + data_y * (1.0 - ENVELOPE_Y_INSET * 2.0);
    (
        rect.col + (pad_x + data_x * (1.0 - pad_x * 2.0)) * rect.width,
        rect.row + (pad_y + (1.0 - envelope_y) * (1.0 - pad_y * 2.0)) * rect.height,
    )
}

fn data_from_local(rect: Rect, col: f32, row: f32) -> (f32, f32) {
    let pad_x = 0.055;
    let pad_y = 0.12;
    let u = ((col - rect.col) / rect.width.max(0.0001)).clamp(0.0, 1.0);
    let v = ((row - rect.row) / rect.height.max(0.0001)).clamp(0.0, 1.0);
    let plot_y = (1.0 - ((v - pad_y) / (1.0 - pad_y * 2.0))).clamp(0.0, 1.0);
    let envelope_y = ((plot_y - ENVELOPE_Y_INSET) / (1.0 - ENVELOPE_Y_INSET * 2.0)).clamp(0.0, 1.0);
    (
        ((u - pad_x) / (1.0 - pad_x * 2.0)).clamp(0.0, 1.0),
        envelope_y,
    )
}

fn nearest_handle(node: &LayoutNode, col: f32, row: f32) -> i32 {
    let points: Vec<_> = (1..=4).filter_map(|i| adsr_handle_center(node, i).map(|point| (i, point))).collect();
    let mut best_idx = -1;
    let mut best_dist = f32::MAX;
    for (idx, (px, py)) in points {
        let dist = ((px - col).powi(2) + (py - row).powi(2)).sqrt();
        if dist < best_dist {
            best_dist = dist;
            best_idx = idx;
        }
    }
    if best_dist <= 1.0 { best_idx } else { -1 }
}

fn env_map(attack: f32, decay: f32, sustain: f32, release: f32, active: Option<&str>) -> Value {
    let mut values = HashMap::from([
        (
            "attack".to_string(),
            Rc::new(RefCell::new(Value::Number(attack as f64))),
        ),
        (
            "decay".to_string(),
            Rc::new(RefCell::new(Value::Number(decay as f64))),
        ),
        (
            "sustain".to_string(),
            Rc::new(RefCell::new(Value::Number(sustain as f64))),
        ),
        (
            "release".to_string(),
            Rc::new(RefCell::new(Value::Number(release as f64))),
        ),
    ]);
    values.insert(
        "active".to_string(),
        Rc::new(RefCell::new(match active {
            Some(name) => Value::Keyword(name.to_string()),
            None => Value::Bool(false),
        })),
    );
    Value::Map(values)
}

fn solve_norm_for_x(target_x: f32, mut x_for_norm: impl FnMut(f32) -> f32) -> f32 {
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if x_for_norm(mid) < target_x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5
}

fn envelope_for_drag(node: &LayoutNode, handle_idx: i32, col: f32, row: f32) -> AdsrEnvelope {
    let attack_max = super::get_f32_prop(&node.props, "attack-max", ATTACK_MAX_DEFAULT).max(1.0);
    let decay_max = super::get_f32_prop(&node.props, "decay-max", DECAY_MAX_DEFAULT).max(1.0);
    let release_max = super::get_f32_prop(&node.props, "release-max", RELEASE_MAX_DEFAULT).max(1.0);
    let (data_x, data_y) = data_from_local(node.rect, col, row);
    let (attack_norm, decay_norm, release_norm) = adsr_norms(&node.props);
    let (_x1, _x2, x3, _x4) = x_positions_from_norms(attack_norm, decay_norm, release_norm);

    let mut attack = prop_ms(&node.props, "attack", 4.0);
    let mut decay = prop_ms(&node.props, "decay", 400.0);
    let mut sustain = prop_unit(&node.props, "sustain", 0.5);
    let mut release = prop_ms(&node.props, "release", 300.0);

    match handle_idx {
        1 => {
            let norm = solve_norm_for_x(data_x, |candidate| {
                x_positions_from_norms(candidate, decay_norm, release_norm).0
            });
            attack = adsr_ms_from_norm(norm, attack_max);
        }
        2 => {
            let norm = solve_norm_for_x(data_x, |candidate| {
                x_positions_from_norms(attack_norm, candidate, release_norm).1
            });
            decay = adsr_ms_from_norm(norm, decay_max);
            sustain = data_y;
        }
        3 => {
            sustain = data_y;
        }
        4 => {
            let norm = (data_x - x3) / (1.0 - x3);
            release = adsr_ms_from_norm(norm, release_max);
        }
        _ => {}
    }

    AdsrEnvelope {
        attack,
        decay,
        sustain,
        release,
    }
}

impl WidgetDefinition for AdsrEditorWidget {
    fn names(&self) -> &'static [&'static str] {
        &["adsr-editor"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["attack", "decay", "sustain", "release", "initial", "time"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let requested_width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width);
        let requested_height = get_prop_num(node, "height").map(f64_to_f32).unwrap_or(5.0);
        let width = clamp_measured_axis(requested_width, 4.0, constraints.max_width);
        let height = clamp_measured_axis(requested_height, 2.0, constraints.max_height);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let attack = prop_ms(props, "attack", 4.0);
        let decay = prop_ms(props, "decay", 400.0);
        let sustain = prop_unit(props, "sustain", 0.5);
        let release = prop_ms(props, "release", 300.0);
        let label = if matches!(props.get("mode"), Some(Value::Keyword(mode)) if mode == "decay") {
            format!("Initial {:.2} Time {:.0}ms", super::get_f32_prop(props, "initial", 0.0), prop_ms(props, "time", 500.0))
        } else {
            format!("A{attack:.0} D{decay:.0} S{sustain:.2} R{release:.0}")
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in label.chars().enumerate() {
            if i >= rect.width.round() as usize {
                break;
            }
            buf.set(
                row,
                col_start + i as u16,
                styled_cell(ch, theme::FG_MUTED(), None),
            );
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        Some(Value::Number(
            nearest_handle(node, local_col, local_row) as f64
        ))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let handle_idx = match gesture {
            Some(Value::Number(n)) => *n as i32,
            _ => nearest_handle(node, local_col, local_row),
        };
        if decay_mode(node) {
            return match mouse_kind {
                MouseEventKind::Moved => {
                    update_interaction_state(node.widget_id, |state| state.hovered_handle = (handle_idx > 0).then_some(handle_idx));
                    MouseEventOutcome::Consume
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    update_interaction_state(node.widget_id, |state| {
                        state.active_handle = (handle_idx > 0).then_some(handle_idx);
                        state.last_drag_decay = None;
                    });
                    MouseEventOutcome::Consume
                }
                MouseEventKind::Drag(MouseButton::Left) if handle_idx > 0 => {
                    let envelope = decay_for_drag(node, handle_idx, local_col, local_row);
                    update_interaction_state(node.widget_id, |state| {
                        state.active_handle = Some(handle_idx);
                        state.last_drag_decay = Some(envelope);
                    });
                    MouseEventOutcome::Dispatch(WidgetEvent::Custom(envelope.into_value(Some(if handle_idx == 1 { "initial" } else { "time" }))))
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    let state = interaction_state(node.widget_id);
                    update_interaction_state(node.widget_id, |state| {
                        state.active_handle = None;
                        state.last_drag_decay = None;
                    });
                    if state.active_handle.is_some() {
                        MouseEventOutcome::Dispatch(WidgetEvent::Custom(state.last_drag_decay.unwrap_or_else(|| DecayEnvelope::from_node(node)).into_value(None)))
                    } else { MouseEventOutcome::Consume }
                }
                _ => MouseEventOutcome::Ignore,
            };
        }
        match mouse_kind {
            MouseEventKind::Moved => {
                let hovered = (handle_idx > 0).then_some(handle_idx);
                update_interaction_state(node.widget_id, |state| state.hovered_handle = hovered);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Down(MouseButton::Left) => {
                update_interaction_state(node.widget_id, |state| {
                    state.active_handle = (handle_idx > 0).then_some(handle_idx);
                    state.last_drag_envelope = None;
                });
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) if handle_idx > 0 => {
                let envelope = envelope_for_drag(node, handle_idx, local_col, local_row);
                update_interaction_state(node.widget_id, |state| {
                    state.hovered_handle = Some(handle_idx);
                    state.active_handle = Some(handle_idx);
                    state.last_drag_envelope = Some(envelope);
                });
                let active = match handle_idx {
                    1 => Some("attack"),
                    2 => Some("decay"),
                    3 => Some("sustain"),
                    4 => Some("release"),
                    _ => None,
                };
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(envelope.into_value(active)))
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let state = interaction_state(node.widget_id);
                update_interaction_state(node.widget_id, |state| {
                    state.active_handle = None;
                    state.last_drag_envelope = None;
                });
                if state.active_handle.is_some() {
                    let envelope = state
                        .last_drag_envelope
                        .unwrap_or_else(|| AdsrEnvelope::from_node(node));
                    MouseEventOutcome::Dispatch(WidgetEvent::Custom(envelope.into_value(None)))
                } else {
                    MouseEventOutcome::Consume
                }
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![value],
        })
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        ADSR_EDITOR_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let decay_display = interaction_state(node.widget_id).last_drag_decay.unwrap_or_else(|| DecayEnvelope::from_node(node));
        let (initial_min, initial_max) = initial_range(node);
        let envelope = visual_envelope(node);
        let attack = envelope.attack;
        let decay = envelope.decay;
        let sustain = envelope.sustain;
        let release = envelope.release;
        let hold = super::get_f32_prop(&node.props, "hold", 0.35).clamp(0.05, 2.0);
        let attack_max =
            super::get_f32_prop(&node.props, "attack-max", ATTACK_MAX_DEFAULT).max(1.0);
        let decay_max = super::get_f32_prop(&node.props, "decay-max", DECAY_MAX_DEFAULT).max(1.0);
        let release_max =
            super::get_f32_prop(&node.props, "release-max", RELEASE_MAX_DEFAULT).max(1.0);

        let curve_color = resolve_named_color(
            &node.props,
            "curve-color",
            crate::theme::ADSR_CURVE(),
        );
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            crate::theme::ADSR_BG(),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            crate::theme::ADSR_GRID(),
        );
        let point_color = resolve_named_color(
            &node.props,
            "point-color",
            crate::theme::ADSR_POINT(),
        );

        let state = interaction_state(node.widget_id);
        let hovered_handle = super::pointer_hovered(node.widget_id)
            .then_some(state.hovered_handle)
            .flatten();
        let visual_handle = state.active_handle.or(hovered_handle).unwrap_or(0) as f32;
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        vec![GpuPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: 0.0,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [attack, decay, sustain, release],
                uniform_b: [hold, visual_handle, super::ui_px_scale(), 0.0],
                uniform_c: [attack_max, decay_max, release_max, 0.0],
                uniform_d: [if decay_mode(node) { 1.0 } else { 0.0 },
                    (decay_display.initial - initial_min) / (initial_max - initial_min),
                    decay_time_norm(node, decay_display.time), -initial_min / (initial_max - initial_min)],
                color_a: curve_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: point_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

const ADSR_EDITOR_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
float adsr_sdSegment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

float adsr_logWeight(float ms) {
    return max(0.0, log(1.0 + (ms / 20.0)));
}

float2 adsr_toPlot(float2 data) {
    float2 pad = float2(0.055, 0.12);
    const float envelopeYInset = 0.08;
    float envelopeY = mix(envelopeYInset, 1.0 - envelopeYInset, data.y);
    return float2(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - envelopeY) * (1.0 - pad.y * 2.0));
}

float2 adsr_point(int idx, float x1, float x2, float x3, float x4, float sustain) {
    const float attackOrigin = 0.03;
    if (idx == 0) return adsr_toPlot(float2(attackOrigin, 0.0));
    if (idx == 1) return adsr_toPlot(float2(x1, 1.0));
    if (idx == 2) return adsr_toPlot(float2(x2, sustain));
    if (idx == 3) return adsr_toPlot(float2(x3, sustain));
    return adsr_toPlot(float2(x4, 0.0));
}

float adsr_expFall(float t, float start, float end) {
    const float k = 5.0;
    float normalized = (exp(-k * clamp(t, 0.0, 1.0)) - exp(-k)) / (1.0 - exp(-k));
    return end + (start - end) * normalized;
}

float adsr_curveY(float x, float x1, float x2, float x3, float x4, float sustain) {
    const float attackOrigin = 0.03;
    if (x < attackOrigin) return 0.0;
    if (x <= x1) {
        if (x1 <= attackOrigin + 1e-5) return 1.0;
        return clamp((x - attackOrigin) / max(x1 - attackOrigin, 1e-5), 0.0, 1.0);
    }
    if (x <= x2) return adsr_expFall((x - x1) / max(x2 - x1, 1e-5), 1.0, sustain);
    if (x <= x3) return sustain;
    if (x <= x4) return adsr_expFall((x - x3) / max(x4 - x3, 1e-5), sustain, 0.0);
    return 0.0;
}

float adsr_bracketDistance(float2 p, float2 corner, float2 inward, float2 lengthPx) {
    float horizontal = adsr_sdSegment(p, corner, corner + float2(inward.x * lengthPx.x, 0.0));
    float vertical = adsr_sdSegment(p, corner, corner + float2(0.0, inward.y * lengthPx.y));
    return min(horizontal, vertical);
}

float4 adsr_decayDisplay(WidgetVaryings in) {
    float2 uv = in.uv;
    float2 perPixel = max(float2(fwidth(uv.x), fwidth(uv.y)), float2(1e-6));
    float2 p = uv / perPixel;
    float initial = in.uniform_d.y;
    float zero = in.uniform_d.w;
    float end = mix(0.03, 1.0, in.uniform_d.z);
    float4 col = in.color_b;
    float baseline = abs(p.y - adsr_toPlot(float2(0.0, zero)).y / perPixel.y);
    col.rgb = mix(col.rgb, in.color_c.rgb, (1.0 - smoothstep(0.5, 1.5, baseline)) * in.color_c.a * 0.4);
    float distance = 10000.0;
    float2 previous = adsr_toPlot(float2(0.03, initial)) / perPixel;
    for (int i = 1; i <= 64; ++i) {
        float t = float(i) / 64.0;
        float2 current = adsr_toPlot(float2(mix(0.03, end, t), adsr_expFall(t, initial, zero))) / perPixel;
        distance = min(distance, adsr_sdSegment(p, previous, current));
        previous = current;
    }
    distance = min(distance, adsr_sdSegment(p, previous, adsr_toPlot(float2(1.0, zero)) / perPixel));
    col.rgb = mix(col.rgb, in.color_a.rgb, (1.0 - smoothstep(0.65, 1.55, distance)) * in.color_a.a);
    float scale = max(in.uniform_b.z, 0.001);
    for (int i = 1; i <= 2; ++i) {
        float2 h = adsr_toPlot(i == 1 ? float2(0.03, initial) : float2(end, zero)) / perPixel;
        bool active = abs(float(i) - in.uniform_b.y) < 0.5;
        float halfSize = (active ? 7.2 : 6.0) * scale;
        float d = max(abs(p.x - h.x), abs(p.y - h.y));
        float outer = 1.0 - smoothstep(halfSize, halfSize + 0.75, d);
        float inner = 1.0 - smoothstep(halfSize - 1.5 * scale, halfSize - 1.5 * scale + 0.75, d);
        col.rgb = mix(col.rgb, in.color_d.rgb, (active ? outer : max(outer - inner, 0.0)) * in.color_d.a);
    }
    return col;
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    if (in.uniform_d.x > 0.5) return adsr_decayDisplay(in);
    float attack = in.uniform_a.x;
    float decay = in.uniform_a.y;
    float sustain = clamp(in.uniform_a.z, 0.0, 1.0);
    float release = in.uniform_a.w;
    float attackNorm = clamp(adsr_logWeight(attack) / adsr_logWeight(in.uniform_c.x), 0.0, 1.0);
    float decayNorm = clamp(adsr_logWeight(decay) / adsr_logWeight(in.uniform_c.y), 0.0, 1.0);
    float releaseNorm = clamp(adsr_logWeight(release) / adsr_logWeight(in.uniform_c.z), 0.0, 1.0);
    const float releaseStart = 0.68;
    const float attackOrigin = 0.03;
    float attackEnd = releaseStart * 0.42;
    float x1 = mix(attackOrigin, attackEnd, attackNorm);
    float x2 = x1 + (releaseStart - x1) * decayNorm;
    float x3 = releaseStart;
    float x4 = mix(x3, 1.0, releaseNorm);

    float2 uv = in.uv;
    float4 col = in.color_b;

    float2 pad = float2(0.055, 0.12);
    float plotLeft = pad.x;
    float plotRight = 1.0 - pad.x;
    float plotTop = pad.y;
    float plotBottom = 1.0 - pad.y;
    float insidePlot = step(plotLeft, uv.x) * step(uv.x, plotRight)
        * step(plotTop, uv.y) * step(uv.y, plotBottom);

    float gridZero = adsr_toPlot(float2(0.0, 0.0)).y;
    float baselineWidth = max(fwidth(uv.y), 0.001);
    float baseline = 1.0 - smoothstep(0.0, baselineWidth, abs(uv.y - gridZero));
    col.rgb = mix(col.rgb, in.color_c.rgb, baseline * in.color_c.a * 0.25 * insidePlot);

    float2 uvPerPixel = max(float2(fwidth(uv.x), fwidth(uv.y)), float2(1e-6));
    float pxScale = (in.uniform_b.z > 0.0) ? in.uniform_b.z : 1.0;
    float2 pPx = uv / uvPerPixel;
    float2 plotMinPx = float2(plotLeft, plotTop) / uvPerPixel;
    float2 plotMaxPx = float2(plotRight, plotBottom) / uvPerPixel;
    float attackOriginPx = adsr_toPlot(float2(attackOrigin, 0.0)).x / uvPerPixel.x;
    float leftBracketWidthPx = attackOriginPx - plotMinPx.x;
    float envelopeTopPx = adsr_toPlot(float2(attackOrigin, 1.0)).y / uvPerPixel.y;
    float envelopeBottomPx = adsr_toPlot(float2(attackOrigin, 0.0)).y / uvPerPixel.y;
    float topBracketHeightPx = envelopeTopPx - plotMinPx.y;
    float bottomBracketHeightPx = plotMaxPx.y - envelopeBottomPx;
    float bracketDist = 1000.0;
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, plotMinPx, float2(1.0, 1.0), float2(leftBracketWidthPx, topBracketHeightPx)));
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, float2(plotMaxPx.x, plotMinPx.y), float2(-1.0, 1.0), float2(16.0 * pxScale, topBracketHeightPx)));
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, float2(plotMinPx.x, plotMaxPx.y), float2(1.0, -1.0), float2(leftBracketWidthPx, bottomBracketHeightPx)));
    bracketDist = min(bracketDist, adsr_bracketDistance(pPx, plotMaxPx, float2(-1.0, -1.0), float2(16.0 * pxScale, bottomBracketHeightPx)));
    float brackets = 1.0 - smoothstep(0.5, 1.25, bracketDist);
    col.rgb = mix(col.rgb, in.color_a.rgb, brackets * in.color_a.a);

    float dataX = clamp((uv.x - pad.x) / (1.0 - pad.x * 2.0), 0.0, 1.0);
    float curveDataY = adsr_curveY(dataX, x1, x2, x3, x4, sustain);
    float curvePlotY = adsr_toPlot(float2(dataX, curveDataY)).y;
    float fillRegion = step(attackOrigin, dataX) * step(dataX, x4)
        * step(curvePlotY, uv.y) * step(uv.y, gridZero) * insidePlot;
    float fillGradient = clamp((gridZero - uv.y) / max(gridZero - curvePlotY, 1e-5), 0.0, 1.0);
    col.rgb = mix(col.rgb, in.color_a.rgb, fillRegion * fillGradient * in.color_a.a * 0.10);

    float minDistPx = 1000.0;
    float2 previous = adsr_toPlot(float2(attackOrigin, 0.0)) / uvPerPixel;
    const int subdivisions = 8;
    const int segmentCount = 4;
    for (int segment = 0; segment < segmentCount; ++segment) {
        float segmentStart = (segment == 0) ? attackOrigin : ((segment == 1) ? x1 : ((segment == 2) ? x2 : x3));
        float segmentEnd = (segment == 0) ? x1 : ((segment == 1) ? x2 : ((segment == 2) ? x3 : x4));
        for (int stepIndex = 1; stepIndex <= subdivisions; ++stepIndex) {
            float segmentT = float(stepIndex) / float(subdivisions);
            float sampleX = mix(segmentStart, segmentEnd, segmentT);
            float sampleY = (segment == 0)
                ? segmentT
                : adsr_curveY(sampleX, x1, x2, x3, x4, sustain);
            float2 current = adsr_toPlot(float2(sampleX, sampleY)) / uvPerPixel;
            minDistPx = min(minDistPx, adsr_sdSegment(pPx, previous, current));
            previous = current;
        }
    }
    float activeHandle = round(in.uniform_b.y);
    float curve = 1.0 - smoothstep(0.65, 1.55, minDistPx);
    float curveBrightness = activeHandle > 0.5 ? 1.12 : 1.0;
    col.rgb = mix(col.rgb, min(in.color_a.rgb * curveBrightness, float3(1.0)), curve * in.color_a.a);

    float pixelY = max(fwidth(uv.y), 0.001);
    for (int i = 1; i < 5; ++i) {
        float2 h = adsr_point(i, x1, x2, x3, x4, sustain);
        bool highlighted = abs(float(i) - activeHandle) < 0.5;
        float handleHalfPx = (highlighted ? 7.2 : 6.0) * pxScale;
        float handleStrokePx = 1.5 * pxScale;
        float2 pxDelta = float2((uv.x - h.x) / uvPerPixel.x,
                                (uv.y - h.y) / pixelY);
        float2 d = abs(pxDelta);
        float2 outerQ = d - float2(handleHalfPx);
        float outerDist = length(max(outerQ, float2(0.0))) + min(max(outerQ.x, outerQ.y), 0.0);
        float2 innerQ = d - float2(max(handleHalfPx - handleStrokePx, 0.0));
        float innerDist = length(max(innerQ, float2(0.0))) + min(max(innerQ.x, innerQ.y), 0.0);
        float outer = 1.0 - smoothstep(0.0, 0.75, outerDist);
        float inner = 1.0 - smoothstep(0.0, 0.75, innerDist);
        float square = highlighted ? outer : max(outer - inner, 0.0);
        col.rgb = mix(col.rgb, in.color_d.rgb, square * in.color_d.a);
    }

    return col;
}
"#, super::wgsl::ADSR_EDITOR_SHADER);

#[cfg(test)]
mod tests {
    use super::*;

    fn test_node(widget_id: u64) -> LayoutNode {
        LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "adsr-editor".to_string(),
            rect: Rect {
                col: 0.0,
                row: 0.0,
                width: 40.0,
                height: 8.0,
            },
            props: HashMap::from([
                ("attack".to_string(), Value::Number(10.0)),
                ("decay".to_string(), Value::Number(200.0)),
                ("sustain".to_string(), Value::Number(0.5)),
                ("release".to_string(), Value::Number(300.0)),
                (
                    "on-change".to_string(),
                    Value::Symbol("handler".to_string()),
                ),
            ]),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        }
    }

    fn map_value(value: &Value, key: &str) -> Value {
        let Value::Map(map) = value else {
            panic!("expected map");
        };
        map.get(key).expect("map entry").borrow().clone()
    }

    #[test]
    fn bipolar_decay_handles_edit_independently_and_commit_on_release() {
        let mut node = test_node(987654);
        node.props.extend([
            ("mode".into(), Value::Keyword("decay".into())),
            ("initial".into(), Value::Number(-24.0)),
            ("time".into(), Value::Number(500.0)),
            ("initial-min".into(), Value::Number(-48.0)),
            ("initial-max".into(), Value::Number(48.0)),
            ("time-max".into(), Value::Number(15000.0)),
        ]);
        assert!(adsr_handle_center(&node, 3).is_none());
        for (handle, x, y, key, expected) in [(1, 0.03, 0.75, "initial", 24.0), (2, 1.0, 0.5, "time", 15000.0)] {
            let center = adsr_handle_center(&node, handle).unwrap();
            let gesture = Value::Number(handle as f64);
            ADSR_EDITOR_WIDGET.mouse_event(&node, MouseEventKind::Down(MouseButton::Left), center.0, center.1, None, Some(&gesture), KeyModifiers::empty(), 10.0, 20.0);
            let target = plot_point(x, y, node.rect);
            let MouseEventOutcome::Dispatch(WidgetEvent::Custom(drag)) = ADSR_EDITOR_WIDGET.mouse_event(&node, MouseEventKind::Drag(MouseButton::Left), target.0, target.1, None, Some(&gesture), KeyModifiers::empty(), 10.0, 20.0) else { panic!("decay drag"); };
            let Value::Number(value) = map_value(&drag, key) else { panic!("numeric edit"); };
            assert!((value - expected).abs() < 0.01);
            assert_eq!(map_value(&drag, "active"), Value::Keyword(key.into()));
            assert_eq!(map_value(&drag, if handle == 1 { "time" } else { "initial" }), Value::Number(if handle == 1 { 500.0 } else { -24.0 }));
            let MouseEventOutcome::Dispatch(WidgetEvent::Custom(commit)) = ADSR_EDITOR_WIDGET.mouse_event(&node, MouseEventKind::Up(MouseButton::Left), target.0, target.1, None, Some(&gesture), KeyModifiers::empty(), 10.0, 20.0) else { panic!("decay commit"); };
            assert_eq!(map_value(&commit, "active"), Value::Bool(false));
            assert_eq!(map_value(&commit, key), map_value(&drag, key));
            assert!(interaction_state(node.widget_id).last_drag_decay.is_none());
        }
    }

    #[test]
    fn release_anchor_is_fixed_independent_of_attack_and_decay() {
        for (attack, decay) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let (_, _, release_start, _) = x_positions_from_norms(attack, decay, 0.5);
            assert!((release_start - RELEASE_START).abs() < 1.0e-6);
        }
    }

    #[test]
    fn zero_attack_lands_at_the_inset_envelope_origin() {
        assert_eq!(adsr_log_weight(0.0), 0.0);
        let (attack, _, _, _) = x_positions_from_norms(0.0, 0.5, 0.5);
        assert!((attack - ATTACK_ORIGIN).abs() < 1.0e-6);
        assert!(
            ATTACK_ORIGIN > 0.0,
            "attack origin must sit inside the left bracket"
        );
    }

    #[test]
    fn envelope_amplitude_range_is_inset_from_the_outer_brackets() {
        let rect = Rect {
            row: 2.0,
            col: 3.0,
            width: 40.0,
            height: 12.0,
        };
        let (_, top) = plot_point(ATTACK_ORIGIN, 1.0, rect);
        let (_, bottom) = plot_point(ATTACK_ORIGIN, 0.0, rect);
        let outer_top = rect.row + 0.12 * rect.height;
        let outer_bottom = rect.row + (1.0 - 0.12) * rect.height;
        assert!(top > outer_top);
        assert!(bottom < outer_bottom);

        let (x, y_top) = data_from_local(rect, plot_point(ATTACK_ORIGIN, 1.0, rect).0, top);
        let (_, y_bottom) = data_from_local(rect, plot_point(ATTACK_ORIGIN, 0.0, rect).0, bottom);
        assert!((x - ATTACK_ORIGIN).abs() < 1.0e-6);
        assert!((y_top - 1.0).abs() < 1.0e-6);
        assert!(y_bottom.abs() < 1.0e-6);
    }

    #[test]
    fn drag_inverse_tracks_coupled_attack_and_decay_geometry() {
        let attack_target = x_positions_from_norms(0.73, 0.42, 0.3).0;
        let solved_attack = solve_norm_for_x(attack_target, |candidate| {
            x_positions_from_norms(candidate, 0.42, 0.3).0
        });
        assert!((solved_attack - 0.73).abs() < 1.0e-5);

        let decay_target = x_positions_from_norms(0.31, 0.81, 0.3).1;
        let solved_decay = solve_norm_for_x(decay_target, |candidate| {
            x_positions_from_norms(0.31, candidate, 0.3).1
        });
        assert!((solved_decay - 0.81).abs() < 1.0e-5);
    }

    #[test]
    fn drag_payload_names_active_stage_and_release_payload_clears_it() {
        let active = env_map(5.0, 120.0, 0.7, 300.0, Some("decay"));
        assert_eq!(map_value(&active, "active"), Value::Keyword("decay".into()));

        let released = env_map(5.0, 120.0, 0.7, 300.0, None);
        assert_eq!(map_value(&released, "active"), Value::Bool(false));
    }

    #[test]
    fn release_commits_last_drag_envelope_instead_of_stale_layout_props() {
        let node = test_node(9_001);
        let gesture = Value::Number(1.0);
        let widget = AdsrEditorWidget;
        let down = widget.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            1.0,
            1.0,
            None,
            Some(&gesture),
            KeyModifiers::NONE,
            1.0,
            1.0,
        );
        assert!(matches!(down, MouseEventOutcome::Consume));

        let drag = widget.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            18.0,
            1.0,
            None,
            Some(&gesture),
            KeyModifiers::NONE,
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(drag_env)) = drag else {
            panic!("ADSR drag should dispatch an envelope");
        };
        let dragged_attack = map_value(&drag_env, "attack");
        assert_ne!(dragged_attack, Value::Number(10.0));
        assert_eq!(
            Value::Number(visual_envelope(&node).attack as f64),
            dragged_attack,
            "the renderer must use live drag state before the reactive props commit",
        );

        // The layout props intentionally remain at the pre-drag value, matching
        // rack-slot parameters that do not have a live reactive value field.
        let release = widget.mouse_event(
            &node,
            MouseEventKind::Up(MouseButton::Left),
            18.0,
            1.0,
            None,
            Some(&gesture),
            KeyModifiers::NONE,
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(release_env)) = release else {
            panic!("ADSR release should commit an envelope");
        };
        assert_eq!(map_value(&release_env, "attack"), dragged_attack);
        assert_eq!(map_value(&release_env, "active"), Value::Bool(false));
    }

    #[test]
    fn shader_draws_only_the_four_live_handles() {
        let shader = ADSR_EDITOR_SHADER.source(crate::widget_render::ShaderBackend::Msl).unwrap();
        assert!(shader.contains("for (int i = 1; i < 5; ++i)"));
        assert!(!shader.contains("for (int i = 0; i < 5; ++i)"));
        assert!(shader.contains(
            "return clamp((x - attackOrigin) / max(x1 - attackOrigin, 1e-5), 0.0, 1.0)"
        ));
        assert!(shader.contains("? segmentT"));
        assert!(!shader.contains("adsr_expRise"));
    }
}
