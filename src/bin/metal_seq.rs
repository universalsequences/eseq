use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use eseqlisp::backend::Backend;
use eseqlisp::editor::ViewMode;
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::vm::Value;
use eseqlisp::{BufferMode, Editor, EditorConfig, HostCommand, HostEvent, Runtime};

use sequencer::engine;
use sequencer::sequencer::{
    KeyboardTrigger, SequencerState, StepParam, SwingResolution, Timebase, MAX_STEPS,
    SYNC_RESOLUTIONS,
};
use sequencer::ui;
use std::sync::atomic::AtomicBool;

/// Read the current sampler playhead position (in seconds) for a track.
/// Scans all voices and returns the most recently triggered one (smallest
/// non-zero playhead, meaning it just started playing).
fn read_sampler_playhead_seconds(app: &ui::App, track: usize) -> f64 {
    let sampler_ids = match app.graph.track_node_ids.get(track) {
        Some(ids) => &ids.sampler_ids,
        None => return 0.0,
    };
    let min_state_bytes = sequencer::sampler::SAMPLER_STATE_SIZE * std::mem::size_of::<f32>();

    // Find the voice with the smallest positive playhead (most recently triggered)
    let mut best_playhead: f64 = 0.0;
    let mut best_is_playing = false;

    for &node_id in sampler_ids {
        if node_id < 0 {
            continue;
        }
        let mut state_size = 0usize;
        let mut state = [0.0_f32; sequencer::sampler::SAMPLER_STATE_SIZE];
        let copied = unsafe {
            sequencer::audiograph::get_node_state_into(
                app.graph.lg.0,
                node_id,
                state.as_mut_ptr().cast(),
                std::mem::size_of_val(&state),
                &mut state_size as *mut usize,
            )
        };
        if !copied || state_size < min_state_bytes {
            continue;
        }
        let ph = state[sequencer::sampler::PARAM_PLAYHEAD as usize] as f64;
        let playing = state[sequencer::sampler::PARAM_TRIGGER as usize] > 0.0;
        // Prefer playing voices; among those, pick the smallest playhead (most recent trigger)
        if playing && (!best_is_playing || ph < best_playhead) {
            best_playhead = ph;
            best_is_playing = true;
        } else if !best_is_playing && ph > best_playhead {
            // No playing voice found yet — pick the largest playhead (last to finish)
            best_playhead = ph;
        }
    }

    if best_playhead <= 0.0 {
        return 0.0;
    }

    // Convert frame index to seconds using the registered sample's metadata
    let sample = app
        .sampler_paths
        .get(track)
        .and_then(|p| p.as_ref())
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()));
    match sample {
        Some(s) if s.frames > 0 => best_playhead * s.duration_seconds / s.frames as f64,
        _ => {
            let sr = app.graph.sample_rate.max(1) as f64;
            best_playhead / sr
        }
    }
}

fn sync_watched_sampler_voices(
    app: &ui::App,
    current_track: usize,
    watched_track: &mut Option<usize>,
    watched_voice_ids: &mut Vec<i32>,
) {
    let desired_voice_ids =
        if current_track < app.tracks.len() && app.is_sampler_track(current_track) {
            app.graph
                .track_node_ids
                .get(current_track)
                .map(|ids| ids.sampler_ids.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    if *watched_track == Some(current_track) && *watched_voice_ids == desired_voice_ids {
        return;
    }

    for node_id in watched_voice_ids.drain(..) {
        unsafe {
            sequencer::audiograph::remove_node_from_watchlist(app.graph.lg.0, node_id);
        }
    }

    for &node_id in &desired_voice_ids {
        if node_id >= 0 {
            unsafe {
                sequencer::audiograph::add_node_to_watchlist(app.graph.lg.0, node_id);
            }
        }
    }

    *watched_track = if desired_voice_ids.is_empty() {
        None
    } else {
        Some(current_track)
    };
    *watched_voice_ids = desired_voice_ids;
}

/// Register a WAV file with eseqlisp's sample registry so the waveform widget can display it.
fn register_waveform_sample(path: &Path) {
    match eseqlisp::audio::sample::SampleBuffer::load_wav(path) {
        Ok(sample) => {
            sample.register();
        }
        Err(e) => {
            eprintln!(
                "waveform: failed to register sample {}: {e}",
                path.display()
            );
        }
    }
}

const DEFAULT_SAMPLES: &[&str] = &[
    "samples/producers/Boom-Bap/Boom-Bap Kick 51.wav",
    "samples/producers/madlib/Snare SwaggedOut 3.wav",
    "samples/producers/donda/PABLO HAT.wav",
];
const PAGE_SIZE: usize = 16;
const AUTO_FOLLOW_COOLDOWN: Duration = Duration::from_secs(5);
const METER_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CPU_UI_POLL_INTERVAL: Duration = Duration::from_millis(500);
const VOICE_COUNT_LOG_INTERVAL: Duration = Duration::from_secs(2);
const METER_LEVEL_STEPS: f64 = 48.0;
const BUILTIN_ACCUMULATOR_NAMES: &[&str] = &[
    "Off",
    "TransposeRamp",
    "VelocityDecay",
    "OctaveEcho",
    "SendToTrack",
];
const ACCUM_MODE_LABELS: &[&str] = &["rtz", "clip", "rvtz", "rvbp"];
const FTS_SCALE_NAMES: &[&str] = &[
    "Off",
    "Major",
    "Minor",
    "Dorian",
    "Mixolydian",
    "Lydian",
    "Phrygian",
    "Locrian",
    "Pent. Major",
    "Pent. Minor",
    "Blues",
    "Whole Tone",
    "Diminished",
];
const PIANO_ROLL_ID_STRIDE: usize = 16;
const PIANO_ROLL_MIN_TRANSPOSE: i32 = -48;
const PIANO_ROLL_MAX_TRANSPOSE: i32 = 48;
const PIANO_ROLL_MIN_DURATION: f32 = 0.03125;

#[derive(Clone)]
struct PianoRollNote {
    transpose: f32,
    duration: f32,
}

fn piano_roll_sanitize_duration(duration: f32) -> f32 {
    duration.max(PIANO_ROLL_MIN_DURATION)
}

#[derive(Clone)]
struct PianoRollMoveItem {
    id: u64,
    step: usize,
    transpose: f32,
    duration: f32,
}

struct PianoRollMoveState {
    ids: Vec<u64>,
    anchor_step: usize,
    anchor_lane: isize,
    originals: Vec<PianoRollMoveItem>,
    last_positions: Vec<PianoRollMoveItem>,
}

struct UiLoopStats {
    enabled: bool,
    window_start: Instant,
    events: u64,
    syncs: u64,
    frames: u64,
    event_handle: Duration,
    gestures: Duration,
    host_commands: Duration,
    reactive_sync: Duration,
    frame_build: Duration,
    render: Duration,
    max_event: Duration,
    max_sync: Duration,
    max_frame_build: Duration,
    max_render: Duration,
}

impl UiLoopStats {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("ESEQLISP_PROFILE_UI").is_some(),
            window_start: Instant::now(),
            events: 0,
            syncs: 0,
            frames: 0,
            event_handle: Duration::ZERO,
            gestures: Duration::ZERO,
            host_commands: Duration::ZERO,
            reactive_sync: Duration::ZERO,
            frame_build: Duration::ZERO,
            render: Duration::ZERO,
            max_event: Duration::ZERO,
            max_sync: Duration::ZERO,
            max_frame_build: Duration::ZERO,
            max_render: Duration::ZERO,
        }
    }

    fn note_event(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.events += 1;
        self.event_handle += elapsed;
        self.max_event = self.max_event.max(elapsed);
        self.maybe_emit();
    }

    fn note_gestures(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.gestures += elapsed;
        self.maybe_emit();
    }

    fn note_host_commands(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.host_commands += elapsed;
        self.maybe_emit();
    }

    fn note_sync(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.reactive_sync += elapsed;
        self.syncs += 1;
        self.max_sync = self.max_sync.max(elapsed);
        self.maybe_emit();
    }

    fn note_frame(&mut self, build: Duration, render: Duration) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
        self.frame_build += build;
        self.render += render;
        self.max_frame_build = self.max_frame_build.max(build);
        self.max_render = self.max_render.max(render);
        self.maybe_emit();
    }

    fn maybe_emit(&mut self) {
        if !self.enabled || self.window_start.elapsed().as_secs_f64() < 1.0 {
            return;
        }
        let secs = self.window_start.elapsed().as_secs_f64();
        eprintln!(
            "[ui-profile][sequencer] events/s={:.1} frames/s={:.1} event_avg={:.2}ms event_max={:.2}ms gestures={:.2}ms host={:.2}ms sync_avg={:.2}ms sync_max={:.2}ms frame_build_avg={:.2}ms frame_build_max={:.2}ms render_avg={:.2}ms render_max={:.2}ms",
            self.events as f64 / secs,
            self.frames as f64 / secs,
            avg_ms(self.event_handle, self.events),
            self.max_event.as_secs_f64() * 1000.0,
            self.gestures.as_secs_f64() * 1000.0,
            self.host_commands.as_secs_f64() * 1000.0,
            avg_ms(self.reactive_sync, self.syncs),
            self.max_sync.as_secs_f64() * 1000.0,
            avg_ms(self.frame_build, self.frames),
            self.max_frame_build.as_secs_f64() * 1000.0,
            avg_ms(self.render, self.frames),
            self.max_render.as_secs_f64() * 1000.0,
        );
        self.window_start = Instant::now();
        self.events = 0;
        self.syncs = 0;
        self.frames = 0;
        self.event_handle = Duration::ZERO;
        self.gestures = Duration::ZERO;
        self.host_commands = Duration::ZERO;
        self.reactive_sync = Duration::ZERO;
        self.frame_build = Duration::ZERO;
        self.render = Duration::ZERO;
        self.max_event = Duration::ZERO;
        self.max_sync = Duration::ZERO;
        self.max_frame_build = Duration::ZERO;
        self.max_render = Duration::ZERO;
    }
}

fn avg_ms(total: Duration, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total.as_secs_f64() * 1000.0 / count as f64
    }
}

fn log_active_voice_counts(state: &SequencerState, track_names: &[String]) {
    let num_tracks = state.active_track_count().min(track_names.len());
    if num_tracks == 0 {
        return;
    }
    let cpu_load = f32::from_bits(state.transport.cpu_load_pct.load(Ordering::Relaxed));
    let mut total = 0u32;
    let mut parts = Vec::with_capacity(num_tracks);
    for track in 0..num_tracks {
        let active = state.transport.active_voice_counts[track].load(Ordering::Relaxed);
        total += active;
        parts.push(format!("{}={active}", track_names[track]));
    }
    eprintln!(
        "[voice-counts] total={total} audio_cpu={cpu_load:.1}% {}",
        parts.join(" ")
    );
}

#[derive(Clone)]
struct SampleTreeNode {
    label: String,
    label_lower: String,
    path: Option<String>,
    children: Vec<SampleTreeNode>,
}

#[derive(Clone, Debug)]
struct HeldKeyboardNote {
    key: char,
    transpose: f32,
    step_at_press: usize,
    press_time: Instant,
    tracks: Vec<usize>,
}

fn build_sample_tree_node(dir: &std::path::Path) -> Vec<SampleTreeNode> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            dirs.push((name, path));
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("wav") {
                files.push((name, path.to_string_lossy().to_string()));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut items = Vec::new();

    for (label, path) in dirs {
        let children = build_sample_tree_node(&path);
        if children.is_empty() {
            continue;
        }
        items.push(SampleTreeNode {
            label_lower: label.to_lowercase(),
            label,
            path: None,
            children,
        });
    }

    for (label, full_path) in files {
        items.push(SampleTreeNode {
            label_lower: label.to_lowercase(),
            label,
            path: Some(full_path),
            children: Vec::new(),
        });
    }

    items
}

fn auto_follow_enabled(override_until: &Arc<Mutex<Option<Instant>>>) -> bool {
    let guard = override_until.lock().unwrap();
    match *guard {
        Some(until) => Instant::now() >= until,
        None => true,
    }
}

fn sample_tree_nodes_to_value(items: &[SampleTreeNode]) -> Value {
    Value::List(
        items
            .iter()
            .map(|item| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "label".to_string(),
                    Rc::new(RefCell::new(Value::String(item.label.clone()))),
                );
                if !item.children.is_empty() {
                    map.insert(
                        "children".to_string(),
                        Rc::new(RefCell::new(sample_tree_nodes_to_value(&item.children))),
                    );
                }
                if let Some(path) = &item.path {
                    map.insert(
                        "path".to_string(),
                        Rc::new(RefCell::new(Value::String(path.clone()))),
                    );
                }
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect(),
    )
}

