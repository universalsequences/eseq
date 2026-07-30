//! Read-only piano keyboard activity display.
//!
//! `:notes-by-track` is a list of note-activity lists, indexed by track. Each
//! activity is a `{:note n :velocity v}` map; bare MIDI-note numbers remain
//! accepted as full-velocity shorthand.
//! `:track-colors` is the matching list of DAW colors, and the optional
//! `:tracks` list selects which sources are visible. `:overlap-mode :split`
//! renders simultaneous sources as bands; `:loudest` chooses the
//! highest-velocity source, resolving ties to the first track.
//! A `:trigger-id` change is treated as a new note-on even if that pitch was
//! already active. Note-on compresses the key; removal from the active set
//! releases it back to its resting geometry.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::{styled_cell, CellBuffer, WidgetDefinition};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, MetalRectPrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::{f64_to_f32, get_prop_num, Constraints, LayoutNode, MeasureCtx, Rect, Size};
use crate::theme;
use crate::vm::Value;

pub struct PianoKeyboardWidget;

pub static PIANO_KEYBOARD_WIDGET: PianoKeyboardWidget = PianoKeyboardWidget;

const DEFAULT_START_NOTE: u8 = 21;
const DEFAULT_KEY_COUNT: usize = 88;
const DEFAULT_WIDTH: f32 = 80.0;
const DEFAULT_HEIGHT: f32 = 6.0;
const DEFAULT_PRESS_STRENGTH: f32 = 1.0;
const MAX_PRESS_STRENGTH: f32 = 2.0;
const HELD_PRESS_DEPTH: f32 = 0.48;
const PRESS_ATTACK_SECONDS: f32 = 0.045;
const PRESS_SETTLE_SECONDS: f32 = 0.085;
const RELEASE_SECONDS: f32 = 0.12;

thread_local! {
    static PIANO_ANIMATION_STATES: RefCell<HashMap<u64, PianoAnimationState>> =
        RefCell::new(HashMap::new());
}

fn value_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) if value.is_finite() => Some(*value),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot)),
        _ => None,
    }
}

fn prop_note_range(props: &HashMap<String, Value>) -> (u8, usize) {
    let start = props
        .get("start-note")
        .and_then(value_number)
        .unwrap_or(DEFAULT_START_NOTE as f64)
        .round()
        .clamp(0.0, 127.0) as u8;
    let available = 128 - start as usize;
    let count = props
        .get("key-count")
        .and_then(value_number)
        .unwrap_or(DEFAULT_KEY_COUNT as f64)
        .round()
        .clamp(1.0, available as f64) as usize;
    (start, count)
}

fn press_strength(props: &HashMap<String, Value>) -> f32 {
    props
        .get("press-depth")
        .and_then(value_number)
        .unwrap_or(DEFAULT_PRESS_STRENGTH as f64)
        .clamp(0.0, MAX_PRESS_STRENGTH as f64) as f32
}

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

fn selected_tracks(props: &HashMap<String, Value>) -> Option<HashSet<usize>> {
    let Value::List(items) = props.get("tracks")? else {
        return Some(HashSet::new());
    };
    Some(
        items
            .iter()
            .filter_map(|item| value_number(&item.borrow()))
            .filter(|track| *track >= 0.0)
            .map(|track| track.round() as usize)
            .collect(),
    )
}

fn fallback_track_color(track: usize) -> Color {
    const PALETTE: [[f32; 3]; 8] = [
        [0.24, 0.64, 1.00],
        [1.00, 0.34, 0.50],
        [0.38, 0.90, 0.48],
        [0.70, 0.44, 1.00],
        [1.00, 0.70, 0.20],
        [0.16, 0.86, 0.82],
        [1.00, 0.44, 0.82],
        [0.54, 0.78, 0.20],
    ];
    let [r, g, b] = PALETTE[track % PALETTE.len()];
    Color::rgba(r, g, b, 1.0)
}

