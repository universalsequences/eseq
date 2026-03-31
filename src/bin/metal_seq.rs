use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::Event;

use eseqlisp::backend::Backend;
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::vm::Value;
use eseqlisp::{Editor, EditorConfig, HostCommand, HostEvent, Runtime};

use sequencer::engine;
use sequencer::sequencer::{
    KeyboardTrigger, SequencerState, StepParam, SwingResolution, Timebase, MAX_STEPS,
    SYNC_RESOLUTIONS,
};
use sequencer::ui;
use std::sync::atomic::AtomicBool;

const DEFAULT_SAMPLES: &[&str] = &[
    "samples/producers/Boom-Bap/Boom-Bap Kick 51.wav",
    "samples/producers/madlib/Snare SwaggedOut 3.wav",
    "samples/producers/donda/PABLO HAT.wav",
];
const PAGE_SIZE: usize = 16;

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
    // UI-only counter for changes that shouldn't affect pattern_epoch (e.g. volume, selection)
    let ui_epoch = Arc::new(AtomicUsize::new(0));

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

    // Register SEQ reactive namespace
    runtime.register_reactive(
        "SEQ",
        vec![
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
            ("playhead", Value::Number(0.0)),
            ("track-names", build_track_names(&track_names)),
            ("steps", build_steps_value(&state, 0)),
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
            ("available-effects", build_available_effects()),
            ("selected-steps", build_selection_value(&selected_steps)),
            (
                "step-has-plocks",
                build_step_has_plocks(&state, 0, &effect_descriptors),
            ),
            ("compiling", Value::Bool(false)),
            ("recording", Value::Bool(false)),
            (
                "record-armed",
                build_record_armed_value(&record_armed.lock().unwrap()),
            ),
            ("sidebar-kind", Value::String("sampler".to_string())),
            ("sidebar-instrument-name", Value::String(String::new())),
            ("sidebar-loaded-preset", Value::String(String::new())),
            ("sidebar-selected-sample", Value::String(String::new())),
            ("sidebar-presets", Value::List(vec![])),
            ("sidebar-preset-tree", Value::List(vec![])),
        ],
        false,
    );

    // ── Native functions ──

    // seq-toggle-step — toggle step on current track
    let st = state.clone();
    let ct = current_track.clone();
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
        Ok(Value::Bool(st.pattern.patterns[track].is_active(step)))
    });

    // seq-set-step-param — set param on current track
    let st = state.clone();
    let ct = current_track.clone();
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
        Ok(Value::Number(val as f64))
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(clamped as f64))
    });

    // ── Selection natives ──

    // seq-select-step — toggle step in/out of selection
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
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
        Ok(Value::Bool(!was_selected))
    });

    // seq-clear-selection
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-clear-selection", move |_args, _ctx| {
        sel.lock().unwrap().clear();
        ui_ep.fetch_add(1, Ordering::Relaxed);
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
    runtime.register_native("seq-select-all-steps", move |_args, _ctx| {
        let track = ct.load(Ordering::Relaxed);
        let num_steps = st.pattern.track_params[track].get_num_steps();
        let mut set = sel.lock().unwrap();
        set.clear();
        set.extend(0..num_steps);
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(num_steps as f64))
    });

    // seq-delete-selected-steps — clear all selected steps and clear selection
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(steps.len() as f64))
    });

    // seq-shift-selected-steps — rotate selected step payloads left/right in place
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
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
        st.rotate_steps(track, &steps, direction.signum());
        st.publish_scheduler_snapshot();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Bool(true))
    });

    // seq-set-effect-plock — apply p-lock to ALL selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::Number(val as f64))
    });

    // seq-set-step-param-plock — apply step param p-lock to selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
    runtime.register_native("seq-set-step-param-plock", move |args, _ctx| {
        let (Some(Value::Keyword(param_name)), Some(Value::Number(val))) =
            (args.first(), args.get(1))
        else {
            return Err("seq-set-step-param-plock: expected (:param value)".into());
        };
        let param = match param_name.as_str() {
            "velocity" | "vel" => StepParam::Velocity,
            "duration" | "dur" => StepParam::Duration,
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
    let ui_ep = ui_epoch.clone();
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
                tp.set_swing(v);
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
            ui_ep.fetch_add(1, Ordering::Relaxed);
            v
        })
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

    // seq-set-timebase — set the default timebase for the current track (by label string)
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    // seq-plock-timebase — set a timebase p-lock on selected steps
    let st = state.clone();
    let ct = current_track.clone();
    let sel = selected_steps.clone();
    let ui_ep = ui_epoch.clone();
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
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(tb.label().to_string()))
    });

    // seq-set-swing-resolution — set the default swing resolution for the current track (by label string)
    let st = state.clone();
    let ct = current_track.clone();
    let ui_ep = ui_epoch.clone();
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
        st.pattern.track_params[track].set_swing_resolution(resolution);
        st.publish_scheduler_snapshot();
        ui_ep.fetch_add(1, Ordering::Relaxed);
        Ok(Value::String(resolution.label().to_string()))
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

    let mut backend = MetalBackend::new().map_err(|_| "Metal backend creation failed")?;
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

    let mut prev_playing = false;
    let mut prev_bpm: u32 = 0;
    let mut prev_playhead: u32 = u32::MAX;
    let mut prev_pattern_epoch: u64 = 0;
    let mut prev_snapshot_version: u64 = 0;
    let mut prev_current_track: usize = usize::MAX;
    let mut prev_ui_epoch: usize = 0;

    eprintln!("metal_seq: entering event loop");

    loop {
        editor.update_timers();
        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);

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
        match backend.poll_event(timeout) {
            Some(Event::Key(key)) => {
                // Intercept keyboard for live recording when any track is armed
                let any_armed = record_armed.lock().unwrap().iter().any(|a| *a);
                let intercepted = if any_armed {
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
            Some(Event::Mouse(mouse)) => {
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
            Some(Event::Resize(_, _)) => editor.mark_needs_redraw(),
            _ => {}
        }

        // Touchpad gestures
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

        // Flush pending drag at frame boundary
        if last_render_at.elapsed() >= frame_interval {
            if let Some((Event::Mouse(mouse), (precise_col, precise_row))) = pending_drag.take() {
                editor.handle_tiled_mouse_precise(mouse, precise_col, precise_row, 0);
            }
        }

        // 1b. Drain host commands (sample browser etc.)
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "velocities",
                                        build_param_list(&state, idx, StepParam::Velocity),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "durations",
                                        build_param_list(&state, idx, StepParam::Duration),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "transposes",
                                        build_param_list(&state, idx, StepParam::Transpose),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "pans",
                                        build_param_list(&state, idx, StepParam::Pan),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "syncs",
                                        build_param_list(&state, idx, StepParam::Sync),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-volumes",
                                        build_track_volumes(&state),
                                    );
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
                                    sync_track_params(rt, &state, idx, &selected_steps);
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
                                            rt.set_reactive(
                                                "SEQ",
                                                "velocities",
                                                build_param_list(&state, idx, StepParam::Velocity),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "durations",
                                                build_param_list(&state, idx, StepParam::Duration),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "transposes",
                                                build_param_list(&state, idx, StepParam::Transpose),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "pans",
                                                build_param_list(&state, idx, StepParam::Pan),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "syncs",
                                                build_param_list(&state, idx, StepParam::Sync),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "track-volumes",
                                                build_track_volumes(&state),
                                            );
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
                                            sync_track_params(rt, &state, idx, &selected_steps);
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
                                        let steps: Vec<usize> =
                                            selected_steps.lock().unwrap().iter().copied().collect();
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
                                    sync_pattern_state(rt, &state);
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                                    rt.set_reactive(
                                        "SEQ",
                                        "velocities",
                                        build_param_list(&state, ct, StepParam::Velocity),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "durations",
                                        build_param_list(&state, ct, StepParam::Duration),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "transposes",
                                        build_param_list(&state, ct, StepParam::Transpose),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "pans",
                                        build_param_list(&state, ct, StepParam::Pan),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "syncs",
                                        build_param_list(&state, ct, StepParam::Sync),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-volumes",
                                        build_track_volumes(&state),
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, ct, &selected_steps),
                                    );
                                    sync_track_params(rt, &state, ct, &selected_steps);
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
                            sync_pattern_state(rt, &state);
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                            rt.set_reactive(
                                "SEQ",
                                "velocities",
                                build_param_list(&state, ct, StepParam::Velocity),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "durations",
                                build_param_list(&state, ct, StepParam::Duration),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "transposes",
                                build_param_list(&state, ct, StepParam::Transpose),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "pans",
                                build_param_list(&state, ct, StepParam::Pan),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "syncs",
                                build_param_list(&state, ct, StepParam::Sync),
                            );
                            rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
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
                            sync_track_params(rt, &state, ct, &selected_steps);
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
                    other => {
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Unknown host command: {other}"
                        )));
                    }
                }
            }
        }

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
            editor.handle_host_event(HostEvent::Status(status));
        }

        // 2. Sync reactive state AFTER events
        let ct = current_track.load(Ordering::Relaxed);
        {
            let playing = state.transport.playing.load(Ordering::Relaxed);
            let bpm = state.transport.bpm.load(Ordering::Relaxed);
            let playhead = state.transport.track_playheads[ct].load(Ordering::Relaxed);
            let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            let snap_ver = state.scheduler_snapshot_version();

            let mut needs_reactive_cycle = false;

            // Track switch — rebuild everything
            if ct != prev_current_track {
                editor.reset_widget_scroll_left();
                let rt = editor.runtime_mut();
                sync_pattern_state(rt, &state);
                rt.set_reactive("SEQ", "current-track", Value::Number(ct as f64));
                rt.set_reactive("SEQ", "playhead", Value::Number(playhead as f64));
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                rt.set_reactive(
                    "SEQ",
                    "velocities",
                    build_param_list(&state, ct, StepParam::Velocity),
                );
                rt.set_reactive(
                    "SEQ",
                    "durations",
                    build_param_list(&state, ct, StepParam::Duration),
                );
                rt.set_reactive(
                    "SEQ",
                    "transposes",
                    build_param_list(&state, ct, StepParam::Transpose),
                );
                rt.set_reactive("SEQ", "pans", build_param_list(&state, ct, StepParam::Pan));
                rt.set_reactive(
                    "SEQ",
                    "syncs",
                    build_param_list(&state, ct, StepParam::Sync),
                );
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
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
                sync_track_params(rt, &state, ct, &selected_steps);
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                prev_current_track = ct;
                prev_playhead = playhead;
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
            if epoch != prev_pattern_epoch || snap_ver != prev_snapshot_version {
                let rt = editor.runtime_mut();
                sync_pattern_state(rt, &state);
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                rt.set_reactive(
                    "SEQ",
                    "velocities",
                    build_param_list(&state, ct, StepParam::Velocity),
                );
                rt.set_reactive(
                    "SEQ",
                    "durations",
                    build_param_list(&state, ct, StepParam::Duration),
                );
                rt.set_reactive(
                    "SEQ",
                    "transposes",
                    build_param_list(&state, ct, StepParam::Transpose),
                );
                rt.set_reactive("SEQ", "pans", build_param_list(&state, ct, StepParam::Pan));
                rt.set_reactive(
                    "SEQ",
                    "syncs",
                    build_param_list(&state, ct, StepParam::Sync),
                );
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
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
                sync_track_params(rt, &state, ct, &selected_steps);
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
                rt.set_reactive("SEQ", "track-volumes", build_track_volumes(&state));
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
                sync_track_params(rt, &state, ct, &selected_steps);
                rt.set_reactive(
                    "SEQ",
                    "selected-steps",
                    build_selection_value(&selected_steps),
                );
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
            if playhead != prev_playhead {
                editor.runtime_mut().set_reactive(
                    "SEQ",
                    "playhead",
                    Value::Number(playhead as f64),
                );
                prev_playhead = playhead;
                needs_reactive_cycle = true;
            }

            if needs_reactive_cycle {
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                editor.mark_needs_redraw();
            }
        }

        // Always redraw when selection is active (for itime animation on selected steps)
        if !selected_steps.lock().unwrap().is_empty() {
            editor.mark_needs_redraw();
        }

        // Render
        if editor.needs_redraw() && last_render_at.elapsed() >= frame_interval {
            let tiled_frame =
                eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, cols, rows);
            backend
                .render_tiled(&tiled_frame)
                .map_err(|_| "render failed")?;
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

/// Build a Lisp Value::List of bools for record-armed state per track.
fn build_record_armed_value(armed: &[bool]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = armed
        .iter()
        .map(|a| Rc::new(RefCell::new(Value::Bool(*a))))
        .collect();
    Value::List(items)
}

/// Build a Lisp Value::List of track name strings.
fn build_track_names(names: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = names
        .iter()
        .map(|name| Rc::new(RefCell::new(Value::String(name.clone()))))
        .collect();
    Value::List(items)
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
        return Value::List(vec![]);
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
fn build_available_effects() -> Value {
    let names = sequencer::lisp_effect::list_saved_effects();
    let items: Vec<Rc<RefCell<Value>>> = names
        .into_iter()
        .map(|n| Rc::new(RefCell::new(Value::String(n))))
        .collect();
    Value::List(items)
}

fn build_string_list(items: &[String]) -> Value {
    let items: Vec<Rc<RefCell<Value>>> = items
        .iter()
        .map(|item| Rc::new(RefCell::new(Value::String(item.clone()))))
        .collect();
    Value::List(items)
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
    state: &Arc<SequencerState>,
    track: usize,
    selected: &Arc<Mutex<HashSet<usize>>>,
) {
    let tp = &state.pattern.track_params[track];
    rt.set_reactive("SEQ", "tp-attack", Value::Number(tp.get_attack_ms() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-release",
        Value::Number(tp.get_release_ms() as f64),
    );
    rt.set_reactive("SEQ", "tp-swing", Value::Number(tp.get_swing() as f64));
    rt.set_reactive("SEQ", "tp-send", Value::Number(tp.get_send() as f64));
    rt.set_reactive(
        "SEQ",
        "tp-num-steps",
        Value::Number(tp.get_num_steps() as f64),
    );
    rt.set_reactive("SEQ", "tp-gate", Value::Bool(tp.is_gate_on()));
    rt.set_reactive("SEQ", "tp-poly", Value::Bool(tp.is_polyphonic()));
    // Resolve timebase: show p-locked value from first selected step, otherwise track default
    let timebase_label = {
        let sel = selected.lock().unwrap();
        sel.iter()
            .copied()
            .min()
            .and_then(|step| state.pattern.timebase_plocks[track].get(step))
            .unwrap_or_else(|| tp.get_timebase())
            .label()
            .to_string()
    };
    rt.set_reactive("SEQ", "tp-timebase", Value::String(timebase_label));
    rt.set_reactive(
        "SEQ",
        "tp-swing-resolution",
        Value::String(tp.get_swing_resolution().label().to_string()),
    );
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
    let items: Vec<Rc<RefCell<Value>>> = (0..MAX_STEPS)
        .map(|step| {
            let has_plock = (0..num_slots).any(|slot_idx| {
                let Some(slot) = chain.get(slot_idx) else {
                    return false;
                };
                let np = slot.num_params.load(Ordering::Relaxed) as usize;
                (0..np).any(|p| slot.plocks.get(step, p).is_some())
            });
            Rc::new(RefCell::new(Value::Bool(has_plock)))
        })
        .collect();
    Value::List(items)
}

#[cfg(test)]
mod tests {
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