fn filter_sample_tree_nodes(items: &[SampleTreeNode], query_lower: &str) -> Vec<SampleTreeNode> {
    if query_lower.is_empty() {
        return items.to_vec();
    }

    let mut filtered = Vec::new();
    for item in items {
        if item.children.is_empty() {
            if item.label_lower.contains(query_lower) {
                filtered.push(item.clone());
            }
            continue;
        }

        let children = filter_sample_tree_nodes(&item.children, query_lower);
        if !children.is_empty() {
            filtered.push(SampleTreeNode {
                label: item.label.clone(),
                label_lower: item.label_lower.clone(),
                path: None,
                children,
            });
        }
    }
    filtered
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    sequencer::crash::install()?;

    // 1. Init audio engine
    let eng = engine::init_engine()?;
    let lg_ptr = eng.lg_ptr;
    let state = eng.state.clone();
    let stream = eng._stream;

    // 2. Create App and add default tracks
    let mut app = ui::App::new(
        eng.state.clone(),
        eng.lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder,
        eng.keyboard_tx,
    );

    let mut track_names: Vec<String> = Vec::new();
    for sample_path in DEFAULT_SAMPLES {
        let path = Path::new(sample_path);
        if path.exists() {
            let idx = app.graph_controller().add_track(path)?;
            register_waveform_sample(path);
            let name = app.tracks[idx].clone();
            eprintln!("metal_seq: track {idx} = {name} ({sample_path})");
            track_names.push(name);
        } else {
            eprintln!("metal_seq: skipping missing sample: {sample_path}");
        }
    }

    if track_names.is_empty() {
        return Err("No samples found".into());
    }

    // Collect node IDs for param pushing to audiograph
    let track_pan_ids: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(
        app.graph.track_node_ids.iter().map(|n| n.pan_id).collect(),
    ));
    let effect_descriptors = app.graph.effect_descriptors.clone();
    let lg_raw = lg_ptr.0;

    // Shared current track index
    let current_track = Arc::new(AtomicUsize::new(0));
    // Selected steps for p-locking
    let selected_steps: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_selection: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>> = Arc::new(Mutex::new(None));
    let step_clipboard: Arc<Mutex<Option<Vec<(usize, sequencer::sequencer::StepSnapshot)>>>> =
        Arc::new(Mutex::new(None));
    // UI-only counter for changes that shouldn't affect pattern_epoch (e.g. volume, selection)
    let ui_epoch = Arc::new(AtomicUsize::new(0));
    // FX/instrument panel refresh counter for changes that affect *fx* but
    // should not force *fx* to rerun on unrelated step-grid edits.
    let fx_epoch = Arc::new(AtomicUsize::new(0));
    // When set, pagination stays on the user-selected page until the cooldown expires.
    let auto_follow_override_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    // Recording state shared between native functions and event loop
    let recording = Arc::new(AtomicBool::new(false));
    let record_armed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(vec![false; track_names.len()]));
    // Keyboard trigger sender for live playing when armed
    let keyboard_tx = app.graph.keyboard_tx.clone();
    // Keyboard octave offset for live playing
    let keyboard_octave = Arc::new(std::sync::atomic::AtomicI32::new(0));
    // Held keys for recording: (key_char, transpose, step_at_press, press_instant)
    let held_notes: Arc<Mutex<Vec<HeldKeyboardNote>>> = Arc::new(Mutex::new(Vec::new()));

    // 3. Set up eseqlisp runtime with sequencer natives
    let mut runtime = Runtime::new();

    let track_count = track_names.len();
    let accumulator_names = Arc::new(Mutex::new(build_accumulator_names(&app)));

    // Register SEQ reactive namespace
    runtime.register_reactive(
        "SEQ",
        {
            let mut fields = vec![
                ("playing", Value::Bool(false)),
                ("bpm", Value::Number(120.0)),
                ("num-steps", Value::Number(PAGE_SIZE as f64)),
                ("num-tracks", Value::Number(track_count as f64)),
                ("current-track", Value::Number(0.0)),
                (
                    "current-pattern",
                    Value::Number(state.pattern.current_pattern.load(Ordering::Relaxed) as f64),
                ),
                (
                    "num-patterns",
                    Value::Number(state.pattern.num_patterns.load(Ordering::Relaxed) as f64),
                ),
                ("auto-follow", Value::Bool(true)),
                ("playhead", Value::Number(0.0)),
                ("transport-playhead", Value::Number(0.0)),
                ("sampler-playhead", Value::Number(0.0)),
                ("track-names", build_track_names(&track_names)),
                ("steps", build_steps_value(&state, 0)),
                ("piano-roll-lanes", build_piano_roll_lanes_value()),
                (
                    "piano-roll-items",
                    build_piano_roll_items_value(&state, 0, &piano_roll_selection),
                ),
                (
                    "piano-roll-selection",
                    build_piano_roll_selection_value(&piano_roll_selection),
                ),
                (
                    "velocities",
                    build_param_list(&state, 0, StepParam::Velocity),
                ),
                (
                    "durations",
                    build_param_list(&state, 0, StepParam::Duration),
                ),
                (
                    "transposes",
                    build_param_list(&state, 0, StepParam::Transpose),
                ),
                ("auxas", build_param_list(&state, 0, StepParam::AuxA)),
                ("pans", build_param_list(&state, 0, StepParam::Pan)),
                ("syncs", build_param_list(&state, 0, StepParam::Sync)),
                ("sync-labels", build_sync_labels()),
                ("track-volumes", build_track_volumes(&state)),
                (
                    "effects",
                    build_effects_value(&state, 0, &effect_descriptors, &selected_steps),
                ),
                (
                    "instrument-panel",
                    build_instrument_panel_value(&app, 0, &selected_steps),
                ),
                ("track-params", build_track_params(&state, 0)),
                (
                    "tp-attack",
                    Value::Number(state.pattern.track_params[0].get_attack_ms() as f64),
                ),
                (
                    "tp-release",
                    Value::Number(state.pattern.track_params[0].get_release_ms() as f64),
                ),
                (
                    "tp-swing",
                    Value::Number(state.pattern.track_params[0].get_swing() as f64),
                ),
                (
                    "tp-send",
                    Value::Number(state.pattern.track_params[0].get_send() as f64),
                ),
                (
                    "tp-num-steps",
                    Value::Number(state.pattern.track_params[0].get_num_steps() as f64),
                ),
                (
                    "tp-gate",
                    Value::Bool(state.pattern.track_params[0].is_gate_on()),
                ),
                (
                    "tp-poly",
                    Value::Bool(state.pattern.track_params[0].is_polyphonic()),
                ),
                (
                    "tp-timebase",
                    Value::String(
                        state.pattern.track_params[0]
                            .get_timebase()
                            .label()
                            .to_string(),
                    ),
                ),
                (
                    "tp-swing-resolution",
                    Value::String(
                        state.pattern.track_params[0]
                            .get_swing_resolution()
                            .label()
                            .to_string(),
                    ),
                ),
                (
                    "tp-fts",
                    Value::String(
                        FTS_SCALE_NAMES
                            .get(state.pattern.track_params[0].get_fts_scale())
                            .copied()
                            .unwrap_or("Off")
                            .to_string(),
                    ),
                ),
                (
                    "tp-accumulator",
                    Value::String(selected_accumulator_name(&app, 0)),
                ),
                (
                    "tp-accum-limit",
                    Value::Number(state.pattern.track_params[0].get_accum_limit() as f64),
                ),
                (
                    "tp-accum-mode",
                    Value::String(
                        accum_mode_label(state.pattern.track_params[0].get_accum_mode())
                            .to_string(),
                    ),
                ),
                ("accumulator-options", build_accumulator_options(&app)),
                ("fts-options", build_fts_options()),
                ("accum-mode-options", build_accum_mode_options()),
                ("available-effects", build_available_effects()),
                ("selected-steps", build_selection_value(&selected_steps)),
                (
                    "step-has-plocks",
                    build_step_has_plocks(&state, 0, &effect_descriptors),
                ),
                ("compiling", Value::Bool(false)),
                ("recording", Value::Bool(false)),
                ("cpu-load-pct", Value::Number(0.0)),
                ("master-peak-l", Value::Number(0.0)),
                ("master-peak-r", Value::Number(0.0)),
                (
                    "record-armed",
                    build_record_armed_value(&record_armed.lock().unwrap()),
                ),
                ("playhead-page", Value::Number(0.0)),
                ("sidebar-kind", Value::String("sampler".to_string())),
                ("sidebar-instrument-name", Value::String(String::new())),
                ("sidebar-loaded-preset", Value::String(String::new())),
                ("sidebar-selected-sample", Value::String(String::new())),
                ("sidebar-presets", Value::List(vec![])),
                ("sidebar-preset-tree", Value::List(vec![])),
                ("current-project-name", Value::String(String::new())),
                // Editor mode state (for inline instrument/effect creation/editing)
                ("editor-active", Value::Bool(false)),
                ("editor-error", Value::String(String::new())),
                ("editor-mode", Value::String(String::new())),
                ("editor-buffer-name", Value::String(String::new())),
            ];
            for idx in 0..track_count {
                fields.push((
                    Box::leak(format!("track-peak-{idx}").into_boxed_str()),
                    Value::Number(0.0),
                ));
            }
            for idx in 0..MAX_STEPS {
                fields.push((
                    Box::leak(format!("playhead-active-{idx}").into_boxed_str()),
                    Value::Bool(idx == 0),
                ));
            }
            fields
        },
        false,
    );

    // ── Native functions ──

    // seq-toggle-step — toggle step on current track
    let st = state.clone();
    let ct = current_track.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-step", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-toggle-step: expected step number".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-toggle-step: step {step} out of range").into());
        }
        let track = ct.load(Ordering::Relaxed);
        st.toggle_step_and_clear_plocks(track, step);
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    // seq-set-step-param — set param on current track
    let st = state.clone();
    let ct = current_track.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-step-param", move |args, _ctx| {
        let (Some(Value::Number(step)), Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-step-param: expected (step :param value)".into());
        };
        let step = *step as usize;
        if step >= MAX_STEPS {
            return Err(format!("seq-set-step-param: step {step} out of range").into());
        }
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "aux-a" | "aux_a" | "auxa" | "axa" => StepParam::AuxA,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "sync" | "syn" => StepParam::Sync,
            "speed" => StepParam::Speed,
            other => return Err(format!("seq-set-step-param: unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        st.pattern.step_data[track].set(step, param, val);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let piano_sel = piano_roll_selection.clone();
    let piano_move = piano_roll_move_state.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-piano-roll-action", move |args, ctx| {
        let Some(action) = args.first() else {
            return Err("seq-piano-roll-action: expected action map".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let status = apply_piano_roll_action(&st, track, &piano_sel, &piano_move, action)?;
        if piano_roll_action_mutates_pattern(action) {
            st.publish_scheduler_snapshot();
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        }
        ui_ep.fetch_add(1, Ordering::Relaxed);
        ctx.set_status(status.clone());
        Ok(Value::String(status))
    });

    // seq-set-track — switch current track
    let st = state.clone();
    let ct = current_track.clone();
    runtime.register_native("seq-set-track", move |args, _ctx| {
        let Some(Value::Number(track)) = args.first() else {
            return Err("seq-set-track: expected track number".into());
        };
        let track = *track as usize;
        if track >= st.active_track_count() {
            return Err(format!("seq-set-track: track {track} out of range").into());
        }
        ct.store(track, Ordering::Relaxed);
        Ok(Value::Number(track as f64))
    });

    // seq-set-track-volume — (seq-set-track-volume track-idx volume)
    let st = state.clone();
    let pan_ids = track_pan_ids.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-track-volume", move |args, _ctx| {
        let (Some(Value::Number(track)), Some(Value::Number(vol))) = (args.first(), args.get(1))
        else {
            return Err("seq-set-track-volume: expected (track volume)".into());
        };
        let track = *track as usize;
        let vol = (*vol as f32).clamp(0.0, 1.0);
        st.pattern.track_params[track].set_volume(vol);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        // Push volume to audiograph's stereo panner node
        let pan_ids_lock = pan_ids.lock().unwrap();
        if let Some(&pan_id) = pan_ids_lock.get(track) {
            unsafe {
                sequencer::audiograph::params_push_wrapper(
                    lg_raw,
                    sequencer::audiograph::ParamMsg {
                        idx: sequencer::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: pan_id as u64,
                        fvalue: vol,
                    },
                );
            }
        }
        Ok(Value::Number(vol as f64))
    });

    // seq-set-effect-param — (seq-set-effect-param slot-idx param-idx value)
    let st = state.clone();
    let ct = current_track.clone();
    let descs = effect_descriptors.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-effect-param", move |args, _ctx| {
        let (Some(Value::Number(slot)), Some(Value::Number(param)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-effect-param: expected (slot param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let param_idx = *param as usize;
        let val = *val as f32;

        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("seq-set-effect-param: slot {slot_idx} out of range").into());
        };

        // Clamp to descriptor range if available
        let clamped = descs
            .get(track)
            .and_then(|d| d.get(slot_idx))
            .and_then(|d| d.params.get(param_idx))
            .map(|p| val.clamp(p.min, p.max))
            .unwrap_or(val);

        slot_state.defaults.set(param_idx, clamped);

        // Push to audiograph
        let node_id = slot_state.node_id.load(Ordering::Relaxed);
        if node_id != 0 {
            let idx = slot_state.resolve_node_idx(param_idx);
            // Check for host_control — skip if present
            let skip = descs
                .get(track)
                .and_then(|d| d.get(slot_idx))
                .and_then(|d| d.params.get(param_idx))
                .and_then(|p| p.host_control.as_ref())
                .is_some();
            if !skip {
                unsafe {
                    sequencer::audiograph::params_push_wrapper(
                        lg_raw,
                        sequencer::audiograph::ParamMsg {
                            idx,
                            logical_id: node_id as u64,
                            fvalue: clamped,
                        },
                    );
                }
            }
        }

        // Publish snapshot so the scheduler sees the new default
        // (otherwise it re-applies the old value on next step trigger)
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(clamped as f64))
    });

    // ── Selection natives ──

    // seq-select-step — toggle step in/out of selection
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    runtime.register_native("seq-select-step", move |args, _ctx| {
        let Some(Value::Number(step)) = args.first() else {
            return Err("seq-select-step: expected step number".into());
        };
        let step = *step as usize;
        let mut set = sel.lock().unwrap();
        let was_selected = !set.insert(step);
        if was_selected {
            set.remove(&step);
        }
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(!was_selected))
    });

    // seq-clear-selection
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    runtime.register_native("seq-clear-selection", move |_args, _ctx| {
        sel.lock().unwrap().clear();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Nil)
    });

    // seq-has-selection?
    let sel = selected_steps.clone();
    runtime.register_native("seq-has-selection?", move |_args, _ctx| {
        Ok(Value::Bool(!sel.lock().unwrap().is_empty()))
    });

    // seq-select-all-steps — select every step in the current track pattern
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    runtime.register_native("seq-select-all-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let mut set = sel.lock().unwrap();
        set.clear();
        set.extend(0..num_steps);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(num_steps as f64))
    });

    // seq-delete-selected-steps — clear all selected steps and clear selection
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-delete-selected-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let steps: Vec<usize> = {
            let mut set = sel.lock().unwrap();
            let mut steps: Vec<usize> = set.iter().copied().collect();
            steps.sort_unstable();
            set.clear();
            steps
        };
        for step in &steps {
            st.clear_step_payload(track, *step);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(steps.len() as f64))
    });

    // seq-shift-selected-steps — rotate selected step payloads left/right in place
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-shift-selected-steps", move |args, _ctx| {
        let Some(Value::Number(direction)) = args.first() else {
            return Err("seq-shift-selected-steps: expected direction".into());
        };
        let direction = (*direction).round() as isize;
        if direction == 0 {
            return Ok(Value::Nil);
        }
        let track = ct.load(Ordering::Relaxed);
        let steps: Vec<usize> = {
            let set = sel.lock().unwrap();
            let mut steps: Vec<usize> = set.iter().copied().collect();
            steps.sort_unstable();
            steps
        };
        if steps.is_empty() {
            return Ok(Value::Bool(false));
        }
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let delta = direction.signum();
        let can_shift = if delta < 0 {
            steps[0] > 0
        } else {
            steps[steps.len() - 1] + 1 < num_steps
        };
        if !can_shift {
            return Ok(Value::Bool(false));
        }

        let snapshots: Vec<(usize, sequencer::sequencer::StepSnapshot)> = steps
            .iter()
            .map(|&step| (step, st.capture_step_snapshot(track, step)))
            .collect();
        for &(step, _) in &snapshots {
            st.clear_step_payload(track, step);
        }
        let shifted_steps: Vec<usize> = snapshots
            .iter()
            .map(|(step, _)| (*step as isize + delta) as usize)
            .collect();
        for ((_, snapshot), dst_step) in snapshots.iter().zip(shifted_steps.iter().copied()) {
            st.restore_step_snapshot(track, dst_step, snapshot);
        }
        {
            let mut set = sel.lock().unwrap();
            set.clear();
            set.extend(shifted_steps);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    // seq-set-effect-plock — apply p-lock to ALL selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let fx_ep = fx_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-effect-plock", move |args, _ctx| {
        let (Some(Value::Number(slot)), Some(Value::Number(param)), Some(Value::Number(val))) =
            (args.first(), args.get(1), args.get(2))
        else {
            return Err("seq-set-effect-plock: expected (slot param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let slot_idx = *slot as usize;
        let param_idx = *param as usize;
        let val = *val as f32;
        let chain = &st.pattern.effect_chains[track];
        let Some(slot_state) = chain.get(slot_idx) else {
            return Err(format!("slot {slot_idx} out of range").into());
        };
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            slot_state.plocks.set(step, param_idx, val);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        fx_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    // seq-set-step-param-plock — apply step param p-lock to selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-step-param-plock", move |args, _ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-step-param-plock: expected (:param value)".into());
        };
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
            "aux-a" | "aux_a" | "auxa" | "axa" => StepParam::AuxA,
            "transpose" => StepParam::Transpose,
            "pan" => StepParam::Pan,
            "sync" | "syn" => StepParam::Sync,
            "speed" => StepParam::Speed,
            other => return Err(format!("unknown param :{other}").into()),
        };
        let track = ct.load(Ordering::Relaxed);
        let val = (*val as f32).clamp(param.min(), param.max());
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            st.pattern.step_data[track].set(step, param, val);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    // seq-toggle-play
    let st = state.clone();
    runtime.register_native("seq-toggle-play", move |_args, _ctx| {
        let playing = st.transport.playing.load(Ordering::Relaxed);
        if playing {
            st.transport.playing.store(false, Ordering::Relaxed);
            st.publish_scheduler_snapshot();
        } else {
            st.transport.playing.store(true, Ordering::Relaxed);
            st.transport.playhead.store(0, Ordering::Relaxed);
            st.publish_scheduler_snapshot();
        }
        Ok(Value::Bool(!playing))
    });

    let st = state.clone();
    runtime.register_native("seq-set-bpm", move |args, _ctx| {
        let Some(Value::Number(bpm)) = args.first() else {
            return Err("seq-set-bpm: expected bpm number".into());
        };
        let bpm = (*bpm as u32).clamp(20, 300);
        st.transport.bpm.store(bpm, Ordering::Relaxed);
        st.publish_scheduler_snapshot();
        Ok(Value::Number(bpm as f64))
    });

    // seq-set-track-param — set a track parameter on the current track
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-track-param", move |args, _ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-track-param: expected (:param value)".into());
        };
        let track = ct.load(Ordering::Relaxed);
        let tp = &st.pattern.track_params[track];
        match param_name.as_str() {
            "attack" => {
                let v = (*val as f32).clamp(0.0, 500.0);
                tp.set_attack_ms(v);
                Ok(Value::Number(v as f64))
            }
            "release" => {
                let v = (*val as f32).clamp(0.0, 2000.0);
                tp.set_release_ms(v);
                Ok(Value::Number(v as f64))
            }
            "swing" => {
                let v = (*val as f32).clamp(50.0, 75.0);
                let steps = sel.lock().unwrap();
                if steps.is_empty() {
                    tp.set_swing(v);
                } else {
                    for &step in steps.iter() {
                        st.pattern.swing_plocks[track].set(step, v);
                    }
                }
                Ok(Value::Number(v as f64))
            }
            "num-steps" => {
                let v = (*val as usize).clamp(1, MAX_STEPS);
                tp.set_num_steps(v);
                Ok(Value::Number(v as f64))
            }
            "send" => {
                let v = (*val as f32).clamp(0.0, 1.0);
                tp.set_send(v);
                Ok(Value::Number(v as f64))
            }
            "gate" => {
                let want_on = *val != 0.0;
                if want_on != tp.is_gate_on() {
                    tp.toggle_gate();
                }
                Ok(Value::Bool(tp.is_gate_on()))
            }
            "poly" => {
                let want_on = *val != 0.0;
                if want_on != tp.is_polyphonic() {
                    tp.toggle_polyphonic();
                }
                Ok(Value::Bool(tp.is_polyphonic()))
            }
            other => return Err(format!("seq-set-track-param: unknown param :{other}").into()),
        }
        .map(|v| {
            st.publish_scheduler_snapshot();
            *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
            ui_ep.fetch_add(1, Ordering::Relaxed);
            v
        })
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let accumulator_names_for_native = accumulator_names.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accumulator", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-accumulator: expected string label".into()),
        };
        let names = accumulator_names_for_native.lock().unwrap();
        let idx = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(label))
            .ok_or_else(|| format!("seq-set-accumulator: unknown accumulator '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let tp = &st.pattern.track_params[track];
        tp.set_accumulator_idx(idx);
        if idx < BUILTIN_ACCUMULATOR_NAMES.len() {
            tp.set_script_accumulator_name(None);
            tp.set_accum_limit(builtin_accumulator_default_limit(idx));
        } else {
            tp.set_script_accumulator_name(Some(names[idx].clone()));
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(names[idx].clone()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accum-mode", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-accum-mode: expected string label".into()),
        };
        let mode = ACCUM_MODE_LABELS
            .iter()
            .position(|entry: &&str| entry.eq_ignore_ascii_case(label))
            .map(|idx| idx as u32)
            .ok_or_else(|| format!("seq-set-accum-mode: unknown mode '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_accum_mode(mode);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(accum_mode_label(mode).to_string()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-accum-limit", move |args, _ctx| {
        let Some(Value::Number(limit)) = args.first() else {
            return Err("seq-set-accum-limit: expected number".into());
        };
        let limit = (*limit as f32).clamp(0.0, 127.0);
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_accum_limit(limit);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(limit as f64))
    });

    // seq-double-track-pattern — duplicate current track pattern to double its length
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-double-track-pattern", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let new_len = st.duplicate_track_pattern(track);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(new_len as f64))
    });

    // seq-halve-track-pattern — halve current track pattern length
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-halve-track-pattern", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let new_len = st.halve_track_pattern(track);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(new_len as f64))
    });

    let ct = current_track.clone();
    runtime.register_native(
        "seq-propagate-current-track-to-all-patterns",
        move |_args, ctx| {
            let track = ct.load(Ordering::Relaxed);
            ctx.enqueue_command(HostCommand::Custom {
                name: "propagate-current-track-to-all-patterns".to_string(),
                payload: Value::Number(track as f64),
            });
            Ok(Value::Bool(true))
        },
    );

    // seq-set-timebase — set the default timebase for the current track (by label string)
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-timebase", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-timebase: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let tb = Timebase::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| Timebase::ALL[i])
            .ok_or_else(|| format!("seq-set-timebase: unknown timebase '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_timebase(tb);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-fts", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-fts: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let scale_idx = FTS_SCALE_NAMES
            .iter()
            .position(|scale| scale.to_ascii_lowercase() == normalized)
            .ok_or_else(|| format!("seq-set-fts: unknown scale '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        st.pattern.track_params[track].set_fts_scale(scale_idx);
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(FTS_SCALE_NAMES[scale_idx].to_string()))
    });

    // seq-plock-timebase — set a timebase p-lock on selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-plock-timebase", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-plock-timebase: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let tb = Timebase::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| Timebase::ALL[i])
            .ok_or_else(|| format!("seq-plock-timebase: unknown timebase '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let steps = sel.lock().unwrap();
        for &step in steps.iter() {
            st.pattern.timebase_plocks[track].set(step, tb);
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    // seq-set-swing-resolution — set the default swing resolution for the current track (by label string)
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-set-swing-resolution", move |args, _ctx| {
        let label = match args.first() {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err("seq-set-swing-resolution: expected string label".into()),
        };
        let normalized = label.to_ascii_lowercase();
        let resolution = SwingResolution::LABELS
            .iter()
            .position(|l| l.to_ascii_lowercase() == normalized)
            .map(|i| SwingResolution::ALL[i])
            .ok_or_else(|| format!("seq-set-swing-resolution: unknown resolution '{label}'"))?;
        let track = ct.load(Ordering::Relaxed);
        let steps = sel.lock().unwrap();
        if steps.is_empty() {
            st.pattern.track_params[track].set_swing_resolution(resolution);
        } else {
            for &step in steps.iter() {
                st.pattern.swing_resolution_plocks[track].set(step, resolution);
            }
        }
        st.publish_scheduler_snapshot();
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(resolution.label().to_string()))
    });

    let ui_ep = ui_epoch.clone();
    let auto_follow_override = auto_follow_override_until.clone();
    runtime.register_native("seq-pause-auto-follow", move |_args, _ctx| {
        *auto_follow_override.lock().unwrap() = Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(false))
    });

    // seq-toggle-record — toggle recording mode (requires at least one armed track)
    let rec = recording.clone();
    let ra = record_armed.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-record", move |_args, _ctx| {
        let any_armed = ra.lock().unwrap().iter().any(|a| *a);
        if any_armed {
            let was = rec.load(Ordering::Relaxed);
            rec.store(!was, Ordering::Relaxed);
            ui_ep.fetch_add(1, Ordering::Relaxed);
            Ok(Value::Bool(!was))
        } else {
            Ok(Value::Bool(false))
        }
    });

    // seq-toggle-record-arm — toggle record arm for a given track index
    let ra = record_armed.clone();
    let rec = recording.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-toggle-record-arm", move |args, _ctx| {
        let Some(Value::Number(track_idx)) = args.first() else {
            return Err("seq-toggle-record-arm: expected track index".into());
        };
        let track = *track_idx as usize;
        let mut armed = ra.lock().unwrap();
        if track < armed.len() {
            armed[track] = !armed[track];
            // If no tracks armed, turn off recording
            if !armed.iter().any(|a| *a) {
                rec.store(false, Ordering::Relaxed);
            }
            ui_ep.fetch_add(1, Ordering::Relaxed);
            Ok(Value::Bool(armed[track]))
        } else {
            Ok(Value::Bool(false))
        }
    });

    // seq-search-samples — recursively search samples/ for .wav files matching a query
    // Pre-scan the sample tree once and cache it for fast filtering.
    let sample_index: Vec<(String, String, String)> = {
        let mut index = Vec::new();
        let samples_dir = std::path::Path::new("samples");
        if samples_dir.is_dir() {
            let mut stack = vec![samples_dir.to_path_buf()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if let Some(ext) = path.extension() {
                        if ext.eq_ignore_ascii_case("wav") {
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let parent = path
                                .parent()
                                .and_then(|p| p.file_name())
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let full_path = path.to_string_lossy().to_string();
                            index.push((name, parent, full_path));
                        }
                    }
                }
            }
            index.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        }
        eprintln!("metal_seq: indexed {} samples", index.len());
        index
    };
    runtime.register_native("seq-search-samples", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.to_lowercase(),
            _ => String::new(),
        };
        let results: Vec<Rc<RefCell<Value>>> = sample_index
            .iter()
            .filter(|(name, _, _)| query.is_empty() || name.to_lowercase().contains(&query))
            .take(100) // cap results for UI performance
            .map(|(name, parent, full_path)| {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(name.clone()))),
                );
                map.insert(
                    "parent".to_string(),
                    Rc::new(RefCell::new(Value::String(parent.clone()))),
                );
                map.insert(
                    "path".to_string(),
                    Rc::new(RefCell::new(Value::String(full_path.clone()))),
                );
                Rc::new(RefCell::new(Value::Map(map)))
            })
            .collect();
        Ok(Value::List(results))
    });

    let sample_tree_nodes = build_sample_tree_node(std::path::Path::new("samples"));
    let sample_tree = sample_tree_nodes_to_value(&sample_tree_nodes);
    eprintln!("metal_seq: sample tree built");
    runtime.register_native(
        "seq-sample-tree",
        move |_args, _ctx| Ok(sample_tree.clone()),
    );
    runtime.register_native("seq-filter-sample-tree", move |args, _ctx| {
        let query_lower = match args.first() {
            Some(Value::String(s)) => s.trim().to_lowercase(),
            _ => String::new(),
        };
        let filtered = filter_sample_tree_nodes(&sample_tree_nodes, &query_lower);
        Ok(sample_tree_nodes_to_value(&filtered))
    });
    runtime.register_native("seq-project-tree", move |args, _ctx| {
        let query = match args.first() {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        Ok(build_project_tree(&query))
    });
    runtime.register_native("seq-saved-instruments", move |_args, _ctx| {
        Ok(Value::List(
            sequencer::lisp_effect::list_saved_instruments()
                .into_iter()
                .map(|name| Rc::new(RefCell::new(Value::String(name))))
                .collect(),
        ))
    });

    // 4. Create editor with Metal backend
    let init_src = std::fs::read_to_string("init.lisp")
        .or_else(|_| std::fs::read_to_string("../eseqlisp/init.lisp"))
        .unwrap_or_default();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
        },
    );

    let _ = editor.open_or_create_file_buffer("metal-seq-grid.lisp");
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    push_project_scratch_to_named_buffer(&mut editor, &app);

    let mut backend =
        MetalBackend::new_with_size(1100, 700).map_err(|_| "Metal backend creation failed")?;
    backend
        .initialize()
        .map_err(|_| "Metal backend init failed")?;

    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
    }

    // 5. Metal event loop
    let frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let mut last_render_at = Instant::now() - frame_interval;
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;

    // Inline editor session state (instrument/effect creation/editing)
    let mut editor_buffer_name: Option<String> = None;
    let mut editor_mode: Option<String> = None;
    let mut editor_effect_name: Option<String> = None; // original effect name (without .lisp)
    let mut editor_effect_slot: Option<usize> = None; // effect slot index for hot-swap

    let mut prev_playing = false;
    let mut prev_bpm: u32 = 0;
    let mut prev_playhead: u32 = u32::MAX;
    let mut prev_transport_playhead: u32 = u32::MAX;
    let mut prev_pattern_epoch: u64 = 0;
    let mut prev_snapshot_version: u64 = 0;
    let mut prev_current_track: usize = usize::MAX;
    let mut prev_cpu_load_bits: u32 = u32::MAX;
    let mut prev_peak_l_level = -1.0f64;
    let mut prev_peak_r_level = -1.0f64;
    let mut prev_track_peak_levels: Vec<f64> = Vec::new();
    let mut prev_ui_epoch: usize = 0;
    let mut prev_fx_epoch: usize = 0;
    let mut prev_auto_follow = true;
    let mut watched_sampler_voice_track: Option<usize> = None;
    let mut watched_sampler_voice_ids: Vec<i32> = Vec::new();
    let mut cached_peak_l_level = 0.0f64;
    let mut cached_peak_r_level = 0.0f64;
    let mut cached_track_peak_levels = vec![0.0; track_names.len()];
    let mut last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL;
    let mut last_cpu_ui_poll_at = Instant::now() - CPU_UI_POLL_INTERVAL;
    let mut last_voice_count_log_at = Instant::now() - VOICE_COUNT_LOG_INTERVAL;
    let log_voice_counts = std::env::var_os("TINYSEQ_LOG_VOICE_COUNTS").is_some();
    let mut cached_cpu_load_bits: u32 = 0.0f32.to_bits();

    eprintln!("metal_seq: entering event loop");
    let mut ui_loop_stats = UiLoopStats::new();

    loop {
        pull_named_scratch_buffer_into_project(&editor, &mut app);
        editor.update_timers();
        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);
        if log_voice_counts && last_voice_count_log_at.elapsed() >= VOICE_COUNT_LOG_INTERVAL {
            log_active_voice_counts(&state, &track_names);
            last_voice_count_log_at = Instant::now();
        }

        let sdf_animation_active =
            eseqlisp::widget_render::sdf_widget::sdf_visual_animations_active(
                backend.time_seconds(),
            );
        if sdf_animation_active {
            editor.mark_needs_redraw();
            let elapsed = last_render_at.elapsed();
            if elapsed < frame_interval {
                std::thread::sleep(frame_interval - elapsed);
            }
            let frame_build_started = Instant::now();
            let tiled_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            let frame_build_elapsed = frame_build_started.elapsed();
            let render_started = Instant::now();
            backend
                .render_tiled(&tiled_frame)
                .map_err(|_| "render failed")?;
            let render_elapsed = render_started.elapsed();
            ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed);
            editor.clear_needs_redraw();
            last_render_at = Instant::now();
            continue;
        }

        // 1. Poll events FIRST
        let playing_now = state.transport.playing.load(Ordering::Relaxed);
        let timeout = if playing_now {
            frame_interval
                .saturating_sub(last_render_at.elapsed())
                .max(Duration::from_millis(8))
        } else if editor.needs_redraw() {
            Duration::from_millis(4)
        } else {
            Duration::from_millis(50)
        };
        if let Some(event) = backend.poll_event(timeout) {
            let event_started = Instant::now();
            match event {
                Event::Key(raw_key) => {
                    if handle_metal_command_shortcut(
                        &mut editor,
                        &raw_key,
                        &state,
                        &current_track,
                        &selected_steps,
                        &step_clipboard,
                    ) {
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    let key = normalize_command_shortcuts(raw_key);
                    if should_toggle_play_on_space(&editor, &key) {
                        let _ = editor.runtime_mut().eval_str("(seq-toggle-play)");
                        editor.refresh_runtime_side_effects();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    // Intercept keyboard for live recording when any track is armed
                    let any_armed = record_armed.lock().unwrap().iter().any(|a| *a);
                    let intercepted =
                        if any_armed && should_route_to_live_keyboard(&editor, &key, &held_notes) {
                            handle_recording_key(
                                &key,
                                &state,
                                &record_armed,
                                &recording,
                                &keyboard_tx,
                                &keyboard_octave,
                                &current_track,
                                &held_notes,
                            )
                        } else {
                            false
                        };
                    // Only pass Press events to the editor (Release is only for note-off)
                    if !intercepted && key.kind == crossterm::event::KeyEventKind::Press {
                        editor.handle_key(key);
                    }
                }
                Event::Mouse(mouse) => {
                    let (precise_col, precise_row) = backend
                        .take_last_precise_mouse()
                        .unwrap_or((mouse.column as f32, mouse.row as f32));
                    if matches!(
                        mouse.kind,
                        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left)
                    ) {
                        pending_drag = Some((Event::Mouse(mouse), (precise_col, precise_row)));
                    } else {
                        if matches!(mouse.kind, crossterm::event::MouseEventKind::Up(_)) {
                            pending_drag = None;
                        }
                        editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
                    }
                }
                Event::Resize(_, _) => editor.mark_needs_redraw(),
                _ => {}
            }
            ui_loop_stats.note_event(event_started.elapsed());
        }

        // Touchpad gestures
        let gestures_started = Instant::now();
        while let Some((delta, (precise_col, precise_row))) = backend.take_pending_magnify() {
            editor.handle_tiled_touchpad_magnify(precise_col, precise_row, 0, delta);
        }
        while let Some(((delta_x, delta_y), (precise_col, precise_row))) =
            backend.take_pending_scroll()
        {
            let widget_handled =
                editor.handle_tiled_touchpad_scroll(precise_col, precise_row, 0, delta_x, delta_y);
            if widget_handled {
                continue;
            }

            // In UI mode, apply pixel deltas directly for smooth sub-cell scrolling.
            if editor.is_ui_scroll_mode() {
                let scroll_speed = 0.05; // cells per pixel-delta
                let delta_cells_y = delta_y * scroll_speed;
                let delta_cells_x = delta_x * scroll_speed;
                editor.apply_smooth_widget_scroll(delta_cells_x, delta_cells_y);
                continue;
            }

            scroll_accum_y += delta_y;
            let line_px = backend.viewport_size().1.max(1) as f32 / (rows.max(1) as f32);
            let threshold = line_px.max(20.0);
            while scroll_accum_y > threshold {
                scroll_accum_y -= threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollUp,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            while scroll_accum_y < -threshold {
                scroll_accum_y += threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollDown,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            scroll_accum_x += delta_x;
            while scroll_accum_x > threshold {
                scroll_accum_x -= threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollLeft,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
            while scroll_accum_x < -threshold {
                scroll_accum_x += threshold;
                let mouse = crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::ScrollRight,
                    column: precise_col as u16,
                    row: precise_row as u16,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                };
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
        }

        // Flush the latest coalesced drag every loop iteration. Waiting for the
        // render boundary makes slider/knob drags feel stale and can drop the
        // final motion segment if mouse-up lands before the next frame.
        if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
            editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
        }
        ui_loop_stats.note_gestures(gestures_started.elapsed());

        // 1b. Drain host commands (sample browser etc.)
        let host_commands_started = Instant::now();
        for command in editor.drain_host_commands() {
            if let HostCommand::Custom { name, payload } = command {
                match name.as_str() {
                    "audition-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        if let Some(path_str) = path_str {
                            let path = Path::new(&path_str);
                            let track = current_track.load(Ordering::Relaxed);
                            match sequencer::sampler::load_wav_buffer(lg_raw, path) {
                                Ok((new_buffer_id, new_name)) => {
                                    register_waveform_sample(path);
                                    app.graph_controller()
                                        .send_buffer_to_all_voices(track, new_buffer_id);
                                    app.graph.track_buffer_ids[track] = new_buffer_id;
                                    app.tracks[track] = new_name.clone();
                                    app.register_sample_path(&new_name, path.to_path_buf());
                                    if track < app.sampler_paths.len() {
                                        app.sampler_paths[track] = Some(path.to_path_buf());
                                    }
                                    track_names[track] = new_name.clone();
                                    // Update reactive state
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&track_names),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, track, &selected_steps),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Audition: {new_name}"
                                    )));
                                }
                                Err(e) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error loading sample: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        if let Some(path_str) = path_str {
                            let path = Path::new(&path_str);
                            match app.graph_controller().add_track(path) {
                                Ok(idx) => {
                                    register_waveform_sample(path);
                                    current_track.store(idx, Ordering::Relaxed);
                                    let new_name = app.tracks[idx].clone();
                                    track_names.push(new_name.clone());
                                    // Update pan IDs for new track
                                    track_pan_ids
                                        .lock()
                                        .unwrap()
                                        .push(app.graph.track_node_ids[idx].pan_id);
                                    // Extend record_armed for new track
                                    record_armed.lock().unwrap().push(false);
                                    // Update reactive state
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "num-tracks",
                                        Value::Number(track_names.len() as f64),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "current-track",
                                        Value::Number(idx as f64),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&track_names),
                                    );
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, idx));
                                    sync_step_param_lists(rt, &state, idx);
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-volumes",
                                        build_track_volumes(&state),
                                    );
                                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            idx,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, idx, &selected_steps),
                                    );
                                    *accumulator_names.lock().unwrap() =
                                        build_accumulator_names(&app);
                                    sync_track_params(rt, &app, &state, idx, &selected_steps);
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            idx,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Added track {}: {new_name}",
                                        idx + 1
                                    )));
                                }
                                Err(e) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error adding track: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(name) = &*cell.borrow() {
                                    match app.add_saved_instrument_track_sync(name) {
                                        Ok(idx) => {
                                            current_track.store(idx, Ordering::Relaxed);
                                            let new_name = app.tracks[idx].clone();
                                            track_names.push(new_name.clone());
                                            track_pan_ids
                                                .lock()
                                                .unwrap()
                                                .push(app.graph.track_node_ids[idx].pan_id);
                                            record_armed.lock().unwrap().push(false);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "num-tracks",
                                                Value::Number(track_names.len() as f64),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "current-track",
                                                Value::Number(idx as f64),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-names",
                                                build_track_names(&track_names),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "steps",
                                                build_steps_value(&state, idx),
                                            );
                                            sync_step_param_lists(rt, &state, idx);
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-volumes",
                                                build_track_volumes(&state),
                                            );
                                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                                            rt.set_reactive(
                                                "SEQ",
                                                "effects",
                                                build_effects_value(
                                                    &state,
                                                    idx,
                                                    &app.graph.effect_descriptors,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "instrument-panel",
                                                build_instrument_panel_value(
                                                    &app,
                                                    idx,
                                                    &selected_steps,
                                                ),
                                            );
                                            *accumulator_names.lock().unwrap() =
                                                build_accumulator_names(&app);
                                            sync_track_params(
                                                rt,
                                                &app,
                                                &state,
                                                idx,
                                                &selected_steps,
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "step-has-plocks",
                                                build_step_has_plocks(
                                                    &state,
                                                    idx,
                                                    &app.graph.effect_descriptors,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added instrument track {}: {new_name}",
                                                idx + 1
                                            )));
                                        }
                                        Err(e) => {
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Error adding instrument track: {e}"
                                            )));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "delete-track" => {
                        let track = match &payload {
                            Value::Map(map) => {
                                map.get("track").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        }
                        .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                        let request_id = if state.is_playing() {
                            let request_id = state.request_track_delete_boundary(track);
                            let wait_deadline = Instant::now() + Duration::from_millis(250);
                            while !state.topology_edit_ready(request_id)
                                && Instant::now() < wait_deadline
                            {
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            if !state.topology_edit_ready(request_id) {
                                state.complete_topology_edit(request_id);
                                state.publish_scheduler_snapshot();
                                editor.handle_host_event(HostEvent::Status(
                                    "Delete timed out waiting for playback boundary".to_string(),
                                ));
                                continue;
                            }
                            Some(request_id)
                        } else {
                            None
                        };

                        match app.graph_controller().delete_track(track) {
                            Ok(new_idx) => {
                                if let Some(request_id) = request_id {
                                    state.complete_topology_edit(request_id);
                                    state.publish_scheduler_snapshot();
                                }
                                current_track.store(new_idx, Ordering::Relaxed);
                                *track_pan_ids.lock().unwrap() = app
                                    .graph
                                    .track_node_ids
                                    .iter()
                                    .map(|ids| ids.pan_id)
                                    .collect();
                                cached_track_peak_levels = read_track_peak_levels(
                                    app.graph.lg,
                                    &track_pan_ids.lock().unwrap(),
                                );
                                last_meter_poll_at = Instant::now();
                                *record_armed.lock().unwrap() = app.graph.record_armed.clone();

                                let rt = editor.runtime_mut();
                                sync_track_topology_state(
                                    rt,
                                    &app,
                                    &state,
                                    &mut track_names,
                                    new_idx,
                                    &selected_steps,
                                    &piano_roll_selection,
                                    &accumulator_names,
                                    &record_armed,
                                    &cached_track_peak_levels,
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Deleted track {}",
                                    track + 1
                                )));
                            }
                            Err(e) => {
                                if let Some(request_id) = request_id {
                                    state.complete_topology_edit(request_id);
                                    state.publish_scheduler_snapshot();
                                }
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting track: {e}"
                                )));
                            }
                        }
                    }
                    "load-instrument-preset" => {
                        if let Value::Map(ref map) = payload {
                            let preset_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(name) => Some(name.clone()),
                                    _ => None,
                                });
                            if let Some(preset_name) = preset_name {
                                let track = current_track.load(Ordering::Relaxed);
                                match load_instrument_preset_into_track(
                                    &mut app,
                                    track,
                                    &preset_name,
                                ) {
                                    Ok(()) => {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "instrument-panel",
                                            build_instrument_panel_value(
                                                &app,
                                                track,
                                                &selected_steps,
                                            ),
                                        );
                                        sync_sidebar_browser(rt, &app, track);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Loaded preset '{preset_name}'"
                                        )));
                                    }
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error loading preset: {e}"
                                        )));
                                    }
                                }
                            }
                        }
                    }
                    "save-preset" => {
                        if let Value::Map(ref map) = payload {
                            let preset_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                });
                            let overwrite = map
                                .get("overwrite")
                                .map(|cell| match &*cell.borrow() {
                                    Value::Bool(b) => *b,
                                    _ => false,
                                })
                                .unwrap_or(false);
                            if let Some(name) = preset_name {
                                let name = name.trim().to_string();
                                if name.is_empty() {
                                    editor.handle_host_event(HostEvent::Status(
                                        "Preset name cannot be empty".to_string(),
                                    ));
                                } else {
                                    let track = current_track.load(Ordering::Relaxed);
                                    app.ui.cursor_track = track;
                                    app.save_current_track_as_preset(&name, overwrite);
                                    // Refresh sidebar presets list
                                    let rt = editor.runtime_mut();
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Saved preset '{name}'"
                                    )));
                                }
                            }
                        }
                    }
                    "overwrite-preset" => {
                        let track = current_track.load(Ordering::Relaxed);
                        app.ui.cursor_track = track;
                        app.overwrite_loaded_preset();
                        let rt = editor.runtime_mut();
                        sync_sidebar_browser(rt, &app, track);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                    "save-project" => {
                        let requested_name = if let Value::Map(ref map) = payload {
                            map.get("name").and_then(|cell| match &*cell.borrow() {
                                Value::String(name) => Some(name.clone()),
                                _ => None,
                            })
                        } else {
                            None
                        };
                        match app.save_project_with_name(requested_name.as_deref()) {
                            Ok(save_name) => {
                                let rt = editor.runtime_mut();
                                sync_project_state(rt, &app);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Saved project '{save_name}'"
                                )));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error saving project: {error}"
                                )));
                            }
                        }
                    }
                    "load-project" => {
                        let requested_name = if let Value::Map(ref map) = payload {
                            map.get("name").and_then(|cell| match &*cell.borrow() {
                                Value::String(name) => Some(name.clone()),
                                _ => None,
                            })
                        } else {
                            None
                        };
                        let Some(project_name) =
                            requested_name.filter(|name| !name.trim().is_empty())
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Error loading project: missing project name".to_string(),
                            ));
                            continue;
                        };
                        eprintln!("metal_seq: host load-project name={project_name}");
                        match app.queue_project_load_named(&project_name) {
                            Ok(()) => {
                                eprintln!("metal_seq: queued project load name={project_name}");
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Opening project '{project_name}'..."
                                )));
                            }
                            Err(error) => {
                                eprintln!(
                                    "metal_seq: queue project load failed name={} error={}",
                                    project_name, error
                                );
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error loading project: {error}"
                                )));
                            }
                        }
                    }
                    "set-instrument-param" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(user_val)) = (param_idx, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetInstrumentParam {
                                            track,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(label)) = (param_idx, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .map(|d| &d.kind)
                                {
                                    if let Some(selected_idx) =
                                        labels.iter().position(|item| item == &label)
                                    {
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentParam {
                                                track,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-instrument-plock" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(user_val)) = (param_idx, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let stored = desc.clamp(desc.user_input_to_stored(user_val));
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetInstrumentPlockMulti {
                                            track,
                                            steps,
                                            param_idx,
                                            value: stored,
                                        },
                                    );
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(param_idx), Some(label)) = (param_idx, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(sequencer::effects::ParamKind::Enum { labels }) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .map(|d| &d.kind)
                                {
                                    if let Some(selected_idx) =
                                        labels.iter().position(|item| item == &label)
                                    {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentPlockMulti {
                                                track,
                                                steps,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-effect-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(label)) =
                                (slot_idx, param_idx, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let selected_idx = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|d| d.get(slot_idx))
                                    .and_then(|d| d.params.get(param_idx))
                                    .and_then(|p| match &p.kind {
                                        sequencer::effects::ParamKind::Enum { labels } => {
                                            labels.iter().position(|item| item == &label)
                                        }
                                        _ => None,
                                    })
                                    .or_else(|| {
                                        (slot_idx == 1 && param_idx == 2).then(|| {
                                            sequencer::effects::SyncDivision::ALL
                                                .iter()
                                                .position(|div| div.label() == label)
                                        })?
                                    });
                                if let Some(selected_idx) = selected_idx {
                                    let is_host_sidechain = matches!(
                                        app.graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .and_then(|d| d.params.get(param_idx))
                                            .and_then(|p| p.host_control.as_ref()),
                                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                                    );
                                    if is_host_sidechain {
                                        app.apply_effect_sidechain_selection(
                                            track,
                                            slot_idx,
                                            param_idx,
                                            selected_idx,
                                        );
                                        if let Some(slot) = app
                                            .state
                                            .pattern
                                            .effect_chains
                                            .get(track)
                                            .and_then(|chain| chain.get(slot_idx))
                                        {
                                            slot.defaults.set(param_idx, selected_idx as f32);
                                        }
                                        app.state.publish_scheduler_snapshot();
                                    } else {
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetEffectParam {
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-effect-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let slot_idx =
                                map.get("slot-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(label)) =
                                (slot_idx, param_idx, label)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let selected_idx = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|d| d.get(slot_idx))
                                    .and_then(|d| d.params.get(param_idx))
                                    .and_then(|p| match &p.kind {
                                        sequencer::effects::ParamKind::Enum { labels } => {
                                            labels.iter().position(|item| item == &label)
                                        }
                                        _ => None,
                                    })
                                    .or_else(|| {
                                        (slot_idx == 1 && param_idx == 2).then(|| {
                                            sequencer::effects::SyncDivision::ALL
                                                .iter()
                                                .position(|div| div.label() == label)
                                        })?
                                    });
                                if let Some(selected_idx) = selected_idx {
                                    let is_host_sidechain = matches!(
                                        app.graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .and_then(|d| d.params.get(param_idx))
                                            .and_then(|p| p.host_control.as_ref()),
                                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                                    );
                                    if is_host_sidechain {
                                        app.apply_effect_sidechain_selection(
                                            track,
                                            slot_idx,
                                            param_idx,
                                            selected_idx,
                                        );
                                        if let Some(slot) = app
                                            .state
                                            .pattern
                                            .effect_chains
                                            .get(track)
                                            .and_then(|chain| chain.get(slot_idx))
                                        {
                                            slot.defaults.set(param_idx, selected_idx as f32);
                                        }
                                        app.state.publish_scheduler_snapshot();
                                    } else {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetEffectPlockMulti {
                                                track,
                                                slot_idx,
                                                steps,
                                                param_idx,
                                                value: selected_idx as f32,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-instrument-base-note" => {
                        if let Value::Map(ref map) = payload {
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let Some(value) = value {
                                let track = current_track.load(Ordering::Relaxed);
                                let clamped = value.clamp(-48.0, 48.0);
                                ui::apply_command(
                                    &mut app,
                                    ui::AppCommand::SetInstrumentBaseNoteOffset {
                                        track,
                                        value: clamped,
                                    },
                                );
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "add-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                    if let Some(slot_idx) = app.next_free_custom_slot() {
                                        app.start_effect_compile(&effect_name, slot_idx);
                                        editor.runtime_mut().set_reactive(
                                            "SEQ",
                                            "compiling",
                                            Value::Bool(true),
                                        );
                                    } else {
                                        editor.handle_host_event(HostEvent::Status(
                                            "No free effect slots available".to_string(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    "switch-pattern" => {
                        if let Value::Map(ref map) = payload {
                            let idx = map.get("idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            if let Some(idx) = idx {
                                let num_tracks = app.tracks.len();
                                if let Some(sample_ids) = app.state.switch_pattern(
                                    idx,
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                ) {
                                    app.graph_controller().apply_sample_ids(&sample_ids);
                                    app.push_all_restored_defaults();
                                    let ct = current_track.load(Ordering::Relaxed);
                                    let rt = editor.runtime_mut();
                                    sync_track_name_state(rt, &mut track_names, &app);
                                    sync_pattern_state(rt, &state);
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                                    sync_step_param_lists(rt, &state, ct);
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-volumes",
                                        build_track_volumes(&state),
                                    );
                                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            ct,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, ct, &selected_steps),
                                    );
                                    *accumulator_names.lock().unwrap() =
                                        build_accumulator_names(&app);
                                    sync_track_params(rt, &app, &state, ct, &selected_steps);
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            ct,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_sidebar_browser(rt, &app, ct);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "propagate-current-track-to-all-patterns" => {
                        let track = match payload {
                            Value::Number(n) => n as usize,
                            _ => current_track.load(Ordering::Relaxed),
                        };
                        let num_patterns =
                            state.pattern.num_patterns.load(Ordering::Relaxed) as usize;
                        if track >= app.tracks.len() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track {} is out of range",
                                track + 1
                            )));
                        } else if num_patterns <= 1 {
                            editor.handle_host_event(HostEvent::Status(
                                "Nothing to propagate: only one pattern exists".to_string(),
                            ));
                        } else if app.state.propagate_track_to_all_patterns(
                            track,
                            app.tracks.len(),
                            &app.graph.track_buffer_ids,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Propagated track {} to {} patterns",
                                track + 1,
                                num_patterns
                            )));
                        } else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Failed to propagate track {}",
                                track + 1
                            )));
                        }
                    }
                    "clone-pattern" => {
                        let num_tracks = app.tracks.len();
                        let new_idx = app.state.clone_pattern(
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        );
                        let rt = editor.runtime_mut();
                        sync_pattern_state(rt, &state);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cloned pattern {}",
                            new_idx + 1
                        )));
                    }
                    "delete-pattern" => {
                        let num_tracks = app.tracks.len();
                        if let Some(sample_ids) = app.state.delete_pattern(
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            app.graph_controller().apply_sample_ids(&sample_ids);
                            app.push_all_restored_defaults();
                            let ct = current_track.load(Ordering::Relaxed);
                            let rt = editor.runtime_mut();
                            sync_track_name_state(rt, &mut track_names, &app);
                            sync_pattern_state(rt, &state);
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                            sync_step_param_lists(rt, &state, ct);
                            rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            rt.set_reactive(
                                "SEQ",
                                "effects",
                                build_effects_value(
                                    &state,
                                    ct,
                                    &app.graph.effect_descriptors,
                                    &selected_steps,
                                ),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            sync_track_params(rt, &app, &state, ct, &selected_steps);
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                            );
                            sync_sidebar_browser(rt, &app, ct);
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    // ── Inline instrument/effect editor commands ──
                    "enter-new-instrument-editor" => {
                        let temp_path =
                            std::path::PathBuf::from("instruments/.untitled-instrument.lisp");
                        std::fs::create_dir_all("instruments").ok();
                        if let Err(e) =
                            std::fs::write(&temp_path, sequencer::lisp_effect::INSTRUMENT_TEMPLATE)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write template: {e}"
                            )));
                        } else {
                            let buf_name =
                                match editor.create_file_buffer(&temp_path, BufferMode::DGenLisp) {
                                    Ok(name) => name,
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to create buffer: {e:?}"
                                        )));
                                        continue;
                                    }
                                };
                            editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                            editor_buffer_name = Some(buf_name.clone());
                            editor_mode = Some("new-instrument".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                            rt.set_reactive(
                                "SEQ",
                                "editor-mode",
                                Value::String("new-instrument".to_string()),
                            );
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                        }
                    }

                    "save-new-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.trim().to_string();
                                    if inst_name.is_empty() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String("Name cannot be empty".to_string()),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    // Write to final path
                                    let final_path = format!("instruments/{inst_name}.lisp");
                                    if let Err(e) = std::fs::write(&final_path, &source) {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("Failed to save: {e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    // Try to compile FIRST — stay in editor on failure
                                    match app.add_saved_instrument_track_sync(&inst_name) {
                                        Ok(idx) => {
                                            // Success — clean up temp file, close editor
                                            let _ = std::fs::remove_file(
                                                "instruments/.untitled-instrument.lisp",
                                            );
                                            editor
                                                .swap_buffer_in_tile_showing(&buf_name, "*metal*");
                                            editor.remove_buffer_by_name(&buf_name);
                                            editor_buffer_name = None;
                                            editor_mode = None;

                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-active",
                                                Value::Bool(false),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-mode",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-buffer-name",
                                                Value::String(String::new()),
                                            );

                                            current_track.store(idx, Ordering::Relaxed);
                                            let new_name = app.tracks[idx].clone();
                                            track_names.push(new_name.clone());
                                            track_pan_ids
                                                .lock()
                                                .unwrap()
                                                .push(app.graph.track_node_ids[idx].pan_id);
                                            record_armed.lock().unwrap().push(false);
                                            rt.set_reactive(
                                                "SEQ",
                                                "num-tracks",
                                                Value::Number(track_names.len() as f64),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "current-track",
                                                Value::Number(idx as f64),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-names",
                                                build_track_names(&track_names),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "steps",
                                                build_steps_value(&state, idx),
                                            );
                                            sync_step_param_lists(rt, &state, idx);
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-volumes",
                                                build_track_volumes(&state),
                                            );
                                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                                            rt.set_reactive(
                                                "SEQ",
                                                "effects",
                                                build_effects_value(
                                                    &state,
                                                    idx,
                                                    &app.graph.effect_descriptors,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "instrument-panel",
                                                build_instrument_panel_value(
                                                    &app,
                                                    idx,
                                                    &selected_steps,
                                                ),
                                            );
                                            *accumulator_names.lock().unwrap() =
                                                build_accumulator_names(&app);
                                            sync_track_params(
                                                rt,
                                                &app,
                                                &state,
                                                idx,
                                                &selected_steps,
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "step-has-plocks",
                                                build_step_has_plocks(
                                                    &state,
                                                    idx,
                                                    &app.graph.effect_descriptors,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Created instrument '{inst_name}' and added track {}",
                                                idx + 1
                                            )));
                                        }
                                        Err(e) => {
                                            // Compile failed — stay in editor, show error
                                            // Clean up the written file so stale source doesn't linger
                                            let _ = std::fs::remove_file(&final_path);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!("{e}")),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    "enter-edit-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    let file_path = std::path::PathBuf::from(format!(
                                        "instruments/{inst_name}.lisp"
                                    ));
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Instrument file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let buf_name = match editor
                                        .create_file_buffer(&file_path, BufferMode::DGenLisp)
                                    {
                                        Ok(name) => name,
                                        Err(e) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to open buffer: {e:?}"
                                            )));
                                            continue;
                                        }
                                    };
                                    editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-instrument".to_string());
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String("edit-instrument".to_string()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(buf_name),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }

                    "update-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    // Save to file
                                    let final_path = format!("instruments/{inst_name}.lisp");
                                    if let Err(e) = std::fs::write(&final_path, &source) {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("Failed to save: {e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    // Try hot-swap FIRST — stay in editor on failure
                                    app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                    match app
                                        .replace_current_custom_instrument_sync(&inst_name, &source)
                                    {
                                        Ok(()) => {
                                            // Success — close editor
                                            editor
                                                .swap_buffer_in_tile_showing(&buf_name, "*metal*");
                                            editor.remove_buffer_by_name(&buf_name);
                                            editor_buffer_name = None;
                                            editor_mode = None;

                                            let ct = current_track.load(Ordering::Relaxed);
                                            track_names[ct] = inst_name.clone();
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-active",
                                                Value::Bool(false),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-mode",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-buffer-name",
                                                Value::String(String::new()),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-names",
                                                build_track_names(&track_names),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "instrument-panel",
                                                build_instrument_panel_value(
                                                    &app,
                                                    ct,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "effects",
                                                build_effects_value(
                                                    &state,
                                                    ct,
                                                    &app.graph.effect_descriptors,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Hot-swapped instrument '{inst_name}'"
                                            )));
                                        }
                                        Err(e) => {
                                            // Compile failed — stay in editor, show error
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!("{e}")),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    "enter-new-effect-editor" => {
                        let temp_path = std::path::PathBuf::from("effects/.untitled-effect.lisp");
                        std::fs::create_dir_all("effects").ok();
                        if let Err(e) =
                            std::fs::write(&temp_path, sequencer::lisp_effect::EFFECT_TEMPLATE)
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write template: {e}"
                            )));
                        } else {
                            let buf_name =
                                match editor.create_file_buffer(&temp_path, BufferMode::DGenLisp) {
                                    Ok(name) => name,
                                    Err(e) => {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to create buffer: {e:?}"
                                        )));
                                        continue;
                                    }
                                };
                            editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                            editor_buffer_name = Some(buf_name.clone());
                            editor_mode = Some("new-effect".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                            rt.set_reactive(
                                "SEQ",
                                "editor-mode",
                                Value::String("new-effect".to_string()),
                            );
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-buffer-name", Value::String(buf_name));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                        }
                    }

                    "save-new-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.trim().to_string();
                                    if effect_name.is_empty() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String("Name cannot be empty".to_string()),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    // Validate compilation before saving — stay in editor on failure
                                    let sr = app.graph.sample_rate;
                                    if let Err(e) =
                                        sequencer::lisp_effect::compile_and_load(&source, sr)
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("{e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let final_path = format!("effects/{effect_name}.lisp");
                                    if let Err(e) = std::fs::write(&final_path, &source) {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!("Failed to save: {e}")),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let _ = std::fs::remove_file("effects/.untitled-effect.lisp");

                                    // Compilation validated — close editor
                                    editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                                    editor.remove_buffer_by_name(&buf_name);
                                    editor_buffer_name = None;
                                    editor_mode = None;

                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "available-effects",
                                        build_available_effects(),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();

                                    // Add effect to current track (re-compiles from file, uses cache)
                                    app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                    if let Some(slot_idx) = app.next_free_custom_slot() {
                                        app.start_effect_compile(&effect_name, slot_idx);
                                        editor.runtime_mut().set_reactive(
                                            "SEQ",
                                            "compiling",
                                            Value::Bool(true),
                                        );
                                    } else {
                                        editor.handle_host_event(HostEvent::Status(
                                            "No free effect slots available".to_string(),
                                        ));
                                    }
                                }
                            }
                        }
                    }

                    "enter-edit-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    let slot_idx =
                                        map.get("slot").and_then(|cell| match &*cell.borrow() {
                                            Value::Number(n) => Some(*n as usize),
                                            _ => None,
                                        });
                                    let file_path = std::path::PathBuf::from(format!(
                                        "effects/{effect_name}.lisp"
                                    ));
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Effect file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let buf_name = match editor
                                        .create_file_buffer(&file_path, BufferMode::DGenLisp)
                                    {
                                        Ok(name) => name,
                                        Err(e) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to open buffer: {e:?}"
                                            )));
                                            continue;
                                        }
                                    };
                                    editor.swap_buffer_in_tile_showing("*metal*", &buf_name);
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-effect".to_string());
                                    editor_effect_name = Some(effect_name.clone());
                                    editor_effect_slot = slot_idx;
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-mode",
                                        Value::String("edit-effect".to_string()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(buf_name),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }

                    "update-effect" => {
                        {
                            let effect_name = match editor_effect_name.clone() {
                                Some(n) => n,
                                None => {
                                    editor.handle_host_event(HostEvent::Error(
                                        "No effect being edited".to_string(),
                                    ));
                                    continue;
                                }
                            };
                            let buf_name = editor_buffer_name.clone().unwrap_or_default();
                            let source = editor.read_buffer_text(&buf_name).unwrap_or_default();

                            // Validate compilation before saving — stay in editor on failure
                            let sr = app.graph.sample_rate;
                            if let Err(e) = sequencer::lisp_effect::compile_and_load(&source, sr) {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("{e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                continue;
                            }

                            let final_path = format!("effects/{effect_name}.lisp");
                            if let Err(e) = std::fs::write(&final_path, &source) {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("Failed to save: {e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                continue;
                            }

                            // Compilation validated — close editor
                            editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                            editor.remove_buffer_by_name(&buf_name);
                            let slot_idx = editor_effect_slot.take();
                            editor_buffer_name = None;
                            editor_mode = None;
                            editor_effect_name = None;

                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                            rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive(
                                "SEQ",
                                "editor-buffer-name",
                                Value::String(String::new()),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();

                            // Recompile at the stored slot
                            if let Some(slot_idx) = slot_idx {
                                app.ui.cursor_track = current_track.load(Ordering::Relaxed);
                                app.start_effect_compile(&effect_name, slot_idx);
                                editor.runtime_mut().set_reactive(
                                    "SEQ",
                                    "compiling",
                                    Value::Bool(true),
                                );
                            } else {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Saved effect '{effect_name}'"
                                )));
                            }
                        }
                    }

                    "cancel-editor" => {
                        if let Some(buf_name) = editor_buffer_name.take() {
                            editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                            editor.remove_buffer_by_name(&buf_name);
                        }

                        // Clean up temp files for new-* modes
                        if let Some(ref mode) = editor_mode {
                            if mode == "new-instrument" {
                                let _ =
                                    std::fs::remove_file("instruments/.untitled-instrument.lisp");
                            } else if mode == "new-effect" {
                                let _ = std::fs::remove_file("effects/.untitled-effect.lisp");
                            }
                        }
                        editor_mode = None;
                        editor_effect_name = None;
                        editor_effect_slot = None;

                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.handle_host_event(HostEvent::Status("Editor cancelled".to_string()));
                    }

                    other => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Unknown host command: {other}"
                        )));
                    }
                }
            }
        }

        if app.has_pending_project_load() {
            let was_pending = true;
            match app.advance_pending_project_load() {
                Ok(()) => {
                    if was_pending && !app.has_pending_project_load() {
                        push_project_scratch_to_named_buffer(&mut editor, &app);
                        eprintln!(
                            "metal_seq: project load completed tracks={} current_project={:?}",
                            app.tracks.len(),
                            app.current_project_name
                        );
                        // Register all sampler track WAVs with eseqlisp for waveform display
                        for (t, path) in app.sampler_paths.iter().enumerate() {
                            if app.is_sampler_track(t) {
                                if let Some(p) = path {
                                    register_waveform_sample(p);
                                }
                            }
                        }
                        track_names = app.tracks.clone();
                        current_track.store(0, Ordering::Relaxed);
                        *track_pan_ids.lock().unwrap() = app
                            .graph
                            .track_node_ids
                            .iter()
                            .map(|ids| ids.pan_id)
                            .collect();
                        *record_armed.lock().unwrap() = vec![false; track_names.len()];

                        let ct = current_track.load(Ordering::Relaxed);
                        let playhead = if app.tracks.is_empty() {
                            0
                        } else {
                            state.transport.track_playheads[ct].load(Ordering::Relaxed)
                        };
                        let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
                        let bpm = state.transport.bpm.load(Ordering::Relaxed);
                        if last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                            cached_cpu_load_bits =
                                state.transport.cpu_load_pct.load(Ordering::Relaxed);
                            last_cpu_ui_poll_at = Instant::now();
                        }
                        let cpu_load_pct = f32::from_bits(cached_cpu_load_bits);
                        let playing = state.transport.playing.load(Ordering::Relaxed);
                        let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        let snap_ver = state.scheduler_snapshot_version();
                        cached_peak_l_level = meter_display_level(f32::from_bits(
                            state.transport.peak_l.load(Ordering::Relaxed),
                        ));
                        cached_peak_r_level = meter_display_level(f32::from_bits(
                            state.transport.peak_r.load(Ordering::Relaxed),
                        ));
                        cached_track_peak_levels =
                            read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                        last_meter_poll_at = Instant::now();
                        let rt = editor.runtime_mut();

                        sync_pattern_state(rt, &state);
                        sync_project_state(rt, &app);
                        rt.set_reactive("SEQ", "playing", Value::Bool(playing));
                        rt.set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                        rt.set_reactive(
                            "SEQ",
                            "transport-playhead",
                            Value::Number(transport_playhead as f64),
                        );
                        rt.set_reactive("SEQ", "cpu-load-pct", Value::Number(cpu_load_pct as f64));
                        rt.set_reactive("SEQ", "master-peak-l", Value::Number(cached_peak_l_level));
                        rt.set_reactive("SEQ", "master-peak-r", Value::Number(cached_peak_r_level));
                        rt.set_reactive(
                            "SEQ",
                            "num-tracks",
                            Value::Number(track_names.len() as f64),
                        );
                        rt.set_reactive("SEQ", "current-track", Value::Number(ct as f64));
                        rt.set_reactive("SEQ", "track-names", build_track_names(&track_names));
                        rt.set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "selected-steps",
                            build_selection_value(&selected_steps),
                        );

                        if app.tracks.is_empty() {
                            sync_playhead_fields(rt, 0, 1);
                            rt.set_reactive("SEQ", "transport-playhead", Value::Number(0.0));
                            rt.set_reactive("SEQ", "steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
                            rt.set_reactive("SEQ", "durations", Value::List(vec![]));
                            rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
                            rt.set_reactive("SEQ", "pans", Value::List(vec![]));
                            rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-volumes", Value::List(vec![]));
                            rt.set_reactive("SEQ", "effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
                            rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
                        } else {
                            sync_playhead_fields(
                                rt,
                                playhead as usize,
                                state.pattern.track_params[ct].get_num_steps(),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "transport-playhead",
                                Value::Number(transport_playhead as f64),
                            );
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                            sync_step_param_lists(rt, &state, ct);
                            rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            rt.set_reactive(
                                "SEQ",
                                "effects",
                                build_effects_value(
                                    &state,
                                    ct,
                                    &app.graph.effect_descriptors,
                                    &selected_steps,
                                ),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            sync_track_params(rt, &app, &state, ct, &selected_steps);
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                            );
                            sync_sidebar_browser(rt, &app, ct);
                        }

                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();

                        prev_current_track = ct;
                        prev_playhead = playhead;
                        prev_transport_playhead = transport_playhead;
                        prev_bpm = bpm;
                        prev_playing = playing;
                        prev_pattern_epoch = epoch;
                        prev_snapshot_version = snap_ver;
                        prev_cpu_load_bits = cached_cpu_load_bits;
                        prev_peak_l_level = cached_peak_l_level;
                        prev_peak_r_level = cached_peak_r_level;
                        prev_track_peak_levels = cached_track_peak_levels.clone();
                        prev_ui_epoch = ui_epoch.load(Ordering::Relaxed);

                        if let Some((status, _)) = app.editor.status_message.take() {
                            eprintln!("metal_seq: project load status={status}");
                            editor.handle_host_event(HostEvent::Status(status));
                        }
                    }
                }
                Err(error) => {
                    eprintln!("metal_seq: project load advance failed error={error}");
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error loading project: {error}"
                    )));
                }
            }
        }
        ui_loop_stats.note_host_commands(host_commands_started.elapsed());

        // 1c. Poll for async effect compilation
        if let Some(status) = app.poll_pending_compile() {
            let ct = current_track.load(Ordering::Relaxed);
            let rt = editor.runtime_mut();
            rt.set_reactive("SEQ", "compiling", Value::Bool(false));
            rt.set_reactive(
                "SEQ",
                "effects",
                build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "instrument-panel",
                build_instrument_panel_value(&app, ct, &selected_steps),
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            fx_epoch.fetch_add(1, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Status(status));
        }

        // 2. Sync reactive state AFTER events
        let ct = current_track.load(Ordering::Relaxed);
        sync_watched_sampler_voices(
            &app,
            ct,
            &mut watched_sampler_voice_track,
            &mut watched_sampler_voice_ids,
        );
        let reactive_sync_started = Instant::now();
        {
            let playing = state.transport.playing.load(Ordering::Relaxed);
            let bpm = state.transport.bpm.load(Ordering::Relaxed);
            if last_cpu_ui_poll_at.elapsed() >= CPU_UI_POLL_INTERVAL {
                cached_cpu_load_bits = state.transport.cpu_load_pct.load(Ordering::Relaxed);
                last_cpu_ui_poll_at = Instant::now();
            }
            let cpu_load_bits = cached_cpu_load_bits;
            let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
            let playhead = state.transport.track_playheads[ct].load(Ordering::Relaxed);
            let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            let snap_ver = state.scheduler_snapshot_version();
            if last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL {
                cached_peak_l_level = meter_display_level(f32::from_bits(
                    state.transport.peak_l.load(Ordering::Relaxed),
                ));
                cached_peak_r_level = meter_display_level(f32::from_bits(
                    state.transport.peak_r.load(Ordering::Relaxed),
                ));
                cached_track_peak_levels =
                    read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                last_meter_poll_at = Instant::now();
            }

            let mut needs_reactive_cycle = false;

            // Track switch — rebuild everything
            if ct != prev_current_track {
                editor.reset_widget_scroll_for_buffer_named("*metal*");
                let rt = editor.runtime_mut();
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                rt.set_reactive("SEQ", "current-track", Value::Number(ct as f64));
                sync_playhead_fields(
                    rt,
                    playhead as usize,
                    state.pattern.track_params[ct].get_num_steps(),
                );
                rt.set_reactive(
                    "SEQ",
                    "transport-playhead",
                    Value::Number(transport_playhead as f64),
                );
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_step_param_lists(rt, &state, ct);
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                sync_track_peak_fields(rt, &cached_track_peak_levels);
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params(rt, &app, &state, ct, &selected_steps);
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                prev_current_track = ct;
                prev_playhead = playhead;
                prev_transport_playhead = transport_playhead;
                prev_pattern_epoch = epoch;
                prev_snapshot_version = snap_ver;
                needs_reactive_cycle = true;
            }

            if playing != prev_playing {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "playing", Value::Bool(playing));
                prev_playing = playing;
                needs_reactive_cycle = true;
            }
            if bpm != prev_bpm {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                prev_bpm = bpm;
                needs_reactive_cycle = true;
            }
            if cpu_load_bits != prev_cpu_load_bits {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "cpu-load-pct",
                    Value::Number(f32::from_bits(cpu_load_bits) as f64),
                );
                prev_cpu_load_bits = cpu_load_bits;
                needs_reactive_cycle = true;
            }
            if cached_peak_l_level != prev_peak_l_level {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "master-peak-l",
                    Value::Number(cached_peak_l_level),
                );
                prev_peak_l_level = cached_peak_l_level;
                needs_reactive_cycle = true;
            }
            if cached_peak_r_level != prev_peak_r_level {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "master-peak-r",
                    Value::Number(cached_peak_r_level),
                );
                prev_peak_r_level = cached_peak_r_level;
                needs_reactive_cycle = true;
            }
            if cached_track_peak_levels != prev_track_peak_levels {
                sync_track_peak_field_delta(
                    editor.runtime_mut(),
                    &prev_track_peak_levels,
                    &cached_track_peak_levels,
                );
                prev_track_peak_levels = cached_track_peak_levels.clone();
                needs_reactive_cycle = true;
            }
            if playhead != prev_playhead {
                sync_playhead_field_delta(
                    editor.runtime_mut(),
                    prev_playhead as usize,
                    playhead as usize,
                    state.pattern.track_params[ct].get_num_steps(),
                );
                prev_playhead = playhead;
                needs_reactive_cycle = true;
            }
            if epoch != prev_pattern_epoch || snap_ver != prev_snapshot_version {
                let rt = editor.runtime_mut();
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                sync_playhead_fields(
                    rt,
                    playhead as usize,
                    state.pattern.track_params[ct].get_num_steps(),
                );
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_step_param_lists(rt, &state, ct);
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                sync_track_peak_fields(rt, &cached_track_peak_levels);
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params(rt, &app, &state, ct, &selected_steps);
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                prev_pattern_epoch = epoch;
                prev_snapshot_version = snap_ver;
                needs_reactive_cycle = true;
            }
            let ui_ep = ui_epoch.load(Ordering::Relaxed);
            if ui_ep != prev_ui_epoch {
                let rt = editor.runtime_mut();
                sync_track_name_state(rt, &mut track_names, &app);
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
                sync_track_peak_fields(rt, &cached_track_peak_levels);
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params(rt, &app, &state, ct, &selected_steps);
                rt.set_reactive(
                    "SEQ",
                    "selected-steps",
                    build_selection_value(&selected_steps),
                );
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                // Sync recording state
                let rec_on = recording.load(Ordering::Relaxed);
                rt.set_reactive("SEQ", "recording", Value::Bool(rec_on));
                let armed = record_armed.lock().unwrap();
                rt.set_reactive("SEQ", "record-armed", build_record_armed_value(&armed));
                // Sync to app for TUI recording logic
                app.ui.recording = rec_on;
                for (i, a) in armed.iter().enumerate() {
                    if i < app.graph.record_armed.len() {
                        app.graph.record_armed[i] = *a;
                    }
                }
                prev_ui_epoch = ui_ep;
                needs_reactive_cycle = true;
            }
            let fx_ep = fx_epoch.load(Ordering::Relaxed);
            if fx_ep != prev_fx_epoch {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                prev_fx_epoch = fx_ep;
                needs_reactive_cycle = true;
            }
            if transport_playhead != prev_transport_playhead {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "transport-playhead",
                    Value::Number(transport_playhead as f64),
                );
                prev_transport_playhead = transport_playhead;
                needs_reactive_cycle = true;
            }
            // Update sampler playhead for waveform display
            {
                let ct = current_track.load(Ordering::Relaxed);
                if app.is_sampler_track(ct) {
                    let ph = read_sampler_playhead_seconds(&app, ct);
                    if ph > 0.0 {
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "sampler-playhead",
                            Value::Number(ph),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            }
            let auto_follow = auto_follow_enabled(&auto_follow_override_until);
            if auto_follow != prev_auto_follow {
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "auto-follow", Value::Bool(auto_follow));
                prev_auto_follow = auto_follow;
                needs_reactive_cycle = true;
            }

            if needs_reactive_cycle {
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
            }
        }
        ui_loop_stats.note_sync(reactive_sync_started.elapsed());

        // Keep selection animation live only during playback; when paused, edits/events
        // still request redraws explicitly, but idle should stay cheap.
        if playing_now && !selected_steps.lock().unwrap().is_empty() {
            editor.mark_needs_redraw();
        }

        // Render
        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
            let frame_build_started = Instant::now();
            let tiled_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            let frame_build_elapsed = frame_build_started.elapsed();
            let render_started = Instant::now();
            backend
                .render_tiled(&tiled_frame)
                .map_err(|_| "render failed")?;
            let render_elapsed = render_started.elapsed();
            ui_loop_stats.note_frame(frame_build_elapsed, render_elapsed);
            editor.clear_needs_redraw();
            last_render_at = Instant::now();
        }

        if editor.should_quit() {
            break;
        }
    }

    let _ = backend.teardown();
    drop(stream);
    unsafe {
        sequencer::audiograph::clear_os_workgroup();
        sequencer::audiograph::engine_stop_workers();
        sequencer::audiograph::destroy_live_graph(lg_ptr.0);
    }

    Ok(())
}

