mod arrangement_actions;
mod browser;
mod capture;
mod constants;
mod custom_ui;
mod editor_setup;
mod host_commands;
mod input;
mod lisp_hot_reload;
mod live_audio_analyzer;
mod natives;
mod piano_roll;
mod patch_learn;
mod profile;
mod roll_record;
mod step_print;
mod sample_import_ui;
mod sampler_monitor;
mod state_values;
mod ui_invalidation;
mod values;

use browser::*;
use constants::*;
use custom_ui::*;
use editor_setup::*;
use host_commands::*;
use input::*;
use lisp_hot_reload::*;
use live_audio_analyzer::*;
use natives::*;
use piano_roll::*;
use patch_learn::*;
use profile::*;
use roll_record::*;
use step_print::*;
use sample_import_ui::*;
use sampler_monitor::*;
use state_values::*;
use ui_invalidation::*;
use values::*;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::Event;

use eseqlisp::backend::{Backend, BackendEvent};

// The render backend behind the app shell: Metal on macOS, wgpu elsewhere.
// Both expose the same inherent API, so everything downstream of this alias
// is platform-neutral.
#[cfg(target_os = "macos")]
pub(crate) use eseqlisp::metal_backend::{MetalBackend as AppBackend, TiledRenderStatus};
#[cfg(not(target_os = "macos"))]
pub(crate) use eseqlisp::wgpu_app::{TiledRenderStatus, WgpuAppBackend as AppBackend};
use eseqlisp::editor::ViewMode;
use eseqlisp::parser::{ASTParser, Expression, Parser};
use eseqlisp::vm::Value;
use eseqlisp::{BufferMode, Editor, HostCommand, HostEvent, Runtime};

use sequencer::agent::actions::{
    AgentInstrumentParamSchema, AgentInstrumentPresetSchema, AgentSessionContext,
};
use sequencer::effects::{ParamKind, ParamScaling};
use sequencer::engine;
use sequencer::sequencer::{
    CustomInstrumentRunMode, InstrumentSlotResetSummary, InstrumentType, KeyboardTrigger,
    MidiFxPosition, PatternId, RackSlotParam, SequencerState, StepParam, SwingResolution, Timebase,
    TrackId, TrackOutput, TrackSendSnapshot, MAX_STEPS, SYNC_RESOLUTIONS,
};
use sequencer::app;
use std::sync::atomic::AtomicBool;

mod agent_finalize;
mod edit_sessions;
mod history_commands;
mod event_loop;
mod loop_ctx;
mod scroll_inertia;
mod reactive_tick;
mod reactive_sync;

use agent_finalize::*;
use edit_sessions::*;
use history_commands::*;
use event_loop::*;
use loop_ctx::*;
use reactive_tick::*;
use reactive_sync::*;