fn track_colors(props: &HashMap<String, Value>) -> Vec<Color> {
    let Some(Value::List(items)) = props.get("track-colors") else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| theme::parse_color_value(&item.borrow()))
        .enumerate()
        .map(|(track, color)| color.unwrap_or_else(|| fallback_track_color(track)))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ActiveNoteSource {
    color: Color,
    velocity: f32,
    trigger_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlapMode {
    Split,
    Loudest,
}

fn overlap_mode(props: &HashMap<String, Value>) -> OverlapMode {
    match props.get("overlap-mode") {
        Some(Value::Keyword(mode)) | Some(Value::String(mode)) if mode == "loudest" => {
            OverlapMode::Loudest
        }
        _ => OverlapMode::Split,
    }
}

fn note_activity(value: &Value) -> Option<(u8, f32, u64)> {
    match value {
        Value::Number(note) if note.is_finite() && (0.0..=127.0).contains(note) => {
            Some((note.round() as u8, 1.0, 0))
        }
        Value::Map(activity) => {
            let note = activity
                .get("note")
                .and_then(|note| value_number(&note.borrow()))?
                .round();
            if !(0.0..=127.0).contains(&note) {
                return None;
            }
            let velocity = activity
                .get("velocity")
                .and_then(|velocity| value_number(&velocity.borrow()))
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32;
            let trigger_id = activity
                .get("trigger-id")
                .and_then(|trigger_id| value_number(&trigger_id.borrow()))
                .unwrap_or(0.0)
                .max(0.0)
                .round() as u64;
            Some((note as u8, velocity, trigger_id))
        }
        _ => None,
    }
}

fn active_note_sources(props: &HashMap<String, Value>) -> Vec<Vec<ActiveNoteSource>> {
    let mut active = vec![Vec::new(); 128];
    let Some(Value::List(tracks)) = props.get("notes-by-track") else {
        return active;
    };
    let selected = selected_tracks(props);
    let colors = track_colors(props);
    for (track, notes) in tracks.iter().enumerate() {
        if selected
            .as_ref()
            .is_some_and(|selected| !selected.contains(&track))
        {
            continue;
        }
        let Value::List(notes) = &*notes.borrow() else {
            continue;
        };
        let color = colors
            .get(track)
            .copied()
            .unwrap_or_else(|| fallback_track_color(track));
        for activity in notes {
            let Some((note, velocity, trigger_id)) = note_activity(&activity.borrow()) else {
                continue;
            };
            active[note as usize].push(ActiveNoteSource {
                color,
                velocity,
                trigger_id,
            });
        }
    }
    active
}

fn displayed_sources(sources: &[ActiveNoteSource], mode: OverlapMode) -> Vec<ActiveNoteSource> {
    match mode {
        OverlapMode::Split => sources.to_vec(),
        OverlapMode::Loudest => sources
            .iter()
            .copied()
            .reduce(|winner, candidate| {
                if candidate.velocity > winner.velocity {
                    candidate
                } else {
                    winner
                }
            })
            .into_iter()
            .collect(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyAnimationState {
    active: bool,
    trigger_id: u64,
    onset_started_at: f32,
    onset_from_depth: f32,
    release_started_at: f32,
    release_from_depth: f32,
}

impl Default for KeyAnimationState {
    fn default() -> Self {
        Self {
            active: false,
            trigger_id: 0,
            onset_started_at: f32::NEG_INFINITY,
            onset_from_depth: 0.0,
            release_started_at: f32::NEG_INFINITY,
            release_from_depth: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
struct PianoAnimationState {
    keys: [KeyAnimationState; 128],
}

impl Default for PianoAnimationState {
    fn default() -> Self {
        Self {
            keys: [KeyAnimationState::default(); 128],
        }
    }
}

fn ease_out_cubic(value: f32) -> f32 {
    let remaining = 1.0 - value.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

fn key_press_depth(state: KeyAnimationState, time_seconds: f32) -> f32 {
    if state.active {
        let age = (time_seconds - state.onset_started_at).max(0.0);
        if age < PRESS_ATTACK_SECONDS {
            let progress = ease_out_cubic(age / PRESS_ATTACK_SECONDS);
            return state.onset_from_depth + (1.0 - state.onset_from_depth) * progress;
        }
        if age < PRESS_ATTACK_SECONDS + PRESS_SETTLE_SECONDS {
            let progress = ease_out_cubic((age - PRESS_ATTACK_SECONDS) / PRESS_SETTLE_SECONDS);
            return 1.0 + (HELD_PRESS_DEPTH - 1.0) * progress;
        }
        HELD_PRESS_DEPTH
    } else {
        let age = (time_seconds - state.release_started_at).max(0.0);
        if age < RELEASE_SECONDS {
            state.release_from_depth * (1.0 - ease_out_cubic(age / RELEASE_SECONDS))
        } else {
            0.0
        }
    }
}

fn key_animation_active(state: KeyAnimationState, time_seconds: f32) -> bool {
    if state.active {
        time_seconds - state.onset_started_at < PRESS_ATTACK_SECONDS + PRESS_SETTLE_SECONDS
    } else {
        time_seconds - state.release_started_at < RELEASE_SECONDS
    }
}

fn newest_trigger_id(sources: &[ActiveNoteSource]) -> u64 {
    sources
        .iter()
        .map(|source| source.trigger_id)
        .max()
        .unwrap_or(0)
}

fn observe_key_activity(
    widget_id: u64,
    active: &[Vec<ActiveNoteSource>],
    time_seconds: f32,
) -> [f32; 128] {
    PIANO_ANIMATION_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.entry(widget_id).or_default();
        let mut depths = [0.0; 128];
        for (note, sources) in active.iter().enumerate() {
            let is_active = !sources.is_empty();
            let trigger_id = newest_trigger_id(sources);
            let key = &mut state.keys[note];
            if is_active && (!key.active || trigger_id > key.trigger_id) {
                let current_depth = key_press_depth(*key, time_seconds);
                key.active = true;
                key.trigger_id = trigger_id;
                key.onset_started_at = time_seconds;
                key.onset_from_depth = current_depth;
                key.release_started_at = f32::NEG_INFINITY;
            } else if !is_active && key.active {
                let current_depth = key_press_depth(*key, time_seconds);
                key.active = false;
                key.release_started_at = time_seconds;
                key.release_from_depth = current_depth;
                key.onset_started_at = f32::NEG_INFINITY;
            }
            depths[note] = key_press_depth(*key, time_seconds);
        }
        depths
    })
}

fn piano_animation_active(widget_id: u64, time_seconds: f32) -> bool {
    PIANO_ANIMATION_STATES.with(|states| {
        states.borrow().get(&widget_id).is_some_and(|state| {
            state
                .keys
                .iter()
                .any(|key| key_animation_active(*key, time_seconds))
        })
    })
}

fn pressed_key_rect(rect: Rect, press_depth: f32, press_strength: f32) -> Rect {
    let depth = press_depth.clamp(0.0, 1.0) * press_strength.clamp(0.0, MAX_PRESS_STRENGTH);
    let width = rect.width * (1.0 - 0.10 * depth);
    let height = rect.height * (1.0 - 0.16 * depth);
    Rect {
        row: rect.row + (rect.height - height) * 0.5,
        col: rect.col + (rect.width - width) * 0.5,
        width,
        height,
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug)]
struct PianoKeyGeometry {
    note: u8,
    rect: Rect,
    black: bool,
}

#[cfg(target_os = "macos")]
fn key_geometry(
    rect: Rect,
    start_note: u8,
    key_count: usize,
    viewport: WidgetViewport,
) -> Vec<PianoKeyGeometry> {
    let notes = (start_note as usize..start_note as usize + key_count).map(|note| note as u8);
    let white_count = notes.clone().filter(|note| !is_black(*note)).count();
    if white_count == 0 {
        return vec![PianoKeyGeometry {
            note: start_note,
            rect,
            black: is_black(start_note),
        }];
    }

    let white_width = rect.width / white_count as f32;
    let black_width = white_width * 0.62;
    let black_height = rect.height * 0.64;
    let gap = (1.0 / viewport.cell_w.max(1.0)).min(white_width * 0.2);
    let vertical_inset = (1.0 / viewport.cell_h.max(1.0)).min(rect.height * 0.1);
    let mut whites_before = 0usize;
    let mut keys = Vec::with_capacity(key_count);
    for note in notes {
        let black = is_black(note);
        let (col, width, height) = if black {
            let boundary = rect.col + whites_before as f32 * white_width;
            (
                (boundary - black_width * 0.5).clamp(rect.col, rect.col + rect.width - black_width),
                black_width,
                black_height,
            )
        } else {
            let col = rect.col + whites_before as f32 * white_width;
            whites_before += 1;
            (col, white_width, rect.height)
        };
        keys.push(PianoKeyGeometry {
            note,
            black,
            rect: Rect {
                row: rect.row + vertical_inset * 0.5,
                col: col + gap * 0.5,
                width: (width - gap).max(0.01),
                height: (height - vertical_inset).max(0.01),
            },
        });
    }
    keys
}

#[cfg(target_os = "macos")]
fn mix_with(color: Color, other: Color, amount: f32) -> Color {
    let t = amount.clamp(0.0, 1.0);
    Color::rgba(
        color.r + (other.r - color.r) * t,
        color.g + (other.g - color.g) * t,
        color.b + (other.b - color.b) * t,
        color.a + (other.a - color.a) * t,
    )
}

impl WidgetDefinition for PianoKeyboardWidget {
    fn names(&self) -> &'static [&'static str] {
        &["piano-keyboard"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["notes-by-track", "track-colors", "press-depth"]
    }

    fn wants_animation_frames(&self, node: &LayoutNode) -> bool {
        piano_animation_active(
            node.widget_id,
            crate::widget_render::sdf_widget::current_sdf_time_seconds(),
        )
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(DEFAULT_WIDTH)
            .max(1.0)
            .min(constraints.max_width.max(1.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(DEFAULT_HEIGHT)
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let (start_note, key_count) = prop_note_range(props);
        let active = active_note_sources(props);
        let mode = overlap_mode(props);
        let width = rect.width.round().max(1.0) as usize;
        for column in 0..width {
            let offset = (column * key_count / width).min(key_count - 1);
            let note = start_note as usize + offset;
            let sources = displayed_sources(&active[note], mode);
            let color = sources
                .first()
                .map(|source| source.color)
                .unwrap_or_else(|| {
                    if is_black(note as u8) {
                        Color::rgba(0.08, 0.08, 0.09, 1.0)
                    } else {
                        Color::rgba(0.84, 0.85, 0.87, 1.0)
                    }
                });
            let glyph = if sources.is_empty() { '▂' } else { '█' };
            buf.set(
                rect.row.round() as u16,
                rect.col.round() as u16 + column as u16,
                styled_cell(glyph, color, None),
            );
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let (start_note, key_count) = prop_note_range(&node.props);
        let active = active_note_sources(&node.props);
        let mode = overlap_mode(&node.props);
        let press_strength = press_strength(&node.props);
        let press_depths = observe_key_activity(node.widget_id, &active, viewport.time_seconds);
        let keys = key_geometry(node.rect, start_note, key_count, viewport);
        let white = Color::rgba(0.88, 0.89, 0.91, 1.0);
        let black = Color::rgba(0.055, 0.058, 0.065, 1.0);
        let chassis = Color::rgba(0.018, 0.020, 0.024, 1.0);
        let mut primitives = vec![MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: chassis,
        })];

        for black_pass in [false, true] {
            for key in keys.iter().filter(|key| key.black == black_pass) {
                let key_rect =
                    pressed_key_rect(key.rect, press_depths[key.note as usize], press_strength);
                primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
                    rect: key_rect,
                    color: if key.black { black } else { white },
                }));
                let sources = displayed_sources(&active[key.note as usize], mode);
                for (index, source) in sources.iter().enumerate() {
                    let segment_width = key_rect.width / sources.len() as f32;
                    let mut color = if key.black {
                        mix_with(source.color, Color::rgba(0.0, 0.0, 0.0, 1.0), 0.08)
                    } else {
                        mix_with(source.color, Color::rgba(1.0, 1.0, 1.0, 1.0), 0.12)
                    };
                    color.a = source.velocity;
                    primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
                        rect: Rect {
                            row: key_rect.row,
                            col: key_rect.col + segment_width * index as f32,
                            width: segment_width,
                            height: key_rect.height,
                        },
                        color,
                    }));
                }
            }
        }
        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn list(values: Vec<Value>) -> Value {
        Value::List(
            values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(value)))
                .collect(),
        )
    }

    fn activity_with_trigger(note: f64, velocity: f64, trigger_id: f64) -> Value {
        Value::Map(HashMap::from([
            (
                "note".to_string(),
                Rc::new(RefCell::new(Value::Number(note))),
            ),
            (
                "velocity".to_string(),
                Rc::new(RefCell::new(Value::Number(velocity))),
            ),
            (
                "trigger-id".to_string(),
                Rc::new(RefCell::new(Value::Number(trigger_id))),
            ),
        ]))
    }

    fn activity(note: f64, velocity: f64) -> Value {
        activity_with_trigger(note, velocity, note)
    }

    fn props_with_activity() -> HashMap<String, Value> {
        HashMap::from([
            (
                "notes-by-track".to_string(),
                list(vec![
                    list(vec![activity(60.0, 0.4), activity(64.0, 0.8)]),
                    list(vec![activity(60.0, 0.9), activity(67.0, 0.3)]),
                ]),
            ),
            (
                "track-colors".to_string(),
                list(vec![
                    list(vec![
                        Value::Number(1.0),
                        Value::Number(0.0),
                        Value::Number(0.0),
                    ]),
                    list(vec![
                        Value::Number(0.0),
                        Value::Number(0.0),
                        Value::Number(1.0),
                    ]),
                ]),
            ),
            (
                "tracks".to_string(),
                list(vec![Value::Number(0.0), Value::Number(1.0)]),
            ),
        ])
    }

    #[test]
    fn activity_keeps_source_track_colors_velocities_and_collisions() {
        let active = active_note_sources(&props_with_activity());
        assert_eq!(active[60].len(), 2);
        assert_eq!(
            active[60][0],
            ActiveNoteSource {
                color: Color::rgba(1.0, 0.0, 0.0, 1.0),
                velocity: 0.4,
                trigger_id: 60,
            }
        );
        assert_eq!(
            active[60][1],
            ActiveNoteSource {
                color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                velocity: 0.9,
                trigger_id: 60,
            }
        );
        assert_eq!(active[64][0].velocity, 0.8);
        assert_eq!(active[67][0].velocity, 0.3);
    }

    #[test]
    fn track_filter_excludes_unselected_activity() {
        let mut props = props_with_activity();
        props.insert("tracks".to_string(), list(vec![Value::Number(1.0)]));
        let active = active_note_sources(&props);
        assert_eq!(
            active[60],
            vec![ActiveNoteSource {
                color: Color::rgba(0.0, 0.0, 1.0, 1.0),
                velocity: 0.9,
                trigger_id: 60,
            }]
        );
        assert!(active[64].is_empty());
        assert_eq!(active[67][0].velocity, 0.3);
    }

    #[test]
    fn loudest_overlap_uses_velocity_and_ties_choose_first_track() {
        let mut props = props_with_activity();
        props.insert(
            "overlap-mode".to_string(),
            Value::Keyword("loudest".to_string()),
        );
        let active = active_note_sources(&props);
        let winner = displayed_sources(&active[60], overlap_mode(&props));
        assert_eq!(winner.len(), 1);
        assert_eq!(winner[0].color, Color::rgba(0.0, 0.0, 1.0, 1.0));
        assert_eq!(winner[0].velocity, 0.9);

        {
            let Value::List(tracks) = props.get("notes-by-track").unwrap() else {
                unreachable!()
            };
            let mut first_track_value = tracks[0].borrow_mut();
            let Value::List(first_track) = &mut *first_track_value else {
                unreachable!()
            };
            *first_track[0].borrow_mut() = activity(60.0, 0.9);
        }
        let active = active_note_sources(&props);
        let tied_winner = displayed_sources(&active[60], overlap_mode(&props));
        assert_eq!(
            tied_winner[0].color,
            Color::rgba(1.0, 0.0, 0.0, 1.0),
            "equal velocity must keep the first track"
        );
    }

    #[test]
    fn note_range_is_clamped_to_midi_domain() {
        let props = HashMap::from([
            ("start-note".to_string(), Value::Number(120.0)),
            ("key-count".to_string(), Value::Number(80.0)),
        ]);
        assert_eq!(prop_note_range(&props), (120, 8));
    }

    #[test]
    fn note_on_retrigger_and_note_off_drive_press_and_release_motion() {
        let widget_id = 9_001;
        let props = props_with_activity();
        let first = active_note_sources(&props);
        let at_onset = observe_key_activity(widget_id, &first, 1.0);
        assert_eq!(at_onset[60], 0.0);
        assert!(piano_animation_active(widget_id, 1.0));

        let fully_pressed = observe_key_activity(widget_id, &first, 1.0 + PRESS_ATTACK_SECONDS);
        assert!((fully_pressed[60] - 1.0).abs() < 0.0001);
        let held = observe_key_activity(widget_id, &first, 1.2);
        assert!((held[60] - HELD_PRESS_DEPTH).abs() < 0.0001);
        assert!(!piano_animation_active(widget_id, 1.2));

        {
            let Value::List(tracks) = props.get("notes-by-track").unwrap() else {
                unreachable!()
            };
            let mut first_track_value = tracks[0].borrow_mut();
            let Value::List(first_track) = &mut *first_track_value else {
                unreachable!()
            };
            *first_track[0].borrow_mut() = activity_with_trigger(60.0, 0.4, 10_000.0);
        }
        let retriggered = active_note_sources(&props);
        let retrigger_start = observe_key_activity(widget_id, &retriggered, 1.21);
        assert!((retrigger_start[60] - HELD_PRESS_DEPTH).abs() < 0.0001);
        let retrigger_peak =
            observe_key_activity(widget_id, &retriggered, 1.21 + PRESS_ATTACK_SECONDS);
        assert!((retrigger_peak[60] - 1.0).abs() < 0.0001);

        let inactive = vec![Vec::new(); 128];
        let release_start = observe_key_activity(widget_id, &inactive, 1.3);
        assert!(release_start[60] > 0.0);
        assert!(piano_animation_active(widget_id, 1.3));
        let released = observe_key_activity(widget_id, &inactive, 1.3 + RELEASE_SECONDS);
        assert_eq!(released[60], 0.0);
        assert!(!piano_animation_active(widget_id, 1.3 + RELEASE_SECONDS));
    }

    #[test]
    fn pressed_geometry_is_smaller_and_remains_centered() {
        let rect = Rect {
            row: 2.0,
            col: 3.0,
            width: 4.0,
            height: 8.0,
        };
        let pressed = pressed_key_rect(rect, 1.0, 1.0);
        assert!(pressed.width < rect.width);
        assert!(pressed.height < rect.height);
        assert_eq!(
            pressed.col + pressed.width * 0.5,
            rect.col + rect.width * 0.5
        );
        assert_eq!(
            pressed.row + pressed.height * 0.5,
            rect.row + rect.height * 0.5
        );
        assert_eq!(pressed_key_rect(rect, 1.0, 0.0), rect);
        let exaggerated = pressed_key_rect(rect, 1.0, 2.0);
        assert!(exaggerated.width < pressed.width);
        assert!(exaggerated.height < pressed.height);
    }

    #[test]
    fn active_press_registers_widget_for_animation_frames() {
        let widget_id = 9_002;
        let props = props_with_activity();
        let active = active_note_sources(&props);
        let now = crate::widget_render::sdf_widget::current_sdf_time_seconds();
        observe_key_activity(widget_id, &active, now);
        let node = LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "piano-keyboard".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 80.0,
                height: 6.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
        };
        assert!(PIANO_KEYBOARD_WIDGET.wants_animation_frames(&node));
        assert!(crate::widget_render::layout_wants_animation_frames(&node));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn eighty_key_geometry_is_finite_and_uses_real_black_key_overlay() {
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let rect = Rect {
            row: 1.0,
            col: 2.0,
            width: 82.0,
            height: 7.0,
        };
        let keys = key_geometry(rect, 24, 80, viewport);
        assert_eq!(keys.len(), 80);
        assert!(keys.iter().all(|key| {
            key.rect.row.is_finite()
                && key.rect.col.is_finite()
                && key.rect.width > 0.0
                && key.rect.height > 0.0
        }));
        let white = keys.iter().find(|key| !key.black).expect("white key");
        let black = keys.iter().find(|key| key.black).expect("black key");
        assert!(black.rect.width < white.rect.width);
        assert!(black.rect.height < white.rect.height);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn loudest_metal_render_uses_velocity_as_color_opacity_without_stripes() {
        let mut props = props_with_activity();
        props.insert(
            "overlap-mode".to_string(),
            Value::Keyword("loudest".to_string()),
        );
        let node = LayoutNode {
            widget_id: 99,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "piano-keyboard".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 80.0,
                height: 6.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let primitives =
            PIANO_KEYBOARD_WIDGET.build_metal_primitives("piano-keyboard", &node, viewport);
        let mut activity_alphas: Vec<f32> = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                MetalPrimitive::Rect(rect) if rect.color.a < 1.0 => Some(rect.color.a),
                _ => None,
            })
            .collect();
        activity_alphas.sort_by(f32::total_cmp);
        assert_eq!(activity_alphas, vec![0.3, 0.8, 0.9]);
        assert!(
            !activity_alphas.contains(&0.4),
            "the quieter colliding source must not emit a stripe"
        );
    }

    #[test]
    fn live_data_props_accept_reactive_bindings() {
        assert_eq!(
            PIANO_KEYBOARD_WIDGET.bindable_props(),
            &["notes-by-track", "track-colors", "press-depth"]
        );
    }

    #[test]
    fn press_strength_reads_live_reactive_slot_updates() {
        let slot = std::sync::Arc::new(std::sync::atomic::AtomicU64::new((0.5_f64).to_bits()));
        let props = HashMap::from([(
            "press-depth".to_string(),
            Value::ReactiveRef {
                namespace: "PIANO_TEST".to_string(),
                field: "press-depth".to_string(),
                index: None,
                kind: crate::vm::BindingKind::Float,
                slot: std::sync::Arc::clone(&slot),
            },
        )]);
        assert_eq!(press_strength(&props), 0.5);

        slot.store((1.75_f64).to_bits(), std::sync::atomic::Ordering::Release);
        assert_eq!(press_strength(&props), 1.75);
    }
}