/// Build a Lisp Value::List of bools from the step pattern for a given track.
fn build_steps_value(state: &Arc<SequencerState>, track: usize) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            Rc::new(RefCell::new(Value::Bool(
                state.pattern.patterns[track].is_active(s),
            )))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of floats for a given step param on a given track.
fn build_param_list(state: &Arc<SequencerState>, track: usize, param: StepParam) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| {
            let val = state.pattern.step_data[track].get(s, param);
            Rc::new(RefCell::new(Value::Number(val as f64)))
        })
        .collect();
    Value::List(items)
}

fn sync_step_param_lists(rt: &mut Runtime, state: &Arc<SequencerState>, track: usize) {
    rt.set_reactive(
        "SEQ",
        "velocities",
        build_param_list(state, track, StepParam::Velocity),
    );
    rt.set_reactive(
        "SEQ",
        "durations",
        build_param_list(state, track, StepParam::Duration),
    );
    rt.set_reactive(
        "SEQ",
        "transposes",
        build_param_list(state, track, StepParam::Transpose),
    );
    rt.set_reactive(
        "SEQ",
        "auxas",
        build_param_list(state, track, StepParam::AuxA),
    );
    rt.set_reactive(
        "SEQ",
        "pans",
        build_param_list(state, track, StepParam::Pan),
    );
    rt.set_reactive(
        "SEQ",
        "syncs",
        build_param_list(state, track, StepParam::Sync),
    );
}