#[cfg(test)]
mod tests;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let capture_args = capture::CaptureArgs::parse_env()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let app_paths = sequencer::app_paths::init()?;
    // Checkout-only startup work: the chdir into the crate directory, and the
    // live shader-override watch that reads this workspace's shader sources.
    // Both are absent in a bundle — the chdir fails outright and the watch
    // would `fs::metadata` a nonexistent path on every rendered frame.
    eseqlisp::ui::set_editable_shader_overrides_enabled(!app_paths.is_release());
    if !app_paths.is_release() {
        sequencer::paths::enter_sequencer_dir()?;
    }
    app_paths.ensure_user_tier()?;
    match sequencer::package_samples::reconcile_app_package_samples(app_paths) {
        Ok(report) => {
            for error in report.errors {
                eprintln!("metal_seq: {error}");
            }
        }
        Err(error) => eprintln!("metal_seq: failed to reconcile package samples: {error}"),
    }
    sequencer::crash::install()?;

    if let Some(args) = capture_args {
        return capture::run(args);
    }

    // 1. Init audio engine
    let eng = engine::init_engine()?;
    let lg_ptr = eng.lg_ptr;
    let state = eng.state.clone();
    let stream = eng._stream;

    // 2. Create App. Start intentionally empty so the first action is choosing
    // a sound instead of editing a canned pattern.
    let master_recorder = eng.master_recorder.clone();
    let mut app = app::App::new(
        eng.state.clone(),
        eng.lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder,
        eng.keyboard_tx,
    );

    let track_names: Vec<String> = Vec::new();

    // Collect node IDs for param pushing to audiograph
    let track_pan_ids: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(
        app.graph.track_node_ids.iter().map(|n| n.pan_id).collect(),
    ));
    let track_collapsed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(app.track_collapsed.clone()));
    let bus_state: Arc<Mutex<Vec<app::BusChannelState>>> = Arc::new(Mutex::new(app.buses.clone()));
    let bus_node_ids: Arc<Mutex<Vec<app::BusNodeIds>>> =
        Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
    let lg_raw = lg_ptr.0;

    // Shared current track index
    let current_track = Arc::new(AtomicUsize::new(0));
    // Multi-select set for mixer group operations (includes current_track when non-empty)
    let selected_tracks: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
    // Shared in-memory track groups (mirror of app.groups, mutated by natives)
    let track_groups: Arc<Mutex<Vec<sequencer::project::ProjectTrackGroup>>> =
        Arc::new(Mutex::new(app.groups.clone()));
    // Selected steps for p-locking
    let selected_steps: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
    let selected_neural_neurons: sequencer::lisp_host::SharedSelectedNeuralNeurons =
        Arc::new(Mutex::new(BTreeSet::new()));
    let piano_roll_selection: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>> = Arc::new(Mutex::new(None));
    let piano_roll_focus = new_shared_piano_roll_focus();
    let step_clipboard: Arc<
        Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>,
    > = Arc::new(Mutex::new(None));
    // UI-only counter for changes that shouldn't affect pattern_epoch (e.g. volume, selection)
    let ui_epoch = Arc::new(AtomicUsize::new(0));
    // FX/instrument panel refresh counter for changes that affect *fx* but
    // should not force *fx* to rerun on unrelated step-grid edits.
    let fx_epoch = Arc::new(AtomicUsize::new(0));
    // Value-only fx refresh counter for scene/clip launches: rides the tick's
    // in-place value-patch path instead of a full *fx* re-eval.
    let fx_value_epoch = Arc::new(AtomicUsize::new(0));
    let ui_invalidations = Arc::new(UiInvalidationQueue::new());
    let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
    let active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>> = Arc::new(Mutex::new(None));
    let active_delete_target_version = Arc::new(AtomicUsize::new(0));
    // When set, pagination stays on the user-selected page until the cooldown expires.
    let auto_follow_override_until: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    // Recording state shared between native functions and event loop
    let recording = Arc::new(AtomicBool::new(false));
    let master_recording = Arc::new(AtomicBool::new(false));
    let record_armed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(vec![false; track_names.len()]));
    // Drum rack v2 pad-play arm: the group id of the rack the live keyboard
    // plays as pads, if any (docs/drum-rack-v2-spec.md, "Arming & live play").
    let armed_rack: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
    // Keyboard trigger sender for live playing when armed
    let keyboard_tx = app.graph.keyboard_tx.clone();
    // Keyboard octave offset for live playing
    let keyboard_octave = Arc::new(std::sync::atomic::AtomicI32::new(0));
    // Held keys for recording: (key_char, transpose, step_at_press, press_instant)
    let held_notes: Arc<Mutex<Vec<HeldKeyboardNote>>> = Arc::new(Mutex::new(Vec::new()));

    let RuntimeInit {
        runtime,
        accumulator_names,
        midi_fx_names: _,
        sample_browser,
        piano_roll_clipboard,
        process_authoring,
    } = init_runtime(
        &app,
        state.clone(),
        &track_names,
        track_pan_ids.clone(),
        track_collapsed.clone(),
        bus_state.clone(),
        bus_node_ids.clone(),
        current_track.clone(),
        selected_tracks.clone(),
        track_groups.clone(),
        selected_steps.clone(),
        piano_roll_selection.clone(),
        piano_roll_move_state.clone(),
        piano_roll_focus.clone(),
        recording.clone(),
        master_recording.clone(),
        master_recorder.clone(),
        record_armed.clone(),
        armed_rack.clone(),
        ui_epoch.clone(),
        fx_epoch.clone(),
        ui_invalidations.clone(),
        expanded_step_projection.clone(),
        selected_neural_neurons.clone(),
        active_delete_target.clone(),
        active_delete_target_version.clone(),
        auto_follow_override_until.clone(),
        lg_raw,
    );

    // A project switch resets this registry, so `App` needs the handle
    // (bead eseq-jo7.21).
    app.editor.ui_process_authoring = Some(process_authoring);

    let (editor, backend) = create_editor_and_backend(runtime, &app)?;

    // Cheaply clonable mirrors of the handles init_runtime captured, bundled
    // for the extracted host-command dispatcher.
    let shared = SharedHandles {
        state: state.clone(),
        lg_raw,
        current_track: current_track.clone(),
        selected_tracks: selected_tracks.clone(),
        selected_steps: selected_steps.clone(),
        selected_neural_neurons: selected_neural_neurons.clone(),
        piano_roll_selection: piano_roll_selection.clone(),
        piano_roll_move_state: piano_roll_move_state.clone(),
        piano_roll_focus: piano_roll_focus.clone(),
        step_clipboard: step_clipboard.clone(),
        ui_epoch: ui_epoch.clone(),
        fx_epoch: fx_epoch.clone(),
        fx_value_epoch: fx_value_epoch.clone(),
        ui_invalidations: ui_invalidations.clone(),
        expanded_step_projection: expanded_step_projection.clone(),
        active_delete_target: active_delete_target.clone(),
        active_delete_target_version: active_delete_target_version.clone(),
        auto_follow_override_until: auto_follow_override_until.clone(),
        track_pan_ids: track_pan_ids.clone(),
        track_collapsed: track_collapsed.clone(),
        bus_state: bus_state.clone(),
        bus_node_ids: bus_node_ids.clone(),
        track_groups: track_groups.clone(),
        record_armed: record_armed.clone(),
        armed_rack: armed_rack.clone(),
        recording: recording.clone(),
        master_recording: master_recording.clone(),
        held_notes: held_notes.clone(),
        roll_record: Arc::new(Mutex::new(RollRecordBuffer::default())),
        step_print: Arc::new(Mutex::new(StepPrintState::default())),
        keyboard_octave: keyboard_octave.clone(),
        sample_browser: sample_browser.clone(),
        keyboard_tx: keyboard_tx.clone(),
        accumulator_names: accumulator_names.clone(),
        piano_roll_clipboard: piano_roll_clipboard.clone(),
        arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
    };

    run_event_loop(app, editor, backend, track_names, shared)?;

    drop(stream);
    unsafe {
        sequencer::audiograph::clear_os_workgroup();
        sequencer::audiograph::engine_stop_workers();
        sequencer::audiograph::destroy_live_graph(lg_ptr.0);
    }

    Ok(())
}