fn value_cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn map_value(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut map = HashMap::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value_cell(value));
    }
    Value::Map(map)
}

fn list_value(values: impl IntoIterator<Item = Value>) -> Value {
    Value::List(values.into_iter().map(value_cell).collect())
}

fn piano_roll_lane_to_transpose(lane: usize) -> f32 {
    (PIANO_ROLL_MAX_TRANSPOSE - lane as i32)
        .clamp(PIANO_ROLL_MIN_TRANSPOSE, PIANO_ROLL_MAX_TRANSPOSE) as f32
}

fn piano_roll_transpose_to_lane(transpose: f32) -> usize {
    (PIANO_ROLL_MAX_TRANSPOSE - transpose.round() as i32)
        .clamp(0, PIANO_ROLL_MAX_TRANSPOSE - PIANO_ROLL_MIN_TRANSPOSE) as usize
}

fn piano_roll_item_id(step: usize, voice_idx: usize) -> u64 {
    (step * PIANO_ROLL_ID_STRIDE + voice_idx.min(PIANO_ROLL_ID_STRIDE - 1)) as u64
}

fn piano_roll_item_parts(id: u64) -> Option<(usize, usize)> {
    let id = id as usize;
    let step = id / PIANO_ROLL_ID_STRIDE;
    let voice_idx = id % PIANO_ROLL_ID_STRIDE;
    if step < MAX_STEPS {
        Some((step, voice_idx))
    } else {
        None
    }
}

fn build_piano_roll_lanes_value() -> Value {
    list_value(
        (PIANO_ROLL_MIN_TRANSPOSE..=PIANO_ROLL_MAX_TRANSPOSE)
            .rev()
            .map(|transpose| {
                let pitch_class = (transpose + 60).rem_euclid(12);
                let is_black_key = matches!(pitch_class, 1 | 3 | 6 | 8 | 10);
                let label = if pitch_class == 0 {
                    format!("C{}", 4 + transpose.div_euclid(12))
                } else {
                    String::new()
                };
                map_value([
                    (
                        "id",
                        Value::Number((PIANO_ROLL_MAX_TRANSPOSE - transpose) as f64),
                    ),
                    ("label", Value::String(label)),
                    (
                        "sidebar-bg",
                        Value::Keyword(if is_black_key { "black" } else { "white" }.to_string()),
                    ),
                    (
                        "label-fg",
                        Value::Keyword(if is_black_key { "white" } else { "black" }.to_string()),
                    ),
                ])
            }),
    )
}

fn piano_roll_step_note_entries(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
) -> Vec<PianoRollNote> {
    let step_duration = state.pattern.step_data[track]
        .get(step, StepParam::Duration)
        .max(PIANO_ROLL_MIN_DURATION);
    let chord_count = state.pattern.chord_data[track].count(step);
    if chord_count == 0 {
        if state.pattern.patterns[track].is_active(step) {
            vec![PianoRollNote {
                transpose: state.pattern.step_data[track].get(step, StepParam::Transpose),
                duration: step_duration,
            }]
        } else {
            Vec::new()
        }
    } else {
        (0..chord_count)
            .map(|idx| {
                let duration = state.pattern.chord_data[track].get_duration(step, idx);
                PianoRollNote {
                    transpose: state.pattern.chord_data[track].get(step, idx),
                    duration: if duration > 0.0 {
                        duration
                    } else {
                        step_duration
                    },
                }
            })
            .collect()
    }
}

fn set_piano_roll_step_note_entries(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    notes: &[PianoRollNote],
) {
    let mut notes = notes
        .iter()
        .map(|note| PianoRollNote {
            transpose: note
                .transpose
                .round()
                .clamp(StepParam::Transpose.min(), StepParam::Transpose.max()),
            duration: piano_roll_sanitize_duration(note.duration),
        })
        .collect::<Vec<_>>();
    notes.sort_by(|a, b| {
        a.transpose
            .partial_cmp(&b.transpose)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    notes.dedup_by(|a, b| (a.transpose - b.transpose).abs() < f32::EPSILON);

    state.pattern.chord_data[track].clear_step(step);
    match notes.as_slice() {
        [] => state.pattern.patterns[track].set_step_active(step, false),
        [note] => {
            state.pattern.step_data[track].set(step, StepParam::Transpose, note.transpose);
            state.pattern.step_data[track].set(step, StepParam::Duration, note.duration);
            state.pattern.patterns[track].set_step_active(step, true);
        }
        notes => {
            let max_duration = notes
                .iter()
                .map(|note| note.duration)
                .fold(PIANO_ROLL_MIN_DURATION, f32::max);
            for note in notes {
                state.pattern.chord_data[track].add_note_with_duration(
                    step,
                    note.transpose,
                    note.duration,
                );
            }
            state.pattern.step_data[track].set(step, StepParam::Transpose, notes[0].transpose);
            state.pattern.step_data[track].set(step, StepParam::Duration, max_duration);
            state.pattern.patterns[track].set_step_active(step, true);
        }
    }
}

fn piano_roll_find_note_index(
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    transpose: f32,
    duration: f32,
) -> Option<usize> {
    piano_roll_step_note_entries(state, track, step)
        .iter()
        .position(|note| {
            (note.transpose - transpose).abs() < f32::EPSILON
                && (note.duration - duration).abs() < f32::EPSILON
        })
        .or_else(|| {
            piano_roll_step_note_entries(state, track, step)
                .iter()
                .position(|note| (note.transpose - transpose).abs() < f32::EPSILON)
        })
}

fn build_piano_roll_items_value(
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<u64>>>,
) -> Value {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let selected = selected.lock().unwrap();
    let mut items = Vec::new();
    for step in 0..num_steps {
        let notes = piano_roll_step_note_entries(state, track, step);
        for (voice_idx, note) in notes.into_iter().enumerate() {
            let id = piano_roll_item_id(step, voice_idx);
            items.push(map_value([
                ("id", Value::Number(id as f64)),
                (
                    "lane",
                    Value::Number(piano_roll_transpose_to_lane(note.transpose) as f64),
                ),
                ("start", Value::Number(step as f64)),
                ("end", Value::Number((step as f32 + note.duration) as f64)),
                ("selected", Value::Bool(selected.contains(&id))),
                ("color", Value::Keyword("cyan".to_string())),
            ]));
        }
    }
    list_value(items)
}

fn build_piano_roll_selection_value(selected: &Arc<Mutex<HashSet<u64>>>) -> Value {
    let mut ids: Vec<u64> = selected.lock().unwrap().iter().copied().collect();
    ids.sort_unstable();
    list_value(ids.into_iter().map(|id| Value::Number(id as f64)))
}

fn sync_piano_roll_state(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<u64>>>,
) {
    rt.set_reactive(
        "SEQ",
        "piano-roll-items",
        build_piano_roll_items_value(state, track, selected),
    );
    rt.set_reactive(
        "SEQ",
        "piano-roll-selection",
        build_piano_roll_selection_value(selected),
    );
}

fn value_as_number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn value_as_usize(value: Option<&Value>) -> Option<usize> {
    value_as_number(value).map(|n| n.max(0.0).round() as usize)
}

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value_as_number(value).map(|n| n.max(0.0).round() as u64)
}

fn value_as_keyword_or_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Keyword(s)) | Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn cloned_map(value: &Value) -> Result<HashMap<String, Value>, String> {
    let Value::Map(map) = value else {
        return Err("expected action map".to_string());
    };
    Ok(map
        .iter()
        .map(|(key, value)| (key.clone(), value.borrow().clone()))
        .collect())
}

fn parse_piano_roll_ids(value: Option<&Value>) -> Vec<u64> {
    let Some(Value::List(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| value_as_u64(Some(&item.borrow())))
        .collect()
}

fn piano_roll_action_mutates_pattern(action: &Value) -> bool {
    let Ok(map) = cloned_map(action) else {
        return false;
    };
    matches!(
        value_as_keyword_or_string(map.get("type")).as_deref(),
        Some(
            "delete-items"
                | "nudge-selection"
                | "move-items-absolute"
                | "resize-item-absolute"
                | "finish-create-item"
        )
    )
}

fn apply_piano_roll_action(
    state: &Arc<SequencerState>,
    track: usize,
    selection: &Arc<Mutex<HashSet<u64>>>,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
    action: &Value,
) -> Result<String, String> {
    let action = cloned_map(action)?;
    let action_type = value_as_keyword_or_string(action.get("type"))
        .ok_or_else(|| "piano roll action missing :type".to_string())?;
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS)
        .max(1);

    match action_type.as_str() {
        "select" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(ids.iter().copied());
            Ok(format!("selected {} note(s)", ids.len()))
        }
        "clear-selection" => {
            *move_state.lock().unwrap() = None;
            selection.lock().unwrap().clear();
            Ok("piano roll selection cleared".to_string())
        }
        "marquee-select" | "finish-marquee-select" => {
            *move_state.lock().unwrap() = None;
            let time_a = value_as_number(action.get("time-a")).unwrap_or(0.0);
            let time_b = value_as_number(action.get("time-b")).unwrap_or(0.0);
            let lane_a = value_as_usize(action.get("lane-a")).unwrap_or(0);
            let lane_b = value_as_usize(action.get("lane-b")).unwrap_or(0);
            let lo_time = time_a.min(time_b);
            let hi_time = time_a.max(time_b);
            let lo_lane = lane_a.min(lane_b);
            let hi_lane = lane_a.max(lane_b);
            let mut ids = Vec::new();
            for step in 0..num_steps {
                let start = step as f64;
                for (voice_idx, note) in piano_roll_step_note_entries(state, track, step)
                    .into_iter()
                    .enumerate()
                {
                    let end = start + note.duration as f64;
                    if start >= hi_time || end <= lo_time {
                        continue;
                    }
                    let lane = piano_roll_transpose_to_lane(note.transpose);
                    if lane >= lo_lane && lane <= hi_lane {
                        ids.push(piano_roll_item_id(step, voice_idx));
                    }
                }
            }
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(ids.iter().copied());
            Ok(format!("marquee selected {} note(s)", ids.len()))
        }
        "delete-items" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            for id in &ids {
                if let Some((step, voice_idx)) = piano_roll_item_parts(*id) {
                    let mut notes = piano_roll_step_note_entries(state, track, step);
                    if voice_idx < notes.len() {
                        notes.remove(voice_idx);
                        set_piano_roll_step_note_entries(state, track, step, &notes);
                    }
                }
            }
            selection.lock().unwrap().clear();
            Ok(format!("deleted {} note(s)", ids.len()))
        }
        "create-item" => {
            *move_state.lock().unwrap() = None;
            Ok("drawing note".to_string())
        }
        "finish-create-item" => {
            *move_state.lock().unwrap() = None;
            let step = value_as_number(action.get("start"))
                .unwrap_or(0.0)
                .round()
                .clamp(0.0, (num_steps - 1) as f64) as usize;
            let lane = value_as_usize(action.get("lane")).unwrap_or(0);
            let duration = (value_as_number(action.get("end")).unwrap_or(step as f64 + 1.0)
                - step as f64) as f32;
            let duration = piano_roll_sanitize_duration(duration);
            let transpose = piano_roll_lane_to_transpose(lane);
            let mut notes = piano_roll_step_note_entries(state, track, step);
            notes.push(PianoRollNote {
                transpose,
                duration,
            });
            set_piano_roll_step_note_entries(state, track, step, &notes);
            let id = piano_roll_step_note_entries(state, track, step)
                .iter()
                .position(|note| (note.transpose - transpose).abs() < f32::EPSILON)
                .map(|voice_idx| piano_roll_item_id(step, voice_idx))
                .unwrap_or_else(|| piano_roll_item_id(step, 0));
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.insert(id);
            Ok(format!("created note step {} {transpose:+.0}", step + 1))
        }
        "nudge-selection" => {
            *move_state.lock().unwrap() = None;
            let ids = parse_piano_roll_ids(action.get("ids"));
            let delta_time = value_as_number(action.get("delta-time"))
                .unwrap_or(0.0)
                .round() as isize;
            let delta_lane = value_as_number(action.get("delta-lane"))
                .unwrap_or(0.0)
                .round() as isize;
            let next_ids = move_piano_roll_items_by_delta(
                state, track, num_steps, &ids, delta_time, delta_lane,
            );
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(next_ids);
            Ok(format!("nudged {} note(s)", ids.len()))
        }
        "move-items-absolute" => {
            let ids = parse_piano_roll_ids(action.get("ids"));
            let anchor_id = value_as_u64(action.get("anchor-id"))
                .or_else(|| ids.first().copied())
                .ok_or_else(|| "move-items-absolute missing anchor-id".to_string())?;
            let start = value_as_number(action.get("start")).unwrap_or(0.0).round() as isize;
            let lane = value_as_usize(action.get("lane")).unwrap_or(0) as isize;
            let next_ids = move_piano_roll_items_absolute(
                state, track, num_steps, &ids, anchor_id, start, lane, move_state,
            );
            let mut selected = selection.lock().unwrap();
            selected.clear();
            selected.extend(next_ids);
            Ok(format!("moved {} note(s)", ids.len()))
        }
        "resize-item-absolute" => {
            *move_state.lock().unwrap() = None;
            let id = value_as_u64(action.get("id"))
                .ok_or_else(|| "resize-item-absolute missing id".to_string())?;
            if value_as_keyword_or_string(action.get("edge")).as_deref() == Some("start") {
                return Ok("piano roll start resize ignored".to_string());
            }
            let time = value_as_number(action.get("time")).unwrap_or(0.0) as f32;
            if let Some((step, voice_idx)) = piano_roll_item_parts(id) {
                let duration = piano_roll_sanitize_duration(time - step as f32);
                let mut notes = piano_roll_step_note_entries(state, track, step);
                if let Some(note) = notes.get_mut(voice_idx) {
                    note.duration = duration;
                    set_piano_roll_step_note_entries(state, track, step, &notes);
                } else {
                    state.pattern.step_data[track].set(step, StepParam::Duration, duration);
                }
                Ok(format!("duration step {} = {:.2}", step + 1, duration))
            } else {
                Ok("resize ignored".to_string())
            }
        }
        "set-cursor" => Ok("piano roll cursor".to_string()),
        "scroll-view" | "zoom-view" | "set-tool" => Ok("piano roll view".to_string()),
        other => Ok(format!("ignored piano roll action {other}")),
    }
}

fn move_piano_roll_items_by_delta(
    state: &Arc<SequencerState>,
    track: usize,
    num_steps: usize,
    ids: &[u64],
    delta_time: isize,
    delta_lane: isize,
) -> Vec<u64> {
    let originals = ids
        .iter()
        .filter_map(|&id| {
            let (step, voice_idx) = piano_roll_item_parts(id)?;
            let notes = piano_roll_step_note_entries(state, track, step);
            let note = notes.get(voice_idx)?;
            Some((id, step, voice_idx, note.transpose, note.duration))
        })
        .collect::<Vec<_>>();
    let mut next_ids = Vec::with_capacity(originals.len());
    for &(_, step, voice_idx, _, _) in &originals {
        let mut notes = piano_roll_step_note_entries(state, track, step);
        if voice_idx < notes.len() {
            notes.remove(voice_idx);
            set_piano_roll_step_note_entries(state, track, step, &notes);
        }
    }
    for &(_, step, _, transpose, duration) in &originals {
        let next_step = (step as isize + delta_time).clamp(0, (num_steps - 1) as isize) as usize;
        let lane = piano_roll_transpose_to_lane(transpose) as isize + delta_lane;
        let next_transpose = piano_roll_lane_to_transpose(lane.max(0) as usize);
        let mut notes = piano_roll_step_note_entries(state, track, next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration,
        });
        set_piano_roll_step_note_entries(state, track, next_step, &notes);
        if let Some(next_voice_idx) =
            piano_roll_find_note_index(state, track, next_step, next_transpose, duration)
        {
            next_ids.push(piano_roll_item_id(next_step, next_voice_idx));
        }
    }
    next_ids
}

fn move_piano_roll_items_absolute(
    state: &Arc<SequencerState>,
    track: usize,
    num_steps: usize,
    ids: &[u64],
    anchor_id: u64,
    start: isize,
    lane: isize,
    move_state: &Arc<Mutex<Option<PianoRollMoveState>>>,
) -> Vec<u64> {
    let mut sorted_ids = ids.to_vec();
    sorted_ids.sort_unstable();

    let mut guard = move_state.lock().unwrap();
    let needs_new_state = guard
        .as_ref()
        .map(|state| state.ids != sorted_ids)
        .unwrap_or(true);

    if needs_new_state {
        let Some((anchor_step, anchor_voice_idx)) = piano_roll_item_parts(anchor_id) else {
            return Vec::new();
        };
        let anchor_notes = piano_roll_step_note_entries(state, track, anchor_step);
        let Some(anchor_note) = anchor_notes.get(anchor_voice_idx) else {
            return Vec::new();
        };
        let anchor_lane = piano_roll_transpose_to_lane(anchor_note.transpose) as isize;
        let originals = ids
            .iter()
            .filter_map(|&id| {
                let (step, voice_idx) = piano_roll_item_parts(id)?;
                let notes = piano_roll_step_note_entries(state, track, step);
                let note = notes.get(voice_idx)?;
                Some(PianoRollMoveItem {
                    id,
                    step,
                    transpose: note.transpose,
                    duration: note.duration,
                })
            })
            .collect::<Vec<_>>();
        if originals.is_empty() {
            return Vec::new();
        }
        *guard = Some(PianoRollMoveState {
            ids: sorted_ids,
            anchor_step,
            anchor_lane,
            last_positions: originals.clone(),
            originals,
        });
    }

    let Some(move_state) = guard.as_mut() else {
        return Vec::new();
    };

    for item in &move_state.last_positions {
        let mut notes = piano_roll_step_note_entries(state, track, item.step);
        if let Some(pos) = notes
            .iter()
            .position(|note| (note.transpose - item.transpose).abs() < f32::EPSILON)
        {
            notes.remove(pos);
            set_piano_roll_step_note_entries(state, track, item.step, &notes);
        }
    }

    let mut next_positions = Vec::with_capacity(move_state.originals.len());
    for item in &move_state.originals {
        let step_offset = item.step as isize - move_state.anchor_step as isize;
        let lane_offset =
            piano_roll_transpose_to_lane(item.transpose) as isize - move_state.anchor_lane;
        let next_step = (start + step_offset).clamp(0, (num_steps - 1) as isize) as usize;
        let next_lane = (lane + lane_offset).max(0) as usize;
        let next_transpose = piano_roll_lane_to_transpose(next_lane);
        let mut notes = piano_roll_step_note_entries(state, track, next_step);
        notes.push(PianoRollNote {
            transpose: next_transpose,
            duration: item.duration,
        });
        set_piano_roll_step_note_entries(state, track, next_step, &notes);
        let next_voice_idx =
            piano_roll_find_note_index(state, track, next_step, next_transpose, item.duration)
                .unwrap_or(0);
        next_positions.push(PianoRollMoveItem {
            id: piano_roll_item_id(next_step, next_voice_idx),
            step: next_step,
            transpose: next_transpose,
            duration: item.duration,
        });
    }
    move_state.last_positions = next_positions;
    move_state
        .last_positions
        .iter()
        .map(|item| item.id)
        .collect()
}

fn build_accumulator_names(app: &ui::App) -> Vec<String> {
    let mut names = BUILTIN_ACCUMULATOR_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if let Some(runtime) = app.editor.scratch_runtime.as_ref() {
        names.extend(runtime.accumulator_names());
    }
    names
}

fn build_accumulator_options(app: &ui::App) -> Value {
    let items = build_accumulator_names(app)
        .into_iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name))))
        .collect();
    Value::List(items)
}

fn build_accum_mode_options() -> Value {
    let items = ACCUM_MODE_LABELS
        .iter()
        .map(|label| Rc::new(RefCell::new(Value::String((*label).to_string()))))
        .collect();
    Value::List(items)
}

fn build_fts_options() -> Value {
    let items = FTS_SCALE_NAMES
        .iter()
        .map(|scale| Rc::new(RefCell::new(Value::String((*scale).to_string()))))
        .collect();
    Value::List(items)
}

fn builtin_accumulator_default_limit(idx: usize) -> f32 {
    match idx {
        1 => 48.0,
        2 => 1.0,
        _ => 0.0,
    }
}

fn accum_mode_label(mode: u32) -> &'static str {
    ACCUM_MODE_LABELS
        .get(mode as usize)
        .copied()
        .unwrap_or(ACCUM_MODE_LABELS[0])
}

fn selected_accumulator_name(app: &ui::App, track: usize) -> String {
    let tp = &app.state.pattern.track_params[track];
    if let Some(name) = tp.script_accumulator_name() {
        return name;
    }
    build_accumulator_names(app)
        .get(tp.get_accumulator_idx())
        .cloned()
        .unwrap_or_else(|| "Off".to_string())
}

/// Build a Lisp Value::List of bools for record-armed state per track.
fn build_record_armed_value(armed: &[bool]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = armed
        .iter()
        .map(|a| Rc::new(RefCell::new(Value::Bool(*a))))
        .collect();
    Value::List(items)
}

fn layout_node_by_id(
    node: &eseqlisp::layout::LayoutNode,
    id: u64,
) -> Option<&eseqlisp::layout::LayoutNode> {
    if node.widget_id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = layout_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn focused_widget_matches(editor: &Editor, predicate: impl FnOnce(&str) -> bool) -> bool {
    let Some(focused_id) = editor.focused_widget_id() else {
        return false;
    };
    let Some(layout) = editor.widget_layout() else {
        return false;
    };
    let Some(node) = layout_node_by_id(&layout, focused_id) else {
        return false;
    };
    predicate(node.widget_type.as_str())
}

fn focused_widget_captures_typing(editor: &Editor) -> bool {
    focused_widget_matches(editor, |widget_type| {
        matches!(widget_type, "text-input" | "number-picker")
    })
}

fn focused_widget_captures_text_input(editor: &Editor) -> bool {
    focused_widget_matches(editor, |widget_type| widget_type == "text-input")
}

fn held_note_for_key(
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
    key: &crossterm::event::KeyEvent,
) -> bool {
    let c = match key.code {
        crossterm::event::KeyCode::Char(c) => c,
        _ => return false,
    };
    held_notes.lock().unwrap().iter().any(|note| note.key == c)
}

fn should_route_to_live_keyboard(
    editor: &Editor,
    key: &crossterm::event::KeyEvent,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
) -> bool {
    use crossterm::event::{KeyEventKind, KeyModifiers};

    if matches!(key.kind, KeyEventKind::Release) {
        return held_note_for_key(held_notes, key);
    }

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return false;
    }

    if focused_widget_captures_text_input(editor) {
        return false;
    }

    matches!(key.code, crossterm::event::KeyCode::Char(_))
}

fn normalize_command_shortcuts(key: crossterm::event::KeyEvent) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    if matches!(
        key.code,
        KeyCode::Char('a')
            | KeyCode::Char('A')
            | KeyCode::Char('c')
            | KeyCode::Char('C')
            | KeyCode::Char('v')
            | KeyCode::Char('V')
    ) && key.modifiers.contains(KeyModifiers::SUPER)
    {
        let mut modifiers = key.modifiers;
        modifiers.remove(KeyModifiers::SUPER);
        modifiers.insert(KeyModifiers::CONTROL);
        return KeyEvent::new(key.code, modifiers);
    }

    key
}

fn should_toggle_play_on_space(editor: &Editor, key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if key.code != KeyCode::Char(' ') || key.modifiers != KeyModifiers::NONE {
        return false;
    }

    if editor.minibuffer_prompt().is_some() || focused_widget_captures_typing(editor) {
        return false;
    }

    let buffer = editor.active_buffer();
    buffer.read_only || matches!(buffer.view_mode, ViewMode::UiOnly) || buffer.name == "*metal*"
}

fn current_metal_cursor_step(editor: &mut Editor) -> Option<usize> {
    match editor.runtime_mut().eval_str("(current-step)") {
        Ok(Some(Value::Number(n))) if n >= 0.0 => Some(n as usize),
        _ => None,
    }
}

fn handle_metal_command_shortcut(
    editor: &mut Editor,
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    step_clipboard: &Arc<Mutex<Option<Vec<(usize, sequencer::sequencer::StepSnapshot)>>>>,
) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    if !matches!(key.kind, KeyEventKind::Press) {
        return false;
    }

    if editor.active_buffer().name != "*metal*" {
        return false;
    }

    if key.modifiers.contains(KeyModifiers::SUPER) {
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let _ = editor.runtime_mut().eval_str("(select-all-steps)");
                editor.refresh_runtime_side_effects();
                return true;
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                let track = current_track.load(Ordering::Relaxed);
                let steps: Vec<usize> = {
                    let set = selected_steps.lock().unwrap();
                    if set.is_empty() {
                        current_metal_cursor_step(editor).into_iter().collect()
                    } else {
                        let mut steps: Vec<usize> = set.iter().copied().collect();
                        steps.sort_unstable();
                        steps
                    }
                };
                if steps.is_empty() {
                    return true;
                }
                let anchor = steps[0];
                let clipboard: Vec<(usize, sequencer::sequencer::StepSnapshot)> = steps
                    .iter()
                    .map(|&s| (s - anchor, state.capture_step_snapshot(track, s)))
                    .collect();
                let count = clipboard.len();
                *step_clipboard.lock().unwrap() = Some(clipboard);
                editor.handle_host_event(HostEvent::Status(format!(
                    "Copied {} step{}",
                    count,
                    if count == 1 { "" } else { "s" }
                )));
                return true;
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                let dest_start = match current_metal_cursor_step(editor) {
                    Some(step) => step,
                    None => return true,
                };
                let clipboard = {
                    let guard = step_clipboard.lock().unwrap();
                    guard.clone()
                };
                let Some(clipboard) = clipboard else {
                    return true;
                };
                let track = current_track.load(Ordering::Relaxed);
                let num_steps = state.pattern.track_params[track].get_num_steps();
                for (offset, snapshot) in &clipboard {
                    let dest = dest_start + offset;
                    if dest >= num_steps {
                        continue;
                    }
                    if !snapshot.active && state.pattern.patterns[track].is_active(dest) {
                        continue;
                    }
                    state.restore_step_snapshot(track, dest, snapshot);
                }
                state.publish_scheduler_snapshot();
                editor.handle_host_event(HostEvent::Status(format!(
                    "Pasted {} step{}",
                    clipboard.len(),
                    if clipboard.len() == 1 { "" } else { "s" }
                )));
                return true;
            }
            _ => {}
        }
    }

    false
}

/// Build a Lisp Value::List of track name strings.
fn build_track_names(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
}

fn sync_track_name_state(rt: &mut Runtime, track_names: &mut Vec<String>, app: &ui::App) {
    if *track_names == app.tracks {
        return;
    }
    *track_names = app.tracks.clone();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
}

/// Build a Lisp Value::List of per-track volumes (0.0–1.0).
fn build_track_volumes(state: &Arc<SequencerState>) -> Value {
    let count = state.active_track_count();
    let items: Vec<Rc<RefCell<Value>>> = (0..count)
        .map(|t| {
            let vol = state.pattern.track_params[t].get_volume();
            Rc::new(RefCell::new(Value::Number(vol as f64)))
        })
        .collect();
    Value::List(items)
}

fn read_track_peak_levels(lg: sequencer::audiograph::LiveGraphPtr, pan_ids: &[i32]) -> Vec<f64> {
    const PANNER_STATE_LEN: usize = sequencer::stereo_panner::STEREO_PANNER_STATE_SIZE;
    const PANNER_STATE_BYTES: usize = PANNER_STATE_LEN * std::mem::size_of::<f32>();
    pan_ids
        .iter()
        .map(|&pan_id| {
            if pan_id < 0 {
                return 0.0;
            }
            let mut state_size = 0usize;
            let mut state = [0.0_f32; PANNER_STATE_LEN];
            let copied = unsafe {
                sequencer::audiograph::get_node_state_into(
                    lg.0,
                    pan_id,
                    state.as_mut_ptr().cast(),
                    PANNER_STATE_BYTES,
                    &mut state_size as *mut usize,
                )
            };
            if !copied || state_size < PANNER_STATE_BYTES {
                return 0.0;
            }
            let peak = state[sequencer::stereo_panner::STATE_PEAK_L]
                .max(state[sequencer::stereo_panner::STATE_PEAK_R]);
            meter_display_level(peak)
        })
        .collect()
}

fn build_track_peaks_value(levels: &[f64]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = levels
        .iter()
        .map(|&level| Rc::new(RefCell::new(Value::Number(level))))
        .collect();
    Value::List(items)
}

fn sync_track_peak_fields(rt: &mut Runtime, levels: &[f64]) {
    for (idx, &level) in levels.iter().enumerate() {
        rt.set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level));
    }
}

fn sync_track_peak_field_delta(rt: &mut Runtime, previous: &[f64], levels: &[f64]) {
    if previous.len() != levels.len() {
        sync_track_peak_fields(rt, levels);
        for idx in levels.len()..previous.len() {
            rt.set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(0.0));
        }
        return;
    }

    for (idx, (&old_level, &level)) in previous.iter().zip(levels.iter()).enumerate() {
        if old_level != level {
            rt.set_reactive("SEQ", &format!("track-peak-{idx}"), Value::Number(level));
        }
    }
}

fn sync_playhead_fields(rt: &mut Runtime, playhead: usize, num_steps: usize) {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    rt.set_reactive(
        "SEQ",
        "playhead-page",
        Value::Number((active_step / PAGE_SIZE) as f64),
    );
    rt.set_reactive("SEQ", "playhead", Value::Number(active_step as f64));
    for idx in 0..MAX_STEPS {
        rt.set_reactive(
            "SEQ",
            &format!("playhead-active-{idx}"),
            Value::Bool(idx == active_step && idx < clamped_steps),
        );
    }
}

fn sync_playhead_field_delta(
    rt: &mut Runtime,
    prev_playhead: usize,
    playhead: usize,
    num_steps: usize,
) {
    let clamped_steps = num_steps.max(1).min(MAX_STEPS);
    let prev_active = prev_playhead.min(clamped_steps.saturating_sub(1));
    let active_step = playhead.min(clamped_steps.saturating_sub(1));
    rt.set_reactive(
        "SEQ",
        "playhead-page",
        Value::Number((active_step / PAGE_SIZE) as f64),
    );
    rt.set_reactive("SEQ", "playhead", Value::Number(active_step as f64));
    if prev_active != active_step {
        rt.set_reactive(
            "SEQ",
            &format!("playhead-active-{prev_active}"),
            Value::Bool(false),
        );
        rt.set_reactive(
            "SEQ",
            &format!("playhead-active-{active_step}"),
            Value::Bool(true),
        );
    }
}

fn sync_track_topology_state(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track_names: &mut Vec<String>,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    piano_roll_selection: &Arc<Mutex<HashSet<u64>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    track_peak_levels: &[f64],
) {
    sync_track_name_state(rt, track_names, app);
    sync_pattern_state(rt, state);
    rt.set_reactive(
        "SEQ",
        "current-track",
        Value::Number(current_track_idx as f64),
    );
    rt.set_reactive(
        "SEQ",
        "record-armed",
        build_record_armed_value(&record_armed.lock().unwrap()),
    );

    if app.tracks.is_empty() {
        sync_playhead_fields(rt, 0, 1);
        rt.set_reactive("SEQ", "steps", Value::List(vec![]));
        rt.set_reactive("SEQ", "piano-roll-items", Value::List(vec![]));
        rt.set_reactive("SEQ", "piano-roll-selection", Value::List(vec![]));
        rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
        rt.set_reactive("SEQ", "durations", Value::List(vec![]));
        rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
        rt.set_reactive("SEQ", "auxas", Value::List(vec![]));
        rt.set_reactive("SEQ", "pans", Value::List(vec![]));
        rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
        rt.set_reactive("SEQ", "track-volumes", Value::List(vec![]));
        rt.set_reactive("SEQ", "effects", Value::List(vec![]));
        rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
        rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
        return;
    }

    sync_playhead_fields(
        rt,
        state.transport.track_playheads[current_track_idx].load(Ordering::Relaxed) as usize,
        state.pattern.track_params[current_track_idx].get_num_steps(),
    );
    rt.set_reactive("SEQ", "steps", build_steps_value(state, current_track_idx));
    sync_piano_roll_state(rt, state, current_track_idx, piano_roll_selection);
    sync_step_param_lists(rt, state, current_track_idx);
    rt.set_reactive("SEQ", "track-volumes", build_track_volumes(state));
    sync_track_peak_fields(rt, track_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(
            state,
            current_track_idx,
            &app.graph.effect_descriptors,
            selected_steps,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, current_track_idx, selected_steps),
    );
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, current_track_idx, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, current_track_idx, &app.graph.effect_descriptors),
    );
    sync_sidebar_browser(rt, app, current_track_idx);
}

fn sync_pattern_state(rt: &mut Runtime, state: &Arc<SequencerState>) {
    rt.set_reactive(
        "SEQ",
        "current-pattern",
        Value::Number(state.pattern.current_pattern.load(Ordering::Relaxed) as f64),
    );
    rt.set_reactive(
        "SEQ",
        "num-patterns",
        Value::Number(state.pattern.num_patterns.load(Ordering::Relaxed) as f64),
    );
}

fn build_sync_labels() -> Value {
    let items: Vec<Rc<RefCell<Value>>> = SYNC_RESOLUTIONS
        .iter()
        .map(|(_, label)| {
            let mut compact = label.replace(' ', "");
            compact.truncate(4);
            Rc::new(RefCell::new(Value::String(compact)))
        })
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of effect slot maps for a track.
/// Each slot is a map: {:name "Filter" :params ({:name "cutoff" :value 1000 :min 20 :max 20000} ...)}
fn build_effects_value(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use sequencer::effects::{ParamKind, SyncDivision};
    use std::collections::HashMap;
    let Some(track_descs) = descriptors.get(track) else {
        return Value::List(vec![]);
    };
    let chain = &state.pattern.effect_chains[track];
    let sel = selected.lock().unwrap();
    // If steps are selected, show p-lock value from first selected step
    let plock_step = sel.iter().copied().min();

    let slots: Vec<Rc<RefCell<Value>>> = track_descs
        .iter()
        .enumerate()
        .map(|(slot_idx, desc)| {
            let mut slot_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();

            slot_map.insert(
                "name".to_string(),
                Rc::new(RefCell::new(Value::String(desc.name.clone()))),
            );

            slot_map.insert(
                "slot-idx".to_string(),
                Rc::new(RefCell::new(Value::Number(slot_idx as f64))),
            );

            let params: Vec<Rc<RefCell<Value>>> = desc
                .params
                .iter()
                .enumerate()
                .map(|(param_idx, pdesc)| {
                    let delay_synced = if desc.name == "Delay" {
                        chain
                            .get(slot_idx)
                            .map(|s| s.defaults.get(1) > 0.5)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    let default_val = chain
                        .get(slot_idx)
                        .map(|s| s.defaults.get(param_idx))
                        .unwrap_or(pdesc.default);
                    // Show p-lock value if steps are selected, fall back to default
                    let current_val = plock_step
                        .and_then(|step| chain.get(slot_idx)?.plocks.get(step, param_idx))
                        .unwrap_or(default_val);

                    let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
                    pmap.insert(
                        "name".to_string(),
                        Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
                    );
                    pmap.insert(
                        "idx".to_string(),
                        Rc::new(RefCell::new(Value::Number(param_idx as f64))),
                    );
                    pmap.insert(
                        "value".to_string(),
                        Rc::new(RefCell::new(Value::Number(current_val as f64))),
                    );
                    pmap.insert(
                        "min".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.min as f64))),
                    );
                    pmap.insert(
                        "max".to_string(),
                        Rc::new(RefCell::new(Value::Number(pdesc.max as f64))),
                    );
                    match &pdesc.kind {
                        ParamKind::Boolean => {
                            pmap.insert(
                                "boolean".to_string(),
                                Rc::new(RefCell::new(Value::Bool(true))),
                            );
                        }
                        ParamKind::Enum { labels } => {
                            let selected = labels
                                .get(current_val.round() as usize)
                                .cloned()
                                .unwrap_or_default();
                            let option_values = labels
                                .iter()
                                .cloned()
                                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                .collect();
                            pmap.insert(
                                "text-value".to_string(),
                                Rc::new(RefCell::new(Value::String(selected))),
                            );
                            pmap.insert(
                                "options".to_string(),
                                Rc::new(RefCell::new(Value::List(option_values))),
                            );
                        }
                        ParamKind::Continuous { .. } => {
                            if desc.name == "Delay" && param_idx == 2 && delay_synced {
                                let labels: Vec<String> = SyncDivision::ALL
                                    .iter()
                                    .map(|d| d.label().to_string())
                                    .collect();
                                let selected = labels
                                    .get(current_val.round() as usize)
                                    .cloned()
                                    .unwrap_or_default();
                                let option_values = labels
                                    .into_iter()
                                    .map(|label| Rc::new(RefCell::new(Value::String(label))))
                                    .collect();
                                pmap.insert(
                                    "text-value".to_string(),
                                    Rc::new(RefCell::new(Value::String(selected))),
                                );
                                pmap.insert(
                                    "options".to_string(),
                                    Rc::new(RefCell::new(Value::List(option_values))),
                                );
                                pmap.insert(
                                    "min".to_string(),
                                    Rc::new(RefCell::new(Value::Number(0.0))),
                                );
                                pmap.insert(
                                    "max".to_string(),
                                    Rc::new(RefCell::new(Value::Number(
                                        (SyncDivision::ALL.len() - 1) as f64,
                                    ))),
                                );
                            }
                        }
                    }
                    Rc::new(RefCell::new(Value::Map(pmap)))
                })
                .collect();

            slot_map.insert(
                "params".to_string(),
                Rc::new(RefCell::new(Value::List(params))),
            );

            Rc::new(RefCell::new(Value::Map(slot_map)))
        })
        .collect();

    Value::List(slots)
}

fn build_sampler_panel_value(
    app: &ui::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .unwrap_or_else(sequencer::effects::EffectDescriptor::builtin_sampler);

    // Look up the pre-registered SampleBuffer and pass its Value map directly
    // to the Lisp side, so the waveform widget can use it without re-loading.
    let registered_sample = app
        .sampler_paths
        .get(track)
        .and_then(|p| p.as_ref())
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()));
    let buffer_value = registered_sample.as_ref().map(|s| s.to_value());
    let sample_duration = registered_sample
        .as_ref()
        .map(|s| s.duration_seconds)
        .unwrap_or(1.0);

    let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = slot.defaults.get(param_idx);
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(pdesc.name.clone()))),
        );
        pmap.insert(
            "idx".to_string(),
            Rc::new(RefCell::new(Value::Number(param_idx as f64))),
        );
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(current_val) as f64,
            ))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(pdesc.min) as f64
            ))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(
                pdesc.stored_to_user(pdesc.max) as f64
            ))),
        );
        params.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    panel_map.insert(
        "type".to_string(),
        Rc::new(RefCell::new(Value::String("sampler".to_string()))),
    );
    if let Some(buf_val) = buffer_value {
        panel_map.insert("buffer".to_string(), Rc::new(RefCell::new(buf_val)));
    }
    panel_map.insert(
        "params".to_string(),
        Rc::new(RefCell::new(Value::List(params))),
    );
    // Start/end as seconds for the waveform selection overlay.
    // Raw stored values are 0.0-1.0 normalized; multiply by duration.
    let start_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 2))
        .unwrap_or_else(|| slot.defaults.get(2));
    let end_raw = plock_step
        .and_then(|step| slot.plocks.get(step, 3))
        .unwrap_or_else(|| slot.defaults.get(3));
    panel_map.insert(
        "start-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (start_raw as f64) * sample_duration,
        ))),
    );
    panel_map.insert(
        "end-time".to_string(),
        Rc::new(RefCell::new(Value::Number(
            (end_raw as f64) * sample_duration,
        ))),
    );
    panel_map.insert(
        "duration".to_string(),
        Rc::new(RefCell::new(Value::Number(sample_duration))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

fn build_instrument_panel_value(
    app: &ui::App,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) -> Value {
    use std::collections::HashMap;

    const MOD_PARAM_BASE: u32 = 1_000_000;
    const PARAM_LFO1_RATE_HZ: usize = 13;
    const PARAM_LFO1_SYNC: usize = 14;
    const PARAM_LFO1_DIV: usize = 15;
    const PARAM_LFO1_SHAPE: usize = 16;
    const PARAM_LFO1_PW: usize = 17;
    const PARAM_LFO1_RETRIGGER: usize = 18;
    const PARAM_LFO2_RATE_HZ: usize = 19;
    const PARAM_LFO2_SYNC: usize = 20;
    const PARAM_LFO2_DIV: usize = 21;
    const PARAM_LFO2_SHAPE: usize = 22;
    const PARAM_LFO2_PW: usize = 23;
    const PARAM_LFO2_RETRIGGER: usize = 24;
    const PARAM_LFO3_RATE_HZ: usize = 25;
    const PARAM_LFO3_SYNC: usize = 26;
    const PARAM_LFO3_DIV: usize = 27;
    const PARAM_LFO3_SHAPE: usize = 28;
    const PARAM_LFO3_PW: usize = 29;
    const PARAM_LFO3_RETRIGGER: usize = 30;
    const PARAM_ENV_ATTACK_MS: usize = 31;
    const PARAM_ENV_DECAY_MS: usize = 32;
    const PARAM_ENV_SUSTAIN: usize = 33;
    const PARAM_ENV_RELEASE_MS: usize = 34;
    const PARAM_RAND_RATE_HZ: usize = 35;
    const PARAM_RAND_SYNC: usize = 36;
    const PARAM_RAND_DIV: usize = 37;
    const PARAM_RAND_SLEW: usize = 38;
    const PARAM_DRIFT_RATE: usize = 39;
    const PARAM_DRIFT_SYNC: usize = 40;
    const PARAM_DRIFT_DIV: usize = 41;

    if app.is_sampler_track(track) {
        return build_sampler_panel_value(app, track, selected);
    }
    let Some(desc) = app.graph.instrument_descriptors.get(track) else {
        return Value::List(vec![]);
    };
    if desc.params.is_empty() {
        return Value::List(vec![]);
    }

    let sel = selected.lock().unwrap();
    let plock_step = sel.iter().copied().min();
    let slot = &app.state.pattern.instrument_slots[track];
    let base_note_default = f32::from_bits(
        app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
    );
    let base_note_current = base_note_default;

    fn push_param(
        out: &mut Vec<Rc<RefCell<Value>>>,
        name: String,
        control: &str,
        idx: Option<usize>,
        value: f32,
        min: f32,
        max: f32,
        options: Option<&Vec<String>>,
    ) {
        let is_boolean_name = name == "enabled" || name == "sync";
        let mut pmap: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        pmap.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(name))),
        );
        pmap.insert(
            "control".to_string(),
            Rc::new(RefCell::new(Value::String(control.to_string()))),
        );
        if let Some(idx) = idx {
            pmap.insert(
                "idx".to_string(),
                Rc::new(RefCell::new(Value::Number(idx as f64))),
            );
        }
        pmap.insert(
            "value".to_string(),
            Rc::new(RefCell::new(Value::Number(value as f64))),
        );
        pmap.insert(
            "min".to_string(),
            Rc::new(RefCell::new(Value::Number(min as f64))),
        );
        pmap.insert(
            "max".to_string(),
            Rc::new(RefCell::new(Value::Number(max as f64))),
        );
        if let Some(labels) = options {
            let selected = labels
                .get(value.round() as usize)
                .cloned()
                .unwrap_or_default();
            let option_values = labels
                .iter()
                .cloned()
                .map(|label| Rc::new(RefCell::new(Value::String(label))))
                .collect();
            pmap.insert(
                "text-value".to_string(),
                Rc::new(RefCell::new(Value::String(selected))),
            );
            pmap.insert(
                "options".to_string(),
                Rc::new(RefCell::new(Value::List(option_values))),
            );
        }
        if options.is_none() && is_boolean_name {
            pmap.insert(
                "boolean".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
        }
        out.push(Rc::new(RefCell::new(Value::Map(pmap))));
    }

    fn is_mod_param(name: &str) -> bool {
        name.starts_with("mod ")
    }

    fn is_source_param(node_param_idx: u32) -> bool {
        node_param_idx >= MOD_PARAM_BASE
    }

    fn source_section_name(node_param_idx: u32) -> &'static str {
        if (MOD_PARAM_BASE + PARAM_LFO1_RATE_HZ as u32
            ..=MOD_PARAM_BASE + PARAM_LFO1_RETRIGGER as u32)
            .contains(&node_param_idx)
        {
            "LFO 1"
        } else if (MOD_PARAM_BASE + PARAM_ENV_ATTACK_MS as u32
            ..=MOD_PARAM_BASE + PARAM_ENV_RELEASE_MS as u32)
            .contains(&node_param_idx)
        {
            "ENV 1"
        } else if (MOD_PARAM_BASE + PARAM_RAND_RATE_HZ as u32
            ..=MOD_PARAM_BASE + PARAM_RAND_SLEW as u32)
            .contains(&node_param_idx)
        {
            "RAND"
        } else if (MOD_PARAM_BASE + PARAM_DRIFT_RATE as u32
            ..=MOD_PARAM_BASE + PARAM_DRIFT_DIV as u32)
            .contains(&node_param_idx)
        {
            "DRIFT"
        } else if (MOD_PARAM_BASE + PARAM_LFO2_RATE_HZ as u32
            ..=MOD_PARAM_BASE + PARAM_LFO2_RETRIGGER as u32)
            .contains(&node_param_idx)
        {
            "LFO 2"
        } else {
            "LFO 3"
        }
    }

    fn rename_source_param(name: &str) -> String {
        if name.ends_with("_div") || name.ends_with("_rate") {
            "rate".to_string()
        } else if name.ends_with("_sync") {
            "sync".to_string()
        } else if name.ends_with("_shape") {
            "shape".to_string()
        } else if name.ends_with("_pw") {
            "pulse width".to_string()
        } else if name.ends_with("_retrigger") {
            "retrigger".to_string()
        } else if name == "mod_rand_slew" {
            "slew".to_string()
        } else if name == "mod_env_attack" {
            "attack".to_string()
        } else if name == "mod_env_decay" {
            "decay".to_string()
        } else if name == "mod_env_sustain" {
            "sustain".to_string()
        } else if name == "mod_env_release" {
            "release".to_string()
        } else {
            name.to_string()
        }
    }

    let source_indices: Vec<usize> = desc
        .params
        .iter()
        .enumerate()
        .filter_map(|(i, p)| is_source_param(p.node_param_idx).then_some(i))
        .collect();

    let find_idx_by_node = |node_param_idx: u32| {
        source_indices
            .iter()
            .copied()
            .find(|&idx| desc.params.get(idx).map(|p| p.node_param_idx) == Some(node_param_idx))
    };

    let lfo_sync = |sync_idx: u32| -> bool {
        find_idx_by_node(sync_idx)
            .map(|idx| slot.defaults.get(idx) > 0.5)
            .unwrap_or(false)
    };
    let lfo_shape_is_pulse = |shape_idx: u32| -> bool {
        find_idx_by_node(shape_idx)
            .map(|idx| slot.defaults.get(idx).round() as i32 == 2)
            .unwrap_or(false)
    };

    let mut source_actual: Vec<usize> = Vec::new();
    let push_lfo = |out: &mut Vec<usize>,
                    rate_idx: usize,
                    sync_idx: usize,
                    div_idx: usize,
                    shape_idx: usize,
                    pw_idx: usize,
                    retrig_idx: usize| {
        let rate_node = MOD_PARAM_BASE + rate_idx as u32;
        let sync_node = MOD_PARAM_BASE + sync_idx as u32;
        let div_node = MOD_PARAM_BASE + div_idx as u32;
        let shape_node = MOD_PARAM_BASE + shape_idx as u32;
        let pw_node = MOD_PARAM_BASE + pw_idx as u32;
        let retrig_node = MOD_PARAM_BASE + retrig_idx as u32;

        if let Some(idx) = if lfo_sync(sync_node) {
            find_idx_by_node(div_node)
        } else {
            find_idx_by_node(rate_node)
        } {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(sync_node) {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(shape_node) {
            out.push(idx);
        }
        if let Some(idx) = find_idx_by_node(retrig_node) {
            out.push(idx);
        }
        if lfo_shape_is_pulse(shape_node) {
            if let Some(idx) = find_idx_by_node(pw_node) {
                out.push(idx);
            }
        }
    };

    push_lfo(
        &mut source_actual,
        PARAM_LFO1_RATE_HZ,
        PARAM_LFO1_SYNC,
        PARAM_LFO1_DIV,
        PARAM_LFO1_SHAPE,
        PARAM_LFO1_PW,
        PARAM_LFO1_RETRIGGER,
    );
    for idx_const in [
        PARAM_ENV_ATTACK_MS,
        PARAM_ENV_DECAY_MS,
        PARAM_ENV_SUSTAIN,
        PARAM_ENV_RELEASE_MS,
    ] {
        if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + idx_const as u32) {
            source_actual.push(idx);
        }
    }
    if let Some(idx) = if lfo_sync(MOD_PARAM_BASE + PARAM_RAND_SYNC as u32) {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_DIV as u32)
    } else {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_RATE_HZ as u32)
    } {
        source_actual.push(idx);
    }
    if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_SYNC as u32) {
        source_actual.push(idx);
    }
    if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + PARAM_RAND_SLEW as u32) {
        source_actual.push(idx);
    }
    if let Some(idx) = if lfo_sync(MOD_PARAM_BASE + PARAM_DRIFT_SYNC as u32) {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_DRIFT_DIV as u32)
    } else {
        find_idx_by_node(MOD_PARAM_BASE + PARAM_DRIFT_RATE as u32)
    } {
        source_actual.push(idx);
    }
    if let Some(idx) = find_idx_by_node(MOD_PARAM_BASE + PARAM_DRIFT_SYNC as u32) {
        source_actual.push(idx);
    }
    push_lfo(
        &mut source_actual,
        PARAM_LFO2_RATE_HZ,
        PARAM_LFO2_SYNC,
        PARAM_LFO2_DIV,
        PARAM_LFO2_SHAPE,
        PARAM_LFO2_PW,
        PARAM_LFO2_RETRIGGER,
    );
    push_lfo(
        &mut source_actual,
        PARAM_LFO3_RATE_HZ,
        PARAM_LFO3_SYNC,
        PARAM_LFO3_DIV,
        PARAM_LFO3_SHAPE,
        PARAM_LFO3_PW,
        PARAM_LFO3_RETRIGGER,
    );

    let mut synth_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    let mut mod_params: Vec<Rc<RefCell<Value>>> = Vec::new();
    push_param(
        &mut synth_params,
        "base_note".to_string(),
        "base-note",
        None,
        base_note_current,
        -48.0,
        48.0,
        None,
    );

    for (param_idx, pdesc) in desc.params.iter().enumerate() {
        let default_val = slot.defaults.get(param_idx);
        let current_val = plock_step
            .and_then(|step| slot.plocks.get(step, param_idx))
            .unwrap_or(default_val);
        let options = match &pdesc.kind {
            sequencer::effects::ParamKind::Enum { labels } => Some(labels),
            _ => None,
        };
        if is_source_param(pdesc.node_param_idx) {
            continue;
        }
        if is_mod_param(&pdesc.name) {
            push_param(
                &mut mod_params,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
            );
        } else {
            push_param(
                &mut synth_params,
                pdesc.name.clone(),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
            );
        }
    }

    let mut source_sections: Vec<Rc<RefCell<Value>>> = Vec::new();
    for section_name in ["LFO 1", "ENV 1", "RAND", "DRIFT", "LFO 2", "LFO 3"] {
        let mut params: Vec<Rc<RefCell<Value>>> = Vec::new();
        for &param_idx in &source_actual {
            let Some(pdesc) = desc.params.get(param_idx) else {
                continue;
            };
            if source_section_name(pdesc.node_param_idx) != section_name {
                continue;
            }
            let default_val = slot.defaults.get(param_idx);
            let current_val = plock_step
                .and_then(|step| slot.plocks.get(step, param_idx))
                .unwrap_or(default_val);
            let options = match &pdesc.kind {
                sequencer::effects::ParamKind::Enum { labels } => Some(labels),
                _ => None,
            };
            push_param(
                &mut params,
                rename_source_param(&pdesc.name),
                "param",
                Some(param_idx),
                pdesc.stored_to_user(current_val),
                pdesc.stored_to_user(pdesc.min),
                pdesc.stored_to_user(pdesc.max),
                options,
            );
        }
        if params.is_empty() {
            continue;
        }
        let mut section_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        section_map.insert(
            "name".to_string(),
            Rc::new(RefCell::new(Value::String(section_name.to_string()))),
        );
        section_map.insert(
            "params".to_string(),
            Rc::new(RefCell::new(Value::List(params))),
        );
        source_sections.push(Rc::new(RefCell::new(Value::Map(section_map))));
    }

    let mut panel_map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    panel_map.insert(
        "name".to_string(),
        Rc::new(RefCell::new(Value::String(
            current_custom_instrument_name(app, track).unwrap_or_else(|| "Instrument".to_string()),
        ))),
    );
    panel_map.insert(
        "synth".to_string(),
        Rc::new(RefCell::new(Value::List(synth_params))),
    );
    panel_map.insert(
        "mod".to_string(),
        Rc::new(RefCell::new(Value::List(mod_params))),
    );
    panel_map.insert(
        "sources".to_string(),
        Rc::new(RefCell::new(Value::List(source_sections))),
    );

    Value::List(vec![Rc::new(RefCell::new(Value::Map(panel_map)))])
}

/// Build a Lisp Value::List of bools indicating which steps are selected.
fn build_selection_value(selected: &Arc<Mutex<HashSet<usize>>>) -> Value {
    let set = selected.lock().unwrap();
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|s| Rc::new(RefCell::new(Value::Bool(set.contains(&s)))))
        .collect();
    Value::List(items)
}

/// Build list of available effect names from the effects/ directory.
/// Prepends "+ New Effect" as a special entry for inline creation.
fn build_available_effects() -> Value {
    let names = sequencer::lisp_effect::list_saved_effects();
    let mut items: Vec<Rc<RefCell<Value>>> = vec![Rc::new(RefCell::new(Value::String(
        "+ New Effect".to_string(),
    )))];
    items.extend(
        names
            .into_iter()
            .map(|n| Rc::new(RefCell::new(Value::String(n)))),
    );
    Value::List(items)
}

fn build_string_list(items: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = items
        .iter()
        .map(|item| Rc::new(RefCell::new(Value::String(item.clone()))))
        .collect();
    Value::List(items)
}

fn master_meter_level(peak: f32) -> f64 {
    if peak <= 0.0 {
        0.0
    } else {
        peak.sqrt().min(1.2) as f64
    }
}

fn quantize_meter_level(level: f64) -> f64 {
    ((level.clamp(0.0, 1.2) * METER_LEVEL_STEPS).round()) / METER_LEVEL_STEPS
}

fn meter_display_level(peak: f32) -> f64 {
    quantize_meter_level(master_meter_level(peak))
}

fn build_flat_tree_items(items: &[String]) -> Value {
    use std::collections::HashMap;
    let items: Vec<Rc<RefCell<Value>>> = items
        .iter()
        .map(|item| {
            let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
            map.insert(
                "label".to_string(),
                Rc::new(RefCell::new(Value::String(item.clone()))),
            );
            Rc::new(RefCell::new(Value::Map(map)))
        })
        .collect();
    Value::List(items)
}

fn visible_project_items() -> Vec<String> {
    sequencer::project::list_project_names().unwrap_or_default()
}

fn build_project_tree(query: &str) -> Value {
    let query = query.trim().to_lowercase();
    let mut items = visible_project_items();
    if !query.is_empty() {
        items.retain(|item| item.to_lowercase().contains(&query));
    }
    build_flat_tree_items(&items)
}

fn sync_project_state(rt: &mut Runtime, app: &ui::App) {
    rt.set_reactive(
        "SEQ",
        "current-project-name",
        Value::String(app.current_project_name.clone().unwrap_or_default()),
    );
}

const PROJECT_SCRATCH_BUFFER_NAME: &str = "*scratch*";

fn push_project_scratch_to_named_buffer(editor: &mut Editor, app: &ui::App) {
    let scratch_text = app.editor.scratch_buffer.clone();
    let scratch_cursor = app.editor.scratch_cursor;

    editor.upsert_scratch_buffer(PROJECT_SCRATCH_BUFFER_NAME, &scratch_text);

    if editor.active_buffer().name == PROJECT_SCRATCH_BUFFER_NAME {
        let buffer = editor.active_buffer_mut();
        let row = scratch_cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = scratch_cursor.1.min(buffer.lines[row].len());
        buffer.cursor = (row, col);
    }
}

fn pull_named_scratch_buffer_into_project(editor: &Editor, app: &mut ui::App) {
    let buffer = editor.active_buffer();
    if buffer.name != PROJECT_SCRATCH_BUFFER_NAME {
        return;
    }

    let text = buffer.text();
    let cursor = buffer.cursor;
    if app.editor.scratch_buffer != text || app.editor.scratch_cursor != cursor {
        app.editor.scratch_buffer = text.clone();
        app.editor.scratch_cursor = cursor;
        app.state.set_scratch_source(text);
        app.editor.scratch_runtime = None;
    }
}

fn current_custom_instrument_name(app: &ui::App, track: usize) -> Option<String> {
    if app.tracks.is_empty() || app.is_sampler_track(track) {
        None
    } else if let Some(Some(engine_id)) = app.graph.track_engine_ids.get(track) {
        app.editor
            .engine_registry
            .get(*engine_id)
            .map(|engine| engine.name.clone())
    } else {
        app.tracks.get(track).cloned()
    }
}

fn visible_preset_items_for_track(app: &ui::App, track: usize) -> Vec<String> {
    let Some(name) = current_custom_instrument_name(app, track) else {
        return Vec::new();
    };
    let mut items: Vec<String> = sequencer::lisp_effect::load_instrument_presets(&name)
        .unwrap_or_default()
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    items.sort();
    items
}

fn sync_sidebar_browser(rt: &mut Runtime, app: &ui::App, track: usize) {
    if app.is_sampler_track(track) {
        let selected_sample = app
            .sampler_paths
            .get(track)
            .and_then(|path| path.as_ref())
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        rt.set_reactive("SEQ", "sidebar-kind", Value::String("sampler".to_string()));
        rt.set_reactive(
            "SEQ",
            "sidebar-instrument-name",
            Value::String(String::new()),
        );
        rt.set_reactive("SEQ", "sidebar-loaded-preset", Value::String(String::new()));
        rt.set_reactive(
            "SEQ",
            "sidebar-selected-sample",
            Value::String(selected_sample),
        );
        rt.set_reactive("SEQ", "sidebar-presets", Value::List(vec![]));
        rt.set_reactive("SEQ", "sidebar-preset-tree", Value::List(vec![]));
        return;
    }

    let instrument_name = current_custom_instrument_name(app, track).unwrap_or_default();
    let loaded_preset = app
        .state
        .pattern
        .track_sound_state
        .lock()
        .unwrap()
        .get(track)
        .and_then(|meta| meta.loaded_preset.clone())
        .unwrap_or_default();
    let preset_items = visible_preset_items_for_track(app, track);

    rt.set_reactive(
        "SEQ",
        "sidebar-kind",
        Value::String("instrument".to_string()),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-instrument-name",
        Value::String(instrument_name),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-loaded-preset",
        Value::String(loaded_preset.clone()),
    );
    rt.set_reactive(
        "SEQ",
        "sidebar-selected-sample",
        Value::String(String::new()),
    );
    rt.set_reactive("SEQ", "sidebar-presets", build_string_list(&preset_items));
    rt.set_reactive(
        "SEQ",
        "sidebar-preset-tree",
        build_flat_tree_items(&preset_items),
    );
}

fn load_instrument_preset_into_track(
    app: &mut ui::App,
    track: usize,
    preset_name: &str,
) -> Result<(), String> {
    let instrument_name = current_custom_instrument_name(app, track)
        .ok_or_else(|| "Current track is not a custom instrument".to_string())?;
    let presets = sequencer::lisp_effect::load_instrument_presets(&instrument_name)
        .map_err(|e| e.to_string())?;
    let preset = presets
        .into_iter()
        .find(|preset| preset.name == preset_name)
        .ok_or_else(|| format!("Preset '{preset_name}' not found"))?;
    let desc = app
        .graph
        .instrument_descriptors
        .get(track)
        .cloned()
        .ok_or_else(|| "Instrument descriptor unavailable".to_string())?;

    {
        let slot = &app.state.pattern.instrument_slots[track];
        for (param_idx, param) in desc.params.iter().enumerate() {
            let value = preset
                .params
                .get(&param.name)
                .copied()
                .unwrap_or(param.default);
            let clamped = param.clamp(value);
            slot.defaults.set(param_idx, clamped);
            app.send_instrument_param(track, param_idx, clamped);
        }
    }

    app.state.pattern.instrument_base_note_offsets[track]
        .store(preset.base_note_offset.to_bits(), Ordering::Relaxed);
    app.state.schedule_mod_resync();
    app.state.publish_scheduler_snapshot();
    let engine_id = app.graph.track_engine_ids.get(track).and_then(|id| *id);
    if let Some(meta) = app
        .state
        .pattern
        .track_sound_state
        .lock()
        .unwrap()
        .get_mut(track)
    {
        meta.engine_id = engine_id;
        meta.loaded_preset = Some(preset.name.clone());
        meta.dirty = false;
    }
    Ok(())
}

/// Extract the :path string from a host-command payload dict.
fn extract_path_from_payload(payload: &Value) -> Option<String> {
    if let Value::Map(map) = payload {
        if let Some(cell) = map.get("path") {
            if let Value::String(s) = &*cell.borrow() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Push individual tp-* reactive fields for the current track.
fn sync_track_params(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) {
    let tp = &state.pattern.track_params[track];
    let selected_step = {
        let sel = selected.lock().unwrap();
        sel.iter().copied().min()
    };
    rt.set_reactive("SEQ", "tp-attack", Value::Number(tp.get_attack_ms() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-release",
        Value::Number(tp.get_release_ms() as f64),
    );
    let swing = selected_step
        .and_then(|step| state.pattern.swing_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing());
    rt.set_reactive("SEQ", "tp-swing", Value::Number(swing as f64));
    rt.set_reactive("SEQ", "tp-send", Value::Number(tp.get_send() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-num-steps",
        Value::Number(tp.get_num_steps() as f64),
    );
    rt.set_reactive("SEQ", "tp-gate", Value::Bool(tp.is_gate_on()));
    rt.set_reactive("SEQ", "tp-poly", Value::Bool(tp.is_polyphonic()));
    // Resolve timebase: show p-locked value from first selected step, otherwise track default
    let timebase_label = selected_step
        .and_then(|step| state.pattern.timebase_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_timebase())
        .label()
        .to_string();
    rt.set_reactive("SEQ", "tp-timebase", Value::String(timebase_label));
    let swing_resolution = selected_step
        .and_then(|step| state.pattern.swing_resolution_plocks[track].get(step))
        .unwrap_or_else(|| tp.get_swing_resolution());
    rt.set_reactive(
        "SEQ",
        "tp-swing-resolution",
        Value::String(swing_resolution.label().to_string()),
    );
    rt.set_reactive(
        "SEQ",
        "tp-fts",
        Value::String(
            FTS_SCALE_NAMES
                .get(tp.get_fts_scale())
                .copied()
                .unwrap_or("Off")
                .to_string(),
        ),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accumulator",
        Value::String(selected_accumulator_name(app, track)),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accum-limit",
        Value::Number(tp.get_accum_limit() as f64),
    );
    rt.set_reactive(
        "SEQ",
        "tp-accum-mode",
        Value::String(accum_mode_label(tp.get_accum_mode()).to_string()),
    );
    rt.set_reactive("SEQ", "accumulator-options", build_accumulator_options(app));
    rt.set_reactive("SEQ", "fts-options", build_fts_options());
    rt.set_reactive("SEQ", "accum-mode-options", build_accum_mode_options());
}

/// Build a Lisp Value::Map of track parameters for the current track.
fn build_track_params(state: &Arc<SequencerState>, track: usize) -> Value {
    use std::collections::HashMap;
    let tp = &state.pattern.track_params[track];
    eprintln!(
        "build_track_params: track={track} attack={} gate={} vol={}",
        tp.get_attack_ms(),
        tp.is_gate_on(),
        tp.get_volume()
    );
    let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
    map.insert(
        "gate".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_gate_on()))),
    );
    map.insert(
        "attack".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_attack_ms() as f64))),
    );
    map.insert(
        "release".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_release_ms() as f64))),
    );
    map.insert(
        "swing".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_swing() as f64))),
    );
    map.insert(
        "swing-resolution".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_swing_resolution().label().to_string(),
        ))),
    );
    map.insert(
        "num-steps".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_num_steps() as f64))),
    );
    map.insert(
        "volume".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_volume() as f64))),
    );
    map.insert(
        "pan".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_pan() as f64))),
    );
    map.insert(
        "timebase".into(),
        Rc::new(RefCell::new(Value::String(
            tp.get_timebase().label().to_string(),
        ))),
    );
    map.insert(
        "send".into(),
        Rc::new(RefCell::new(Value::Number(tp.get_send() as f64))),
    );
    map.insert(
        "poly".into(),
        Rc::new(RefCell::new(Value::Bool(tp.is_polyphonic()))),
    );
    Value::Map(map)
}

/// Build a Lisp Value::List of bools indicating which steps have any p-locks on the given track.
fn build_step_has_plocks(
    state: &Arc<SequencerState>,
    track: usize,
    descriptors: &[Vec<sequencer::effects::EffectDescriptor>],
) -> Value {
    let chain = &state.pattern.effect_chains[track];
    let num_slots = descriptors.get(track).map(|d| d.len()).unwrap_or(0);
    let instrument_slot = &state.pattern.instrument_slots[track];
    let instrument_num_params = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
    let timebase_plocks = &state.pattern.timebase_plocks[track];
    let swing_plocks = &state.pattern.swing_plocks[track];
    let swing_resolution_plocks = &state.pattern.swing_resolution_plocks[track];
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| {
            let effect_has_plock = (0..num_slots).any(|slot_idx| {
                let Some(slot) = chain.get(slot_idx) else {
                    return false;
                };
                let np = slot.num_params.load(Ordering::Relaxed) as usize;
                (0..np).any(|p| slot.plocks.get(step, p).is_some())
            });
            let instrument_has_plock =
                (0..instrument_num_params).any(|p| instrument_slot.plocks.get(step, p).is_some());
            let has_plock = effect_has_plock
                || instrument_has_plock
                || timebase_plocks.has_plock(step)
                || swing_plocks.has_plock(step)
                || swing_resolution_plocks.has_plock(step);
            Rc::new(RefCell::new(Value::Bool(has_plock)))
        })
        .collect();
    Value::List(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::parser::{ASTParser, Parser, ParserError, Token};

    fn parse_expression_at(tokens: &[Token], pos: &mut usize) -> Result<(), ParserError> {
        match tokens.get(*pos) {
            Some(Token::LeftParen) => {
                *pos += 1;
                while let Some(token) = tokens.get(*pos) {
                    match token {
                        Token::RightParen => {
                            *pos += 1;
                            return Ok(());
                        }
                        _ => parse_expression_at(tokens, pos)?,
                    }
                }
                Err(ParserError::UnexpectedEOF)
            }
            Some(Token::Quote) => {
                *pos += 1;
                match tokens.get(*pos) {
                    Some(Token::Symbol(_)) => {
                        *pos += 1;
                        Ok(())
                    }
                    Some(Token::LeftParen) => parse_expression_at(tokens, pos),
                    Some(Token::Number(_))
                    | Some(Token::RightParen)
                    | Some(Token::Pipe)
                    | Some(Token::Quote)
                    | Some(Token::String(_))
                    | Some(Token::Keyword(_))
                    | Some(Token::Backtick)
                    | Some(Token::Comma) => Err(ParserError::InvalidQuote),
                    None => Err(ParserError::UnexpectedEOF),
                }
            }
            Some(Token::Pipe) => {
                *pos += 1;
                loop {
                    match tokens.get(*pos) {
                        Some(Token::Pipe) => {
                            *pos += 1;
                            break;
                        }
                        Some(Token::Symbol(_)) | Some(Token::LeftParen) => {
                            parse_expression_at(tokens, pos)?
                        }
                        Some(_) => return Err(ParserError::InvalidLambda),
                        None => return Err(ParserError::UnexpectedEOF),
                    }
                }
                parse_expression_at(tokens, pos)
            }
            Some(Token::Backtick) | Some(Token::Comma) => {
                *pos += 1;
                parse_expression_at(tokens, pos)
            }
            Some(Token::Number(_))
            | Some(Token::String(_))
            | Some(Token::Symbol(_))
            | Some(Token::Keyword(_)) => {
                *pos += 1;
                Ok(())
            }
            Some(Token::RightParen) => Err(ParserError::ExpectedLeftParen),
            None => Err(ParserError::UnexpectedEOF),
        }
    }

    #[test]
    fn metal_seq_grid_lisp_parses() {
        let src = std::fs::read_to_string("metal-seq-grid.lisp").expect("read metal-seq-grid.lisp");
        let tokens = Parser::new(src)
            .parse()
            .expect("tokenize metal-seq-grid.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-grid.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-grid.lisp");
    }

    #[test]
    fn metal_seq_piano_roll_lisp_loads() {
        let src =
            std::fs::read_to_string("metal-seq-piano-roll.lisp").expect("read piano roll lisp");
        let tokens = Parser::new(src.clone())
            .parse()
            .expect("tokenize metal-seq-piano-roll.lisp");
        let mut pos = 0;
        while pos < tokens.len() {
            if let Err(err) = parse_expression_at(&tokens, &mut pos) {
                let start = pos.saturating_sub(8);
                let end = (pos + 8).min(tokens.len());
                panic!(
                    "parse metal-seq-piano-roll.lisp at token {pos}: {err:?}\ncontext: {:?}",
                    &tokens[start..end]
                );
            }
        }
        ASTParser::new(tokens)
            .parse()
            .expect("parse metal-seq-piano-roll.lisp");

        let mut editor = eseqlisp::Editor::new(Runtime::new(), eseqlisp::EditorConfig::default());
        editor.runtime_mut().register_reactive(
            "SEQ",
            vec![
                ("playhead", Value::Number(0.0)),
                ("tp-num-steps", Value::Number(16.0)),
                ("piano-roll-lanes", build_piano_roll_lanes_value()),
                ("piano-roll-items", Value::List(vec![])),
                ("piano-roll-selection", Value::List(vec![])),
            ],
            true,
        );
        editor
            .runtime_mut()
            .register_native("seq-piano-roll-action", |_args, _ctx| Ok(Value::Bool(true)));
        editor
            .runtime_mut()
            .eval_str(&src)
            .expect("load piano roll lisp");
        editor.refresh_runtime_side_effects();
        assert!(
            editor
                .buffers
                .iter()
                .any(|buffer| buffer.name == "*piano-roll*"),
            "piano roll lisp should create the *piano-roll* buffer"
        );
        editor
            .runtime_mut()
            .eval_str("(set! piano-roll-view-duration 8)")
            .expect("set piano roll duration");
        editor
            .runtime_mut()
            .eval_str("(set! piano-roll-lane-height 1)")
            .expect("set piano roll lane height");
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :zoom-view :anchor-time 4 :factor 2))")
            .expect("zoom piano roll");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-lane-height")
                .expect("read piano roll lane height after x zoom"),
            Some(Value::Number(1.0))
        );
        editor
            .runtime_mut()
            .eval_str("(set! piano-roll-view-duration 8)")
            .expect("reset piano roll duration");
        editor
            .runtime_mut()
            .eval_str("(piano-roll-action (dict :type :scroll-view :delta-time 100))")
            .expect("scroll piano roll");
        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("piano-roll-view-start")
                .expect("read piano roll view start"),
            Some(Value::Number(12.0))
        );
    }

    #[test]
    fn piano_roll_resize_updates_only_target_chord_note_duration() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.chord_data[track].add_note_with_duration(step, 0.0, 1.0);
        state.pattern.chord_data[track].add_note_with_duration(step, 7.0, 4.0);
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 4.0);

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("time", Value::Number(4.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(state.pattern.chord_data[track].get_duration(step, 0), 2.0);
        assert_eq!(state.pattern.chord_data[track].get_duration(step, 1), 4.0);
    }

    #[test]
    fn piano_roll_preserves_half_step_duration_resolution() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 0.5);

        let items = build_piano_roll_items_value(&state, track, &selection);
        let Value::List(items) = items else {
            panic!("expected item list");
        };
        let Value::Map(item) = items[0].borrow().clone() else {
            panic!("expected item map");
        };
        assert_eq!(
            item.get("end").map(|value| value.borrow().clone()),
            Some(Value::Number(2.5))
        );

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("time", Value::Number(2.125)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Duration),
            0.125
        );
    }

    #[test]
    fn piano_roll_empty_marquee_clears_selection() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        selection.lock().unwrap().insert(piano_roll_item_id(2, 0));

        let action = map_value([
            ("type", Value::Keyword("finish-marquee-select".to_string())),
            ("time-a", Value::Number(8.0)),
            ("time-b", Value::Number(9.0)),
            ("lane-a", Value::Number(0.0)),
            ("lane-b", Value::Number(1.0)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("marquee action");

        assert!(selection.lock().unwrap().is_empty());
    }

    #[test]
    fn piano_roll_ignores_left_edge_resize_actions() {
        let state = Arc::new(SequencerState::new(1, vec![]));
        let selection = Arc::new(Mutex::new(HashSet::new()));
        let move_state = Arc::new(Mutex::new(None));
        let track = 0;
        let step = 2;
        state.pattern.patterns[track].set_step_active(step, true);
        state.pattern.step_data[track].set(step, StepParam::Duration, 4.0);

        let action = map_value([
            ("type", Value::Keyword("resize-item-absolute".to_string())),
            ("edge", Value::Keyword("start".to_string())),
            ("id", Value::Number(piano_roll_item_id(step, 0) as f64)),
            ("time", Value::Number(2.125)),
        ]);

        apply_piano_roll_action(&state, track, &selection, &move_state, &action)
            .expect("resize action");

        assert_eq!(
            state.pattern.step_data[track].get(step, StepParam::Duration),
            4.0
        );
    }
}

/// Map keyboard character to semitone offset (piano-style layout).
fn note_from_key(c: char) -> Option<i32> {
    match c {
        'a' => Some(0),  // C
        'w' => Some(1),  // C#
        's' => Some(2),  // D
        'e' => Some(3),  // D#
        'd' => Some(4),  // E
        'f' => Some(5),  // F
        't' => Some(6),  // F#
        'g' => Some(7),  // G
        'y' => Some(8),  // G#
        'h' => Some(9),  // A
        'u' => Some(10), // A#
        'j' => Some(11), // B
        'k' => Some(12), // C+1
        'o' => Some(13), // C#+1
        'l' => Some(14), // D+1
        _ => None,
    }
}

/// Intercept keyboard events for live recording.
/// Returns true if the key was consumed (music note key while armed).
fn handle_recording_key(
    key: &crossterm::event::KeyEvent,
    state: &Arc<SequencerState>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    recording: &Arc<AtomicBool>,
    keyboard_tx: &std::sync::mpsc::Sender<KeyboardTrigger>,
    keyboard_octave: &Arc<std::sync::atomic::AtomicI32>,
    current_track: &Arc<AtomicUsize>,
    held_notes: &Arc<Mutex<Vec<HeldKeyboardNote>>>,
) -> bool {
    use crossterm::event::{KeyCode, KeyEventKind};

    let c = match key.code {
        KeyCode::Char(c) => c,
        _ => return false,
    };

    // Octave shift keys (only on press)
    if c == 'z' || c == 'x' {
        if key.kind == KeyEventKind::Press {
            let delta = if c == 'z' { -12 } else { 12 };
            keyboard_octave.fetch_add(delta, Ordering::Relaxed);
        }
        return true;
    }

    let note = match note_from_key(c) {
        Some(n) => n,
        None => return false,
    };

    match key.kind {
        KeyEventKind::Press => {
            // Suppress key repeat — only trigger on first press
            let mut held = held_notes.lock().unwrap();
            if held.iter().any(|note| note.key == c) {
                return true;
            }

            let armed = record_armed.lock().unwrap();
            let octave = keyboard_octave.load(Ordering::Relaxed);
            let transpose = (note + octave) as f32;
            let mut pressed_tracks = Vec::new();

            // Send note-on to audio thread for all armed tracks
            for (track, a) in armed.iter().enumerate() {
                if *a {
                    pressed_tracks.push(track);
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track,
                        transpose,
                        velocity: 1.0,
                        note_off: false,
                    });
                }
            }

            // Record the step at press time
            let ct = current_track.load(Ordering::Relaxed);
            let playhead = state.transport.track_playheads[ct].load(Ordering::Relaxed) as usize;
            held.push(HeldKeyboardNote {
                key: c,
                transpose,
                step_at_press: playhead,
                press_time: Instant::now(),
                tracks: pressed_tracks,
            });
            true
        }
        KeyEventKind::Release => {
            // Find and remove the held note
            let held_entry = {
                let mut held = held_notes.lock().unwrap();
                let pos = held.iter().position(|note| note.key == c);
                pos.map(|idx| held.remove(idx))
            };

            // Record into pattern if recording + playing
            if let Some(note) = held_entry {
                for track in &note.tracks {
                    let _ = keyboard_tx.send(KeyboardTrigger {
                        track: *track,
                        transpose: note.transpose,
                        velocity: 0.0,
                        note_off: true,
                    });
                }

                if recording.load(Ordering::Relaxed) && state.is_playing() {
                    let armed = record_armed.lock().unwrap();
                    let bpm = state.transport.bpm.load(Ordering::Relaxed) as f64;
                    let secs_per_step = 60.0 / bpm / 4.0;
                    let hold_secs = note.press_time.elapsed().as_secs_f64();
                    let duration_steps = (hold_secs / secs_per_step).max(0.15).min(64.0) as f32;

                    for (track, a) in armed.iter().enumerate() {
                        if !*a {
                            continue;
                        }
                        let num_steps = state.pattern.track_params[track].get_num_steps();
                        let local_step = note.step_at_press % num_steps;
                        if !state.pattern.patterns[track].is_active(local_step) {
                            state.pattern.patterns[track].toggle_step(local_step);
                        }
                        state.pattern.chord_data[track].add_note(local_step, note.transpose);
                        let first_note = state.pattern.chord_data[track].get(local_step, 0);
                        state.pattern.step_data[track].set(
                            local_step,
                            StepParam::Transpose,
                            first_note,
                        );
                        state.pattern.step_data[track].set(local_step, StepParam::Velocity, 1.0);
                        state.pattern.step_data[track].set(
                            local_step,
                            StepParam::Duration,
                            duration_steps,
                        );
                    }
                    state.publish_scheduler_snapshot();
                }
            }
            true
        }
        _ => true, // consume Repeat events too
    }
}
