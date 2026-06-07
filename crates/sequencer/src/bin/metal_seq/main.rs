#![allow(
    dead_code,
    clippy::inspect_for_each,
    clippy::manual_clamp,
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::useless_conversion,
    clippy::useless_format
)]

mod browser;
mod constants;
mod custom_ui;
mod editor_setup;
mod host_commands;
mod input;
mod lisp_hot_reload;
mod natives;
mod piano_roll;
mod profile;
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
use natives::*;
use piano_roll::*;
use profile::*;
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

use eseqlisp::backend::Backend;
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
    CustomInstrumentRunMode, KeyboardTrigger, MidiFxPosition, PatternId, SequencerState, StepParam,
    SwingResolution, Timebase, TrackOutput, TrackSendSnapshot, MAX_STEPS, SYNC_RESOLUTIONS,
};
use sequencer::ui;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FxDeleteChain {
    Audio,
    Midi,
    Bus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActiveDeleteTarget {
    MixerTrack {
        track: usize,
    },
    TrackPattern {
        track: usize,
        pattern_id: PatternId,
    },
    ModRoute {
        source: usize,
        dest: usize,
        input: usize,
    },
    FxEffect {
        chain: FxDeleteChain,
        bus: Option<usize>,
        slot: usize,
    },
}

#[derive(Debug, Clone)]
enum InstrumentEditMode {
    EditExisting {
        persisted_source: String,
    },
    CreateDraft {
        temp_dir: PathBuf,
        draft_track: usize,
        original_track: usize,
    },
}

#[derive(Debug, Clone)]
struct InstrumentEditSession {
    name: String,
    path: PathBuf,
    buffer_name: String,
    engine_id: usize,
    track: usize,
    last_valid_source: String,
    last_valid_layout: Option<String>,
    visible_revision_valid: bool,
    preview_generation: u64,
    run_mode: CustomInstrumentRunMode,
    mode: InstrumentEditMode,
}

impl InstrumentEditSession {
    fn begin_edit_existing(
        name: String,
        path: PathBuf,
        buffer_name: String,
        engine_id: usize,
        track: usize,
        persisted_source: String,
        run_mode: CustomInstrumentRunMode,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            engine_id,
            track,
            last_valid_source: persisted_source.clone(),
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            run_mode,
            mode: InstrumentEditMode::EditExisting { persisted_source },
        }
    }

    fn begin_create_draft(
        name: String,
        path: PathBuf,
        buffer_name: String,
        engine_id: usize,
        source: String,
        temp_dir: PathBuf,
        draft_track: usize,
        original_track: usize,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            engine_id,
            track: draft_track,
            last_valid_source: source,
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            run_mode: CustomInstrumentRunMode::Instrument,
            mode: InstrumentEditMode::CreateDraft {
                temp_dir,
                draft_track,
                original_track,
            },
        }
    }
}

struct PendingInstrumentPreview {
    generation: u64,
    source: String,
    layout: Option<String>,
    receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_effect::CompileResult, String>>,
}

struct PendingInstrumentCancelRestore {
    session: InstrumentEditSession,
    persisted_source: String,
    receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_effect::CompileResult, String>>,
}

#[derive(Debug, Clone)]
enum EffectEditTarget {
    Track { track: usize, slot: usize },
    Bus { bus: usize, slot: usize },
}

#[derive(Debug, Clone)]
enum EffectEditMode {
    EditExisting { persisted_source: String },
    CreateDraft { temp_dir: PathBuf },
}

#[derive(Debug, Clone)]
struct EffectEditSession {
    name: String,
    path: PathBuf,
    buffer_name: String,
    target: EffectEditTarget,
    last_valid_source: String,
    last_valid_layout: Option<String>,
    visible_revision_valid: bool,
    preview_generation: u64,
    mode: EffectEditMode,
}

impl EffectEditSession {
    fn begin_edit_existing(
        name: String,
        path: PathBuf,
        buffer_name: String,
        target: EffectEditTarget,
        persisted_source: String,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            target,
            last_valid_source: persisted_source.clone(),
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            mode: EffectEditMode::EditExisting { persisted_source },
        }
    }

    fn begin_create_draft(
        name: String,
        path: PathBuf,
        buffer_name: String,
        target: EffectEditTarget,
        source: String,
        temp_dir: PathBuf,
    ) -> Self {
        Self {
            name,
            path,
            buffer_name,
            target,
            last_valid_source: source,
            last_valid_layout: None,
            visible_revision_valid: true,
            preview_generation: 0,
            mode: EffectEditMode::CreateDraft { temp_dir },
        }
    }
}

struct PendingEffectPreview {
    generation: u64,
    source: String,
    layout: Option<String>,
    receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_effect::CompileResult, String>>,
}

struct PendingEffectCancelRestore {
    session: EffectEditSession,
    receiver: std::sync::mpsc::Receiver<Result<sequencer::lisp_effect::CompileResult, String>>,
}

struct PendingAgenticBubble {
    path: PathBuf,
    intent: eseqlisp::widget_render::patcher::PatcherIntent,
    bubble_id: String,
    generation: u64,
    receiver: std::sync::mpsc::Receiver<
        Result<sequencer::agent::agentic_bubble::AgenticBubbleOutput, String>,
    >,
}

fn extract_macro_name_from_defmacro(source: &str) -> Option<String> {
    let tokens = Parser::new(source.to_string()).parse().ok()?;
    let exprs = ASTParser::new(tokens).parse().ok()?;
    let Expression::List(items) = exprs.first()? else {
        return None;
    };
    match items.as_slice() {
        [Expression::Symbol(head), Expression::Symbol(name), ..] if head == "defmacro" => {
            Some(name.clone())
        }
        _ => None,
    }
}

fn instrument_patcher_buffer_source(buffer_name: &str, path: &Path) -> String {
    let buffer_name = escape_lisp_string(buffer_name);
    let path = escape_lisp_string(&path.to_string_lossy());
    format!(
        "(effect-buffer \"{buffer_name}\"\n  (patcher\n    :intent :instrument\n    :width :fill\n    :height :fill\n    :path \"{path}\"\n    :on-change (lambda (event)\n      (host-command \"preview-instrument-patch\" event))))\n"
    )
}

fn effect_patcher_buffer_source(buffer_name: &str, path: &Path) -> String {
    let buffer_name = escape_lisp_string(buffer_name);
    let path = escape_lisp_string(&path.to_string_lossy());
    format!(
        "(effect-buffer \"{buffer_name}\"\n  (patcher\n    :intent :effect\n    :width :fill\n    :height :fill\n    :path \"{path}\"\n    :on-change (lambda (event)\n      (host-command \"preview-effect-patch\" event))))\n"
    )
}

const NEW_INSTRUMENT_DRAFT_NAME: &str = "new-instrument-draft/";
const NEW_EFFECT_DRAFT_NAME: &str = "new-effect-draft/";

const NEW_INSTRUMENT_STARTER_DSP: &str = r#"(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param attack @group amp @env amp-env @role attack @default 5 @min 0 @max 1000 @unit ms)
(param decay @group amp @env amp-env @role decay @default 120 @min 1 @max 2000 @unit ms)
(param sustain @group amp @env amp-env @role sustain @default 0.8 @min 0 @max 1)
(param release @group amp @env amp-env @role release @default 180 @min 1 @max 5000 @unit ms)
(param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

(def env (adsr gate trigger attack decay sustain release))
(def phase (phasor pitch))
(out (* phase env velocity (mod gain)) 1 @name audio)
"#;

fn create_new_instrument_draft_dir() -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let dir = std::env::temp_dir()
        .join("eseq-instrument-drafts")
        .join(format!("draft-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create draft instrument directory: {error}"))?;
    Ok(dir)
}

fn create_new_effect_draft_dir() -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System clock is before UNIX epoch: {error}"))?
        .as_nanos();
    let dir = std::env::temp_dir()
        .join("eseq-effect-drafts")
        .join(format!("draft-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create draft effect directory: {error}"))?;
    Ok(dir)
}

fn instrument_run_mode_label(run_mode: CustomInstrumentRunMode) -> &'static str {
    match run_mode {
        CustomInstrumentRunMode::Instrument => "instrument",
        CustomInstrumentRunMode::FreePatch => "free_patch",
    }
}

fn instrument_run_mode_from_label(label: &str) -> Option<CustomInstrumentRunMode> {
    CustomInstrumentRunMode::parse(label)
}

fn show_instrument_patcher_layout_source(buffer_name: &str) -> String {
    let buffer_name = escape_lisp_string(buffer_name);
    format!("(seq-apply-instrument-patcher-layout \"{buffer_name}\")")
}

fn show_instrument_patcher_source_layout_source(
    patcher_buffer_name: &str,
    source_buffer_name: &str,
) -> String {
    let patcher_buffer_name = escape_lisp_string(patcher_buffer_name);
    let source_buffer_name = escape_lisp_string(source_buffer_name);
    format!(
        "(seq-apply-instrument-patcher-source-layout \"{patcher_buffer_name}\" \"{source_buffer_name}\")"
    )
}

fn restore_instrument_patcher_layout_source() -> &'static str {
    "(seq-restore-instrument-patcher-layout)"
}

fn reset_instrument_patcher_state(path: &Path) {
    eseqlisp::widget_render::patcher::reset_patcher_state_for_path(
        path,
        eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
    );
}

fn reset_effect_patcher_state(path: &Path) {
    eseqlisp::widget_render::patcher::reset_patcher_state_for_path(
        path,
        eseqlisp::widget_render::patcher::PatcherIntent::Effect,
    );
}

fn escape_lisp_string(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}

fn preserve_sample_browser_context_for_loaded_sample(editor: &mut Editor, path: &str) {
    let path = escape_lisp_string(path);
    if let Err(error) = editor
        .runtime_mut()
        .eval_str(&format!("(set! sbrowser-auditioned-sample \"{path}\")"))
    {
        eprintln!("sample browser: failed to mark browser-initiated sample load: {error:?}");
    }
}

fn refresh_sample_browser_buffer(editor: &mut Editor) -> Result<(), String> {
    editor
        .runtime_mut()
        .eval_str("(sbrowser-refresh-buffer)")
        .map_err(|error| format!("{error:?}"))?;
    editor.refresh_runtime_side_effects();
    editor.refresh_visible_layouts_for_buffer_named("*samples*");
    editor.mark_needs_redraw();
    Ok(())
}

impl ActiveDeleteTarget {
    fn buffer_name(&self) -> &'static str {
        match self {
            ActiveDeleteTarget::MixerTrack { .. }
            | ActiveDeleteTarget::TrackPattern { .. }
            | ActiveDeleteTarget::ModRoute { .. } => "*mixer*",
            ActiveDeleteTarget::FxEffect { .. } => "*fx*",
        }
    }
}

fn should_clear_active_delete_target_for_buffer(
    target: Option<&ActiveDeleteTarget>,
    active_buffer_name: &str,
) -> bool {
    target.is_some_and(|target| target.buffer_name() != active_buffer_name)
}

fn pull_shared_bus_state(
    app: &mut ui::App,
    bus_state: &Arc<Mutex<Vec<ui::BusChannelState>>>,
) -> bool {
    let latest = bus_state.lock().unwrap().clone();
    if app.buses.len() != latest.len()
        || app
            .buses
            .iter()
            .zip(latest.iter())
            .any(|(a, b)| a.volume != b.volume || a.mute != b.mute || a.solo != b.solo)
    {
        if app.buses.len() == latest.len() {
            for (bus, latest_bus) in app.buses.iter_mut().zip(latest.iter()) {
                bus.volume = latest_bus.volume;
                bus.mute = latest_bus.mute;
                bus.solo = latest_bus.solo;
            }
        } else {
            app.buses = latest;
        }
        true
    } else {
        false
    }
}

fn editor_has_visible_buffer(editor: &Editor, name: &str) -> bool {
    editor.tile_root.leaf_ids().into_iter().any(|tile_id| {
        editor
            .tile_root
            .find_leaf(tile_id)
            .and_then(|leaf| editor.buffers.get(leaf.buffer_idx))
            .is_some_and(|buffer| buffer.name == name && buffer.view_mode != ViewMode::TextOnly)
    })
}

fn reconciled_track_index(
    stored_track: usize,
    cursor_track: usize,
    track_count: usize,
) -> Option<usize> {
    if track_count == 0 {
        return None;
    }
    if stored_track < track_count {
        Some(stored_track)
    } else if cursor_track < track_count {
        Some(cursor_track)
    } else {
        Some(track_count - 1)
    }
}

fn current_track_for_app(app: &mut ui::App, current_track: &Arc<AtomicUsize>) -> Option<usize> {
    let track = reconciled_track_index(
        current_track.load(Ordering::Relaxed),
        app.ui.cursor_track,
        app.tracks.len(),
    )?;
    if current_track.load(Ordering::Relaxed) != track {
        current_track.store(track, Ordering::Relaxed);
    }
    app.ui.cursor_track = track;
    Some(track)
}

fn reveal_sequencer_current_track(editor: &mut Editor, app: &ui::App, track: usize) {
    let Some(track_id) = app.graph.track_node_ids.get(track).map(|ids| ids.pan_id) else {
        return;
    };
    let key = format!("sequencer-track-{track_id}");
    if !editor.visible_buffer_layout_contains_stable_key("*sequencer*", &key) {
        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
    }
    editor.ensure_widget_stable_key_visible_in_buffer_named("*sequencer*", &key, 1.0);
}

fn key_should_reveal_sequencer_track(key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    matches!(
        (key.code, key.modifiers),
        (KeyCode::Tab, KeyModifiers::NONE)
            | (KeyCode::Up, KeyModifiers::NONE)
            | (KeyCode::Down, KeyModifiers::NONE)
    )
}

fn track_button_state_snapshot(state: &Arc<SequencerState>) -> Vec<(bool, bool)> {
    (0..state.active_track_count())
        .map(|track| {
            let params = &state.pattern.track_params[track];
            (params.is_muted(), params.is_solo())
        })
        .collect()
}

fn param_unit(param: &sequencer::effects::ParamDescriptor) -> Option<String> {
    match &param.kind {
        ParamKind::Continuous { unit } => unit.clone(),
        _ => None,
    }
}

fn param_enum_labels(param: &sequencer::effects::ParamDescriptor) -> Vec<String> {
    match &param.kind {
        ParamKind::Enum { labels } => labels.clone(),
        _ => Vec::new(),
    }
}

fn param_scaling(param: &sequencer::effects::ParamDescriptor) -> String {
    match param.scaling {
        ParamScaling::Linear => "linear".to_string(),
        ParamScaling::Exponential => "exponential".to_string(),
    }
}

fn metal_agent_session_context(
    app: &ui::App,
    current_track: &Arc<AtomicUsize>,
    active_delete_target: &Arc<Mutex<Option<ActiveDeleteTarget>>>,
) -> AgentSessionContext {
    let track = current_track_for_snapshot(app, current_track);
    let current_track_name = track.and_then(|track| app.tracks.get(track).cloned());
    let current_instrument_name = track.and_then(|track| {
        if app.graph.track_instrument_types.get(track)
            != Some(&sequencer::sequencer::InstrumentType::Custom)
        {
            return None;
        }
        current_custom_instrument_name(app, track)
    });
    let current_instrument_source = current_instrument_name
        .as_deref()
        .and_then(|name| sequencer::lisp_effect::load_instrument_source(name).ok());
    let current_instrument_preset_schema =
        metal_agent_instrument_preset_schema(app, track, current_instrument_name.as_deref());

    let current_effect_slot = active_delete_target
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|target| match target {
            ActiveDeleteTarget::FxEffect {
                chain: FxDeleteChain::Audio,
                slot,
                ..
            } if *slot >= sequencer::effects::BUILTIN_SLOT_COUNT => Some(*slot),
            _ => None,
        });
    let current_effect_name = track.and_then(|track| {
        current_effect_slot.and_then(|slot| {
            app.graph
                .effect_descriptors
                .get(track)
                .and_then(|descs| descs.get(slot))
                .map(|desc| desc.name.clone())
        })
    });
    let current_effect_source = current_effect_name
        .as_deref()
        .and_then(|name| sequencer::lisp_effect::load_effect_source(name).ok());
    let current_effect_ui_source = current_effect_name
        .as_deref()
        .and_then(|name| sequencer::lisp_effect::load_effect_ui_source(name).ok());

    AgentSessionContext {
        has_tracks: !app.tracks.is_empty(),
        current_track_name,
        current_track_index: track,
        can_apply_effect_to_current_track: app.next_free_custom_slot().is_some(),
        current_effect_name,
        current_effect_source: current_effect_source.clone(),
        current_effect_ui_source,
        current_effect_slot,
        can_update_current_effect: current_effect_source.is_some(),
        current_instrument_name,
        current_instrument_source: current_instrument_source.clone(),
        can_update_current_instrument: current_instrument_source.is_some(),
        current_instrument_preset_schema,
    }
}

fn current_track_for_snapshot(app: &ui::App, current_track: &Arc<AtomicUsize>) -> Option<usize> {
    if app.tracks.is_empty() {
        return None;
    }
    let stored = current_track.load(Ordering::Relaxed);
    if stored < app.tracks.len() {
        Some(stored)
    } else {
        Some(app.tracks.len() - 1)
    }
}

fn metal_agent_instrument_preset_schema(
    app: &ui::App,
    track: Option<usize>,
    instrument_name: Option<&str>,
) -> Option<AgentInstrumentPresetSchema> {
    let track = track?;
    let instrument_name = instrument_name?;
    let desc = app.graph.instrument_descriptors.get(track)?;
    let slot = app.state.pattern.instrument_slots.get(track)?;
    let existing_presets = sequencer::lisp_effect::load_instrument_presets(instrument_name)
        .map(|presets| presets.into_iter().map(|preset| preset.name).collect())
        .unwrap_or_default();

    let mut params = Vec::new();
    for (idx, param) in desc.params.iter().enumerate() {
        let group = if param.node_param_idx >= sequencer::voice_modulator::MOD_PARAM_BASE {
            "source"
        } else if param.name.starts_with("mod ") {
            "mod"
        } else {
            "synth"
        };
        params.push(AgentInstrumentParamSchema {
            name: param.name.clone(),
            group: group.to_string(),
            min: param.min,
            max: param.max,
            default: param.default,
            current_value: Some(slot.defaults.get(idx)),
            unit: param_unit(param),
            enum_labels: param_enum_labels(param),
            scaling: param_scaling(param),
        });
    }

    Some(AgentInstrumentPresetSchema {
        instrument_name: instrument_name.to_string(),
        source_file: sequencer::lisp_effect::instrument_source_path(instrument_name)
            .ok()
            .map(|path| path.display().to_string()),
        base_note_offset: f32::from_bits(
            app.state.pattern.instrument_base_note_offsets[track].load(Ordering::Relaxed),
        ),
        existing_presets,
        params,
    })
}

#[derive(Default)]
struct StubAnimationRenderCache {
    frame: Option<eseqlisp::backend::TiledRenderFrame>,
    size: Option<(usize, usize)>,
}

impl StubAnimationRenderCache {
    fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self) {
        self.frame = None;
        self.size = None;
    }

    fn update_size(&mut self, size: (usize, usize)) {
        if self.size != Some(size) {
            self.reset();
        }
    }

    fn store(&mut self, size: (usize, usize), frame: eseqlisp::backend::TiledRenderFrame) {
        self.size = Some(size);
        self.frame = Some(frame);
    }

    fn is_active(&self, size: (usize, usize), stub_visible: bool) -> bool {
        stub_visible && self.size == Some(size) && self.frame.is_some()
    }

    fn frame(&self) -> Option<&eseqlisp::backend::TiledRenderFrame> {
        self.frame.as_ref()
    }
}

fn map_number(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<f64> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::Number(value) => Some(*value),
        _ => None,
    })
}

fn map_string(
    map: &std::collections::HashMap<String, Rc<RefCell<Value>>>,
    key: &str,
) -> Option<String> {
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

fn map_bool(map: &std::collections::HashMap<String, Rc<RefCell<Value>>>, key: &str) -> bool {
    map.get(key)
        .and_then(|cell| match &*cell.borrow() {
            Value::Bool(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(false)
}

fn param_change_needs_fx_rebuild(param: &sequencer::effects::ParamDescriptor) -> bool {
    matches!(param.kind, ParamKind::Boolean | ParamKind::Enum { .. })
}

struct AgentDraftApplyResult {
    track_index: usize,
    created_track: bool,
}

struct AgentFinalizeResult {
    track_index: usize,
    instrument_name: String,
}

struct AgentEffectApplyResult {
    track_index: usize,
    slot_index: usize,
}

struct AgentEffectFinalizeResult {
    track_index: Option<usize>,
    slot_index: Option<usize>,
    effect_name: String,
}

fn sync_after_agent_instrument_apply(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    track_index: usize,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
) {
    current_track.store(track_index, Ordering::Relaxed);
    app.ui.cursor_track = track_index;
    let track_name = app.tracks[track_index].clone();
    if track_names.len() < app.tracks.len() {
        track_names.push(track_name);
    } else if let Some(name) = track_names.get_mut(track_index) {
        *name = track_name;
    }
    {
        let mut pan_ids = track_pan_ids.lock().unwrap();
        if pan_ids.len() < app.graph.track_node_ids.len() {
            pan_ids.push(app.graph.track_node_ids[track_index].pan_id);
        }
        push_solo_mutes(lg_raw, state, &pan_ids);
    }
    if record_armed.lock().unwrap().len() < app.tracks.len() {
        record_armed.lock().unwrap().push(false);
    }

    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(track_names.len() as f64));
    rt.set_reactive("SEQ", "track-ids", build_track_ids(app));
    set_current_track_reactive(rt, app.tracks.len(), track_index);
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    sync_all_track_sequencer_state(rt, state, app, track_index, selected_steps);
    rt.set_reactive("SEQ", "steps", build_steps_value(state, track_index));
    sync_step_param_lists(rt, state, track_index);
    sync_track_mixer_state(rt, app, state);
    sync_bus_mixer_state(rt, app);
    sync_track_peak_fields(rt, cached_track_peak_levels);
    sync_bus_peak_fields(rt, cached_bus_peak_levels);
    rt.set_reactive(
        "SEQ",
        "effects",
        build_effects_value(
            state,
            track_index,
            &app.graph.effect_descriptors,
            selected_steps,
        ),
    );
    rt.set_reactive(
        "SEQ",
        "midi-effects",
        build_midi_effects_value(state, track_index, selected_steps),
    );
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, track_index, selected_steps),
    );
    *accumulator_names.lock().unwrap() = build_accumulator_names(app);
    sync_track_params(rt, app, state, track_index, selected_steps);
    sync_fx_param_binding_fields(rt, app, state, track_index, selected_steps);
    rt.set_reactive(
        "SEQ",
        "step-has-plocks",
        build_step_has_plocks(state, track_index, &app.graph.effect_descriptors),
    );
    sync_sidebar_browser(rt, app, track_index);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    refresh_visible_track_topology_layouts(editor);
    ui_epoch.fetch_add(1, Ordering::Relaxed);
}

fn refresh_visible_track_topology_layouts(editor: &mut Editor) {
    for buffer_name in [
        "*metal*",
        "*sequencer*",
        "*mixer*",
        "*track*",
        "*fx*",
        "*piano-roll*",
    ] {
        editor.refresh_visible_layouts_for_buffer_named(buffer_name);
    }
}

fn step_param_fields(param: StepParam) -> Option<(&'static str, &'static str, usize)> {
    match param {
        StepParam::Velocity => Some(("velocities", "track-velocities", 0)),
        StepParam::Duration => Some(("durations", "track-durations", 1)),
        StepParam::AuxA => Some(("auxas", "track-auxas", 2)),
        StepParam::Transpose => Some(("transposes", "track-transposes", 3)),
        StepParam::Pan => Some(("pans", "track-pans", 4)),
        StepParam::Sync => Some(("syncs", "track-syncs", 5)),
        StepParam::Delay => Some(("delays", "track-delays", 6)),
        _ => None,
    }
}

fn step_param_slider_value(param: StepParam, value: f32) -> f64 {
    if param == StepParam::Duration {
        param.normalize(value) as f64
    } else {
        value as f64
    }
}

fn sync_single_step_param_binding(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    step: usize,
    param: StepParam,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    let Some((current_field, _, mode)) = step_param_fields(param) else {
        return false;
    };
    let value = state.pattern.step_data[track].get(step, param);
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_param_slider_field(track, mode, step),
            Value::Number(step_param_slider_value(param, value)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_param_haptic_field(track, mode, step),
            Value::Number(value as f64),
        )
        .effects_dirty;
    if track == current_track_idx {
        dirty |= rt
            .set_reactive_list_index("SEQ", current_field, step, Value::Number(value as f64))
            .effects_dirty;
    }
    for viewport in expanded_step_projection.viewports_for_track(track) {
        if let Some(slot) = visible_slot_for_step(viewport, step) {
            dirty |= sync_expanded_step_param_slot(rt, state, viewport, mode, slot);
        }
    }
    dirty
}

fn sync_single_step_structural_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    track: usize,
    step: usize,
    current_track_idx: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    if track >= app.tracks.len() || step >= MAX_STEPS {
        return false;
    }
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let visible = step < num_steps;
    let selected = selected_steps.lock().unwrap();
    let mut dirty = false;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_active_field(track, step),
            Value::Bool(visible && state.pattern.patterns[track].is_active(step)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_duration_field(track, step),
            Value::Bool(visible && track_step_duration_covered(state, track, step)),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_plocked_field(track, step),
            Value::Bool(
                visible && track_step_has_plock(state, track, &app.graph.effect_descriptors, step),
            ),
        )
        .effects_dirty;
    dirty |= rt
        .set_reactive(
            "SEQ",
            &track_step_selected_field(track, step),
            Value::Bool(visible && track == current_track_idx && selected.contains(&step)),
        )
        .effects_dirty;
    for viewport in expanded_step_projection.viewports_for_track(track) {
        if let Some(slot) = visible_slot_for_step(viewport, step) {
            dirty |= sync_expanded_step_slot(
                rt,
                state,
                app,
                &selected,
                current_track_idx,
                viewport,
                slot,
            );
        }
    }
    dirty
}

fn sync_track_duration_span_binding_fields(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    track: usize,
    start_step: usize,
) -> bool {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let mut dirty = false;
    for step in start_step.min(MAX_STEPS)..MAX_STEPS {
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_duration_field(track, step),
                Value::Bool(step < num_steps && track_step_duration_covered(state, track, step)),
            )
            .effects_dirty;
    }
    dirty
}

fn sync_step_selection_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: Option<&ui::App>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    let selected = selected_steps.lock().unwrap();
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    let selection_value = build_selection_value_from_set(&selected);
    let mut dirty = false;
    for step in 0..MAX_STEPS {
        dirty |= rt
            .set_reactive(
                "SEQ",
                &track_step_selected_field(track, step),
                Value::Bool(step < num_steps && selected.contains(&step)),
            )
            .effects_dirty;
    }
    dirty |= rt
        .set_reactive("SEQ", "selected-steps", selection_value)
        .effects_dirty;
    if let Some(app) = app {
        for viewport in expanded_step_projection.viewports_for_track(track) {
            for slot in 0..PAGE_SIZE {
                dirty |= sync_expanded_step_slot(
                    rt,
                    state,
                    app,
                    &selected,
                    current_track_idx,
                    viewport,
                    slot,
                );
            }
        }
    }
    dirty
}

fn neural_neuron_selected_field(pattern_idx: usize, network_id: u64, neuron_idx: usize) -> String {
    format!("neural-neuron-selected-{pattern_idx}-{network_id}-{neuron_idx}")
}

// Mirrors step selection: row widgets bind to targeted fields so selection dirties only those rows.
fn sync_selected_neural_neuron_bindings(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    selection: &BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
) -> bool {
    let mut dirty = rt
        .set_reactive(
            "SEQ",
            "selected-neural-neurons",
            sequencer::lisp_effect::selected_neural_neurons_to_value(selection),
        )
        .effects_dirty;
    let pattern_idx = state.current_scene_index();
    for network in state.current_neural_networks() {
        let neuron_count = network.num_neurons.min(sequencer::neural::NUM_NEURONS);
        for neuron_idx in 0..neuron_count {
            dirty |= rt
                .set_reactive(
                    "SEQ",
                    &neural_neuron_selected_field(pattern_idx, network.id, neuron_idx),
                    Value::Bool(selection.contains(
                        &sequencer::lisp_effect::SelectedNeuralNeuron {
                            pattern_idx,
                            network_id: network.id,
                            neuron_idx,
                        },
                    )),
                )
                .effects_dirty;
        }
    }
    dirty
}

fn sync_track_plocks_for_neural_selection(
    rt: &mut Runtime,
    app: &ui::App,
    state: &Arc<SequencerState>,
    track: usize,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    selection: &BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
) -> bool {
    rt.set_reactive(
        "SEQ",
        "track-plocks",
        build_track_plocks_value_with_neural_selection(
            app,
            state,
            track,
            selected_steps,
            Some(selection),
        ),
    )
    .effects_dirty
}

fn record_selected_neural_instrument_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    selected_neural_neurons: &sequencer::lisp_effect::SharedSelectedNeuralNeurons,
    track: usize,
    param_idx: usize,
    value: f32,
) -> (BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>, bool) {
    let neural_selection = selected_neural_neurons.lock().unwrap().clone();
    let wrote_neural_plock = write_selected_neural_instrument_plock(
        editor,
        state,
        &neural_selection,
        track,
        param_idx,
        value,
    );
    (neural_selection, wrote_neural_plock)
}

fn write_selected_neural_instrument_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    neural_selection: &BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
    track: usize,
    param_idx: usize,
    value: f32,
) -> bool {
    sequencer::lisp_effect::set_selected_neural_instrument_plocks(
        state,
        neural_selection,
        track,
        param_idx,
        value,
    )
    .unwrap_or_else(|error| {
        editor.handle_host_event(HostEvent::Status(format!(
            "Error setting neuron instrument p-lock: {error}"
        )));
        !neural_selection.is_empty()
    })
}

fn record_selected_neural_effect_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    selected_neural_neurons: &sequencer::lisp_effect::SharedSelectedNeuralNeurons,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> (BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>, bool) {
    let neural_selection = selected_neural_neurons.lock().unwrap().clone();
    let wrote_neural_plock = write_selected_neural_effect_plock(
        editor,
        state,
        &neural_selection,
        track,
        slot_idx,
        param_idx,
        value,
    );
    (neural_selection, wrote_neural_plock)
}

fn write_selected_neural_effect_plock(
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    neural_selection: &BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    value: f32,
) -> bool {
    sequencer::lisp_effect::set_selected_neural_effect_plocks(
        state,
        neural_selection,
        track,
        slot_idx,
        param_idx,
        value,
    )
    .unwrap_or_else(|error| {
        editor.handle_host_event(HostEvent::Status(format!(
            "Error setting neuron effect p-lock: {error}"
        )));
        !neural_selection.is_empty()
    })
}

struct InstrumentParamDisplaySync<'a> {
    app: &'a ui::App,
    state: &'a Arc<SequencerState>,
    selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    selection: &'a BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
    track: usize,
    param_idx: usize,
    display_step: Option<usize>,
    sync_plock_list: bool,
    sync_sampler_times: bool,
}

fn sync_instrument_param_authoring_display(
    editor: &mut Editor,
    sync: InstrumentParamDisplaySync<'_>,
) {
    let mut ui_dirty = false;
    if sync.sync_plock_list {
        ui_dirty |= sync_track_plocks_for_neural_selection(
            editor.runtime_mut(),
            sync.app,
            sync.state,
            sync.track,
            sync.selected_steps,
            sync.selection,
        );
    }
    ui_dirty |= sync_instrument_param_value_field_with_neural_selection(
        editor.runtime_mut(),
        sync.app,
        sync.track,
        sync.param_idx,
        sync.display_step,
        Some(sync.selection),
    );
    if sync.sync_sampler_times && (sync.param_idx == 2 || sync.param_idx == 3) {
        ui_dirty |= sync_sampler_selection_time_fields(
            editor.runtime_mut(),
            sync.app,
            sync.track,
            sync.display_step,
        );
    }
    flush_reactive_display_edit(editor, ui_dirty);
}

struct EffectParamDisplaySync<'a> {
    state: &'a Arc<SequencerState>,
    effect_descriptors: &'a [Vec<sequencer::effects::EffectDescriptor>],
    app: &'a ui::App,
    selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    selection: &'a BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
    track: usize,
    slot_idx: usize,
    param_idx: usize,
    display_step: Option<usize>,
    sync_plock_list: bool,
}

fn sync_effect_param_authoring_display(editor: &mut Editor, sync: EffectParamDisplaySync<'_>) {
    let mut ui_dirty = false;
    if sync.sync_plock_list {
        ui_dirty |= sync_track_plocks_for_neural_selection(
            editor.runtime_mut(),
            sync.app,
            sync.state,
            sync.track,
            sync.selected_steps,
            sync.selection,
        );
    }
    ui_dirty |= sync_track_effect_param_value_field_with_neural_selection(
        editor.runtime_mut(),
        sync.state,
        sync.effect_descriptors,
        sync.track,
        sync.slot_idx,
        sync.param_idx,
        sync.display_step,
        Some(sync.selection),
    );
    flush_reactive_display_edit(editor, ui_dirty);
}

fn flush_reactive_display_edit(editor: &mut Editor, dirty: bool) {
    if dirty {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.mark_needs_redraw();
    }
}

fn sync_expanded_step_viewports_for_track(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
    track: usize,
) -> bool {
    let selected = selected_steps.lock().unwrap();
    let mut dirty = false;
    for viewport in expanded_step_projection.viewports_for_track(track) {
        dirty |=
            sync_expanded_step_viewport(rt, state, app, &selected, current_track_idx, viewport);
    }
    dirty
}

fn sync_all_expanded_step_viewports(
    rt: &mut Runtime,
    state: &Arc<SequencerState>,
    app: &ui::App,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    current_track_idx: usize,
    expanded_step_projection: &Arc<ExpandedStepProjectionRegistry>,
) -> bool {
    let selected = selected_steps.lock().unwrap();
    let mut dirty = false;
    for viewport in expanded_step_projection.all_viewports() {
        dirty |=
            sync_expanded_step_viewport(rt, state, app, &selected, current_track_idx, viewport);
    }
    dirty
}

fn sync_shared_track_collapsed(track_collapsed: &Arc<Mutex<Vec<bool>>>, app: &ui::App) {
    *track_collapsed.lock().unwrap() = app.track_collapsed.clone();
}

struct UiInvalidationApplyCtx<'a> {
    app: &'a mut ui::App,
    editor: &'a mut Editor,
    state: &'a Arc<SequencerState>,
    track_collapsed: &'a Arc<Mutex<Vec<bool>>>,
    bus_state: &'a Arc<Mutex<Vec<ui::BusChannelState>>>,
    current_track_idx: usize,
    selected_steps: &'a Arc<Mutex<HashSet<usize>>>,
    selected_neural_neurons: &'a BTreeSet<sequencer::lisp_effect::SelectedNeuralNeuron>,
    piano_roll_selection: &'a Arc<Mutex<HashSet<u64>>>,
    accumulator_names: &'a Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &'a [f64],
    cached_bus_peak_levels: &'a [f64],
    record_armed: &'a Arc<Mutex<Vec<bool>>>,
    active_delete_target: &'a Arc<Mutex<Option<ActiveDeleteTarget>>>,
    active_delete_target_version: &'a Arc<AtomicUsize>,
    expanded_step_projection: &'a Arc<ExpandedStepProjectionRegistry>,
    fx_visible: bool,
    sequencer_visible: bool,
    mixer_visible: bool,
}

fn apply_ui_invalidations(
    invalidations: Vec<UiInvalidation>,
    ctx: UiInvalidationApplyCtx<'_>,
) -> bool {
    if invalidations.is_empty() {
        return false;
    }

    let UiInvalidationApplyCtx {
        app,
        editor,
        state,
        track_collapsed,
        bus_state,
        current_track_idx,
        selected_steps,
        selected_neural_neurons,
        piano_roll_selection,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        record_armed,
        active_delete_target,
        active_delete_target_version,
        expanded_step_projection,
        fx_visible,
        sequencer_visible,
        mixer_visible,
    } = ctx;

    let mut needs_reactive_cycle = true;
    let mut bus_state_pulled = false;
    let active_track_count = state.active_track_count().min(app.tracks.len());
    let rt = editor.runtime_mut();

    for invalidation in invalidations {
        let track_domain = match &invalidation {
            UiInvalidation::CurrentTrack { current, .. } => Some(*current),
            UiInvalidation::TrackTopology(TrackTopologyInvalidation::InstrumentType { track }) => {
                Some(*track)
            }
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track })
            | UiInvalidation::Pattern(PatternInvalidation::TrackLength { track })
            | UiInvalidation::Pattern(PatternInvalidation::TrackTiming { track })
            | UiInvalidation::Step { track, .. }
            | UiInvalidation::StepSelection { track }
            | UiInvalidation::ExpandedStepViewport { track, .. }
            | UiInvalidation::TrackMixer { track, .. }
            | UiInvalidation::TrackBusSend { track, .. }
            | UiInvalidation::TrackRoute { track }
            | UiInvalidation::TrackParam { track, .. }
            | UiInvalidation::TrackParamPanel { track }
            | UiInvalidation::Instrument { track, .. }
            | UiInvalidation::TrackFx { track, .. }
            | UiInvalidation::MidiFx { track, .. }
            | UiInvalidation::PianoRoll { track, .. }
            | UiInvalidation::Sidebar { track, .. } => Some(*track),
            _ => None,
        };
        if track_domain.is_some_and(|track| track >= active_track_count) {
            continue;
        }
        if matches!(&invalidation, UiInvalidation::Step { step, .. } if *step >= MAX_STEPS) {
            continue;
        }
        let bus_domain = match &invalidation {
            UiInvalidation::BusMixer { bus, .. }
            | UiInvalidation::BusFx { bus, .. }
            | UiInvalidation::TrackBusSend { bus, .. } => Some(*bus),
            _ => None,
        };
        if bus_domain.is_some_and(|bus| bus >= app.buses.len()) {
            continue;
        }

        match invalidation {
            UiInvalidation::Full(_)
            | UiInvalidation::TrackTopology(_)
            | UiInvalidation::BusTopology
            | UiInvalidation::ProjectState
            | UiInvalidation::CurrentTrack { .. } => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Pattern(PatternInvalidation::WholeTrack { track }) => {
                if track == current_track_idx {
                    needs_reactive_cycle |= rt
                        .set_reactive("SEQ", "steps", build_steps_value(state, track))
                        .effects_dirty;
                    sync_step_param_lists(rt, state, track);
                }
                if sequencer_visible {
                    sync_all_track_sequencer_state(
                        rt,
                        state,
                        app,
                        current_track_idx,
                        selected_steps,
                    );
                    let _ = sync_expanded_step_viewports_for_track(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                        track,
                    );
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::Pattern(PatternInvalidation::TrackLength { track }) => {
                if track == current_track_idx {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "tp-num-steps",
                            Value::Number(state.pattern.track_params[track].get_num_steps() as f64),
                        )
                        .effects_dirty;
                }
                needs_reactive_cycle |= rt
                    .set_reactive_list_index(
                        "SEQ",
                        "track-num-steps",
                        track,
                        Value::Number(state.pattern.track_params[track].get_num_steps() as f64),
                    )
                    .effects_dirty;
                if sequencer_visible {
                    needs_reactive_cycle |= sync_expanded_step_viewports_for_track(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                        track,
                    );
                }
            }
            UiInvalidation::Pattern(PatternInvalidation::AllTracks)
            | UiInvalidation::Pattern(PatternInvalidation::TrackTiming { .. }) => {
                sync_pattern_state(rt, state);
                if sequencer_visible {
                    let _ = sync_all_expanded_step_viewports(
                        rt,
                        state,
                        app,
                        selected_steps,
                        current_track_idx,
                        expanded_step_projection,
                    );
                }
                needs_reactive_cycle = true;
            }
            UiInvalidation::Step {
                track,
                step,
                change,
            } => match change {
                StepInvalidation::Param(param) => {
                    needs_reactive_cycle |= sync_single_step_param_binding(
                        rt,
                        state,
                        track,
                        step,
                        param.to_step_param(),
                        current_track_idx,
                        expanded_step_projection,
                    );
                }
                StepInvalidation::DurationSpan => {
                    needs_reactive_cycle |=
                        sync_track_duration_span_binding_fields(rt, state, track, step);
                }
                StepInvalidation::Active
                | StepInvalidation::Payload
                | StepInvalidation::PlockPresence
                | StepInvalidation::Selected => {
                    needs_reactive_cycle |= sync_single_step_structural_bindings(
                        rt,
                        state,
                        app,
                        track,
                        step,
                        current_track_idx,
                        selected_steps,
                        expanded_step_projection,
                    );
                }
            },
            UiInvalidation::StepSelection { track } => {
                needs_reactive_cycle |= sync_step_selection_bindings(
                    rt,
                    state,
                    Some(&*app),
                    track,
                    selected_steps,
                    current_track_idx,
                    expanded_step_projection,
                );
                if track == current_track_idx {
                    if fx_visible {
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                state,
                                track,
                                &app.graph.effect_descriptors,
                                selected_steps,
                            ),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(state, track, selected_steps),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(app, track, selected_steps),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "bus-effects",
                            build_bus_effects_value_for_selection(app, Some(selected_steps)),
                        );
                        sync_track_params_with_neural_selection(
                            rt,
                            app,
                            state,
                            track,
                            selected_steps,
                            Some(selected_neural_neurons),
                        );
                        needs_reactive_cycle = true;
                    }
                    needs_reactive_cycle |=
                        sync_fx_param_binding_fields(rt, app, state, track, selected_steps);
                }
            }
            UiInvalidation::ExpandedStepViewport { track: _, track_id } => {
                if let Some(viewport) = expanded_step_projection.viewport(track_id) {
                    let selected = selected_steps.lock().unwrap();
                    needs_reactive_cycle |= sync_expanded_step_viewport(
                        rt,
                        state,
                        app,
                        &selected,
                        current_track_idx,
                        viewport,
                    );
                }
            }
            UiInvalidation::TrackMixer { track, change } => match change {
                TrackMixerInvalidation::Volume => {
                    sync_track_volume_binding_field(rt, state, track);
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-volumes",
                            track,
                            Value::Number(state.pattern.track_params[track].get_volume() as f64),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Pan => {
                    sync_track_pan_binding_field(rt, state, track);
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-mixer-pans",
                            track,
                            Value::Number(state.pattern.track_params[track].get_pan() as f64),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Mute => {
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-mutes",
                            track,
                            Value::Bool(state.pattern.track_params[track].is_muted()),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Solo => {
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-solos",
                            track,
                            Value::Bool(state.pattern.track_params[track].is_solo()),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::MutedBySolo => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "track-muted-by-solo",
                            build_track_muted_by_solo(state),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::RecordArm => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        )
                        .effects_dirty;
                }
                TrackMixerInvalidation::Output => {
                    needs_reactive_cycle |= rt
                        .set_reactive("SEQ", "track-outputs", build_track_outputs(app, state))
                        .effects_dirty;
                }
                TrackMixerInvalidation::Collapsed => {
                    let collapsed = track_collapsed.lock().unwrap().clone();
                    app.replace_track_collapsed(collapsed);
                    needs_reactive_cycle |= rt
                        .set_reactive("SEQ", "track-collapsed", build_track_collapsed(app))
                        .effects_dirty;
                }
            },
            UiInvalidation::BusMixer { bus, change } => {
                if !bus_state_pulled {
                    pull_shared_bus_state(app, bus_state);
                    bus_state_pulled = true;
                }
                if app.buses.get(bus).is_some() {
                    match change {
                        BusMixerInvalidation::Volume => {
                            sync_bus_mixer_control_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                        BusMixerInvalidation::Mute => {
                            sync_bus_mixer_control_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                        BusMixerInvalidation::Solo => {
                            sync_bus_mixer_control_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                        BusMixerInvalidation::Steps | BusMixerInvalidation::Timing => {
                            sync_bus_mixer_state(rt, app);
                            needs_reactive_cycle = true;
                        }
                    }
                }
            }
            UiInvalidation::TrackBusSend { track, bus } => {
                sync_track_bus_send_binding_field(rt, app, state, track, bus);
                if track == current_track_idx {
                    sync_current_track_bus_send_binding_field(rt, app, state, track, bus);
                }
                needs_reactive_cycle |= rt
                    .set_reactive(
                        "SEQ",
                        "track-bus-sends",
                        build_all_track_bus_sends(app, state),
                    )
                    .effects_dirty;
            }
            UiInvalidation::TrackRoute { .. } => {
                sync_track_mixer_state(rt, app, state);
                needs_reactive_cycle = true;
            }
            UiInvalidation::ModRoutes => {
                needs_reactive_cycle |= rt
                    .set_reactive("SEQ", "mod-routes", build_mod_routes(state))
                    .effects_dirty;
            }
            UiInvalidation::TrackParam { track, change } => {
                if change == TrackParamInvalidation::NumSteps {
                    needs_reactive_cycle |= rt
                        .set_reactive_list_index(
                            "SEQ",
                            "track-num-steps",
                            track,
                            Value::Number(state.pattern.track_params[track].get_num_steps() as f64),
                        )
                        .effects_dirty;
                }
                if track == current_track_idx {
                    sync_track_params_with_neural_selection(
                        rt,
                        app,
                        state,
                        track,
                        selected_steps,
                        Some(selected_neural_neurons),
                    );
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::TrackParamPanel { track } => {
                if track == current_track_idx {
                    sync_track_params_with_neural_selection(
                        rt,
                        app,
                        state,
                        track,
                        selected_steps,
                        Some(selected_neural_neurons),
                    );
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::Instrument { track, change } => {
                let display_step =
                    displayed_plock_step(state, track, selected_plock_step(selected_steps));
                match change {
                    InstrumentInvalidation::Param { param } => {
                        needs_reactive_cycle |=
                            sync_instrument_param_value_field(rt, app, track, param, display_step);
                    }
                    InstrumentInvalidation::BaseNote => {
                        needs_reactive_cycle |=
                            sync_instrument_base_note_value_field(rt, app, track);
                    }
                    InstrumentInvalidation::SamplerSelectionTime => {
                        needs_reactive_cycle |=
                            sync_sampler_selection_time_fields(rt, app, track, display_step);
                    }
                    InstrumentInvalidation::PanelTopology | InstrumentInvalidation::Analysis => {
                        if fx_visible && track == current_track_idx {
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(app, track, selected_steps),
                            );
                            needs_reactive_cycle = true;
                        }
                    }
                    InstrumentInvalidation::Playhead => {
                        if app.is_sampler_track(track) {
                            let ph = read_sampler_playhead_seconds(app, track);
                            if ph > 0.0 {
                                needs_reactive_cycle |= rt
                                    .set_reactive("SEQ", "sampler-playhead", Value::Number(ph))
                                    .effects_dirty;
                            }
                        }
                    }
                }
            }
            UiInvalidation::TrackFx { track, change } => match change {
                TrackFxInvalidation::Param { slot, param }
                | TrackFxInvalidation::Plock { slot, param } => {
                    let display_step =
                        displayed_plock_step(state, track, selected_plock_step(selected_steps));
                    needs_reactive_cycle |= sync_track_effect_param_value_field(
                        rt,
                        state,
                        &app.graph.effect_descriptors,
                        track,
                        slot,
                        param,
                        display_step,
                    );
                    if track == current_track_idx {
                        needs_reactive_cycle |= rt
                            .set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(state, track, &app.graph.effect_descriptors),
                            )
                            .effects_dirty;
                    }
                }
                TrackFxInvalidation::Topology | TrackFxInvalidation::PanelTree => {
                    if fx_visible && track == current_track_idx {
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                state,
                                track,
                                &app.graph.effect_descriptors,
                                selected_steps,
                            ),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            },
            UiInvalidation::MidiFx { track, change } => match change {
                MidiFxInvalidation::Param { slot, param } => {
                    let display_step =
                        displayed_plock_step(state, track, selected_plock_step(selected_steps));
                    needs_reactive_cycle |=
                        sync_midi_fx_param_value_field(rt, state, track, slot, param, display_step);
                }
                MidiFxInvalidation::Topology => {
                    if fx_visible && track == current_track_idx {
                        rt.set_reactive(
                            "SEQ",
                            "midi-effects",
                            build_midi_effects_value(state, track, selected_steps),
                        );
                        needs_reactive_cycle = true;
                    }
                }
            },
            UiInvalidation::BusFx { bus, change } => match change {
                BusFxInvalidation::Param { slot, param } => {
                    sync_bus_effect_param_value_field(rt, app, bus, slot, param);
                }
                BusFxInvalidation::Topology => {
                    if mixer_visible || fx_visible {
                        sync_bus_mixer_state(rt, app);
                        needs_reactive_cycle = true;
                    }
                }
            },
            UiInvalidation::PianoRoll { track, .. } => {
                if track == current_track_idx {
                    sync_piano_roll_state(rt, state, track, piano_roll_selection);
                    needs_reactive_cycle = true;
                }
            }
            UiInvalidation::Transport(_) => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Recording(change) => match change {
                RecordingInvalidation::RecordingEnabled => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "recording",
                            Value::Bool(
                                app.ui.recording
                                    || record_armed.lock().unwrap().iter().any(|armed| *armed),
                            ),
                        )
                        .effects_dirty;
                }
                RecordingInvalidation::ArmedTracks => {
                    needs_reactive_cycle |= rt
                        .set_reactive(
                            "SEQ",
                            "record-armed",
                            build_record_armed_value(&record_armed.lock().unwrap()),
                        )
                        .effects_dirty;
                }
            },
            UiInvalidation::DeleteTarget => {
                needs_reactive_cycle |= rt
                    .set_reactive(
                        "SEQ",
                        "delete-target-version",
                        Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
                    )
                    .effects_dirty;
                sync_mixer_delete_target_binding_fields(
                    rt,
                    app.tracks.len(),
                    &state,
                    active_delete_target.lock().unwrap().as_ref(),
                );
            }
            UiInvalidation::AutoFollow => {
                needs_reactive_cycle = true;
            }
            UiInvalidation::Sidebar { track, .. } => {
                sync_sidebar_browser(rt, app, track);
                needs_reactive_cycle = true;
            }
            UiInvalidation::Browser(_) => {
                needs_reactive_cycle = true;
            }
        }
    }

    if needs_reactive_cycle {
        *accumulator_names.lock().unwrap() = build_accumulator_names(app);
        sync_track_peak_fields(rt, cached_track_peak_levels);
        sync_bus_peak_fields(rt, cached_bus_peak_levels);
    }
    needs_reactive_cycle
}

fn reset_sampler_waveform_view(editor: &mut Editor) {
    if let Err(error) = editor.runtime_mut().eval_str("(sampler-reset-view)") {
        eprintln!("waveform: failed to reset sampler viewport: {error:?}");
    }
}

fn load_sample_into_sampler_track(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    track: usize,
    path: &Path,
) -> Result<String, String> {
    if track >= app.tracks.len() {
        return Err(format!("Track {} does not exist", track + 1));
    }
    if !app.is_sampler_track(track) {
        return Err("Drop samples onto sampler tracks".to_string());
    }

    let loaded = sequencer::sampler::load_wav_buffer(lg_raw, path)?;
    app.submit_sample_analysis(&loaded);
    let new_buffer_id = loaded.buffer_id;
    let sample_rate = loaded.sample_rate;
    let new_name = sequencer::sample_db::display_title_for_sample_path(path).unwrap_or(loaded.name);
    register_waveform_sample(path);
    app.graph_controller()
        .send_sample_to_all_voices(track, new_buffer_id, sample_rate);
    app.graph.track_buffer_ids[track] = new_buffer_id;
    app.graph.track_sample_rates[track] = sample_rate;
    app.tracks[track] = new_name.clone();
    app.register_loaded_sample_path(&new_name, new_buffer_id, path.to_path_buf());
    if track < app.sampler_paths.len() {
        app.sampler_paths[track] = Some(path.to_path_buf());
    }
    app.reset_sampler_bpm_for_analysis(track);
    app.publish_sampler_analysis_runtime(track);
    reset_sampler_waveform_view(editor);
    if let Some(track_name) = track_names.get_mut(track) {
        *track_name = new_name.clone();
    }
    current_track.store(track, Ordering::Relaxed);
    app.ui.cursor_track = track;

    let rt = editor.runtime_mut();
    set_current_track_reactive(rt, app.tracks.len(), track);
    rt.set_reactive("SEQ", "track-names", build_track_names(track_names));
    rt.set_reactive(
        "SEQ",
        "instrument-panel",
        build_instrument_panel_value(app, track, selected_steps),
    );
    sync_sampler_selection_time_fields(
        rt,
        app,
        track,
        selected_steps.lock().unwrap().iter().copied().min(),
    );
    sync_track_mixer_state(rt, app, state);
    sync_sidebar_browser(rt, app, track);
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    Ok(new_name)
}

const AGENT_INSTRUMENT_STUB_DSP: &str = r#"; Provisional silent instrument used while Agent Mode is designing the real patch.
(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))

(param enabled @default 1 @min 0 @max 1)

(out 0 1 @name audio)
"#;

const AGENT_INSTRUMENT_STUB_UI: &str = r#"(defsynth-ui
  (box :width 70 :height :fill :padding 0 :debug-name "agent-instrument-stub-skeleton"
    (agent-instrument-stub-bg :width 70 :height :fill)))
"#;

fn ensure_agent_instrument_stub_track(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    conv_id: sequencer::agent::store::ConvId,
) -> Result<usize, String> {
    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    if let Some(target) = snapshot.state.accepted_instrument_target {
        return Ok(target.track_index);
    }
    if let Some(target) = snapshot.state.stub_instrument_target {
        sequencer::lisp_effect::save_instrument(&target.instrument_name, AGENT_INSTRUMENT_STUB_DSP)
            .map_err(|error| format!("Failed to refresh agent stub dsp.lisp: {error}"))?;
        sequencer::lisp_effect::save_instrument_ui(
            &target.instrument_name,
            AGENT_INSTRUMENT_STUB_UI,
        )
        .map_err(|error| format!("Failed to refresh agent stub ui.lisp: {error}"))?;
        if target.track_index < app.tracks.len()
            && app.graph.track_instrument_types.get(target.track_index)
                == Some(&sequencer::sequencer::InstrumentType::Custom)
        {
            app.replace_custom_instrument_track_sync(
                target.track_index,
                &target.instrument_name,
                AGENT_INSTRUMENT_STUB_DSP,
            )
            .map_err(|error| format!("Failed to refresh agent stub track: {error}"))?;
            reload_custom_instrument_ui(editor);
            sync_after_agent_instrument_apply(
                app,
                editor,
                state,
                target.track_index,
                current_track,
                track_names,
                track_pan_ids,
                record_armed,
                selected_steps,
                accumulator_names,
                cached_track_peak_levels,
                cached_bus_peak_levels,
                ui_epoch,
                lg_raw,
            );
            return Ok(target.track_index);
        }
    }

    let inst_name = format!("agent-draft-{conv_id}/");
    sequencer::lisp_effect::save_instrument(&inst_name, AGENT_INSTRUMENT_STUB_DSP)
        .map_err(|error| format!("Failed to save agent stub dsp.lisp: {error}"))?;
    sequencer::lisp_effect::save_instrument_ui(&inst_name, AGENT_INSTRUMENT_STUB_UI)
        .map_err(|error| format!("Failed to save agent stub ui.lisp: {error}"))?;

    let idx = app
        .add_saved_instrument_track_sync(&inst_name)
        .map_err(|error| format!("Failed to create agent stub track: {error}"))?;
    let _ = app.force_instrument_enabled(idx);
    reload_custom_instrument_ui(editor);

    app.agent_store
        .set_stub_instrument_target(
            conv_id,
            sequencer::agent::store::AcceptedInstrumentTarget {
                track_index: idx,
                instrument_name: inst_name,
            },
        )
        .map_err(|error| format!("Failed to record agent stub target: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!("Created working instrument track {}", idx + 1),
        )
        .map_err(|error| format!("Failed to record agent stub message: {error}"))?;

    sync_after_agent_instrument_apply(
        app,
        editor,
        state,
        idx,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );
    Ok(idx)
}

fn apply_agent_draft_to_owned_instrument(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    conv_id: sequencer::agent::store::ConvId,
) -> Result<AgentDraftApplyResult, String> {
    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let draft = snapshot
        .state
        .draft
        .ok_or_else(|| format!("Agent conversation {conv_id} has no compiled draft"))?;

    let target = snapshot
        .state
        .accepted_instrument_target
        .or(snapshot.state.stub_instrument_target);
    let inst_name = target
        .as_ref()
        .map(|target| target.instrument_name.clone())
        .unwrap_or_else(|| format!("agent-draft-{conv_id}/"));

    sequencer::lisp_effect::save_instrument(&inst_name, &draft.dsp_source)
        .map_err(|error| format!("Failed to save agent draft dsp.lisp: {error}"))?;
    sequencer::lisp_effect::save_instrument_ui(&inst_name, &draft.ui_source)
        .map_err(|error| format!("Failed to save agent draft ui.lisp: {error}"))?;

    let (idx, created_track) = if let Some(target) = target {
        if target.track_index < app.tracks.len()
            && app.graph.track_instrument_types.get(target.track_index)
                == Some(&sequencer::sequencer::InstrumentType::Custom)
        {
            app.replace_custom_instrument_track_sync(
                target.track_index,
                &inst_name,
                &draft.dsp_source,
            )
            .map_err(|error| format!("Failed to update agent instrument: {error}"))?;
            (target.track_index, false)
        } else {
            let idx = app
                .add_saved_instrument_track_sync(&inst_name)
                .map_err(|error| format!("Failed to recreate agent instrument track: {error}"))?;
            (idx, true)
        }
    } else {
        let idx = app
            .add_saved_instrument_track_sync(&inst_name)
            .map_err(|error| format!("Failed to accept agent draft: {error}"))?;
        (idx, true)
    };
    if app.force_instrument_enabled(idx) {
        eprintln!(
            "[agent-ui] forced instrument enabled conv={conv_id} track={}",
            idx + 1
        );
    }
    reload_custom_instrument_ui(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");
    let track_name = app.tracks[idx].clone();

    if let Err(error) = app.agent_store.discard(conv_id) {
        eprintln!("[agent-ui] accepted conv={conv_id} but failed to discard draft: {error}");
    }
    if let Err(error) = app.agent_store.set_accepted_instrument_target(
        conv_id,
        sequencer::agent::store::AcceptedInstrumentTarget {
            track_index: idx,
            instrument_name: inst_name,
        },
    ) {
        eprintln!("[agent-ui] accepted conv={conv_id} but failed to record target: {error}");
    }
    if let Err(error) = app.agent_store.push_system_message(
        conv_id,
        if created_track {
            format!("Created instrument track {}: {}", idx + 1, track_name)
        } else {
            format!("Updated instrument track {}: {}", idx + 1, track_name)
        },
    ) {
        eprintln!("[agent-ui] accepted conv={conv_id} but failed to record success: {error}");
    }

    sync_after_agent_instrument_apply(
        app,
        editor,
        state,
        idx,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );

    Ok(AgentDraftApplyResult {
        track_index: idx,
        created_track,
    })
}

fn finalized_instrument_storage_paths(slug: &str) -> (PathBuf, PathBuf) {
    (
        Path::new("instruments").join(slug),
        Path::new("instruments").join(format!("{slug}.lisp")),
    )
}

fn patcher_layout_sidecar_path_for_dsp(dsp_path: &Path) -> PathBuf {
    if dsp_path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        dsp_path.with_file_name("dsp.layout.json")
    } else {
        let stem = dsp_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("dsp");
        dsp_path.with_file_name(format!("{stem}.layout.json"))
    }
}

fn copy_patcher_layout_sidecar(source_dsp: &Path, target_dsp: &Path) -> std::io::Result<()> {
    let source_layout = patcher_layout_sidecar_path_for_dsp(source_dsp);
    if !source_layout.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing layout sidecar '{}'", source_layout.display()),
        ));
    }
    let target_layout = patcher_layout_sidecar_path_for_dsp(target_dsp);
    if let Some(parent) = target_layout.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source_layout, target_layout).map(|_| ())
}

fn write_patcher_layout_sidecar(dsp_path: &Path, layout: &str) -> std::io::Result<()> {
    let layout_path = patcher_layout_sidecar_path_for_dsp(dsp_path);
    if let Some(parent) = layout_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = layout_path.with_file_name(format!(
        ".{}.tmp",
        layout_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("dsp.layout.json")
    ));
    std::fs::write(&tmp_path, layout)?;
    std::fs::rename(&tmp_path, &layout_path).or_else(|error| {
        let _ = std::fs::remove_file(&tmp_path);
        Err(error)
    })
}

fn apply_compiled_effect_edit_session(
    app: &mut ui::App,
    session: &EffectEditSession,
    name: &str,
    result: sequencer::lisp_effect::CompileResult,
) -> Result<(), String> {
    match session.target {
        EffectEditTarget::Track { track, slot } => {
            app.apply_compiled_effect_to_slot_sync(result, name, slot, track)
        }
        EffectEditTarget::Bus { bus, slot } => {
            app.apply_compiled_bus_effect_to_slot_sync(bus, slot, name, result)
        }
    }
}

fn finalized_effect_storage_paths(slug: &str) -> (PathBuf, PathBuf) {
    (
        Path::new("effects").join(slug),
        Path::new("effects").join(format!("{slug}.lisp")),
    )
}

fn display_instrument_name(name: &str) -> String {
    let trimmed = name.trim_end_matches('/');
    Path::new(trimmed)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(trimmed)
        .trim_end_matches(".lisp")
        .to_string()
}

fn cleanup_agent_draft_storage(name: &str) {
    let slug = name.trim_end_matches('/');
    if !slug.starts_with("agent-draft-") {
        return;
    }
    let dir = Path::new("instruments").join(slug);
    if dir.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove draft directory {}: {error}",
                dir.display()
            );
        }
    }
    let legacy_file = Path::new("instruments").join(format!("{slug}.lisp"));
    if legacy_file.exists() {
        if let Err(error) = std::fs::remove_file(&legacy_file) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove legacy draft file {}: {error}",
                legacy_file.display()
            );
        }
    }
}

fn cleanup_agent_effect_draft_storage(name: &str) {
    let slug = name.trim_end_matches('/');
    if !slug.starts_with("agent-effect-draft-") {
        return;
    }
    let dir = Path::new("effects").join(slug);
    if dir.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove draft effect directory {}: {error}",
                dir.display()
            );
        }
    }
    let legacy_file = Path::new("effects").join(format!("{slug}.lisp"));
    if legacy_file.exists() {
        if let Err(error) = std::fs::remove_file(&legacy_file) {
            eprintln!(
                "[agent-ui] finalized {name:?} but failed to remove legacy draft effect file {}: {error}",
                legacy_file.display()
            );
        }
    }
}

fn save_effect_with_ui_rollback(
    name: &str,
    dsp_source: &str,
    ui_source: &str,
) -> Result<(), String> {
    let previous_source = sequencer::lisp_effect::load_effect_source(name).ok();
    let previous_ui = sequencer::lisp_effect::load_effect_ui_source(name).ok();
    sequencer::lisp_effect::save_effect(name, dsp_source)
        .map_err(|error| format!("Failed to save effect dsp.lisp: {error}"))?;
    if let Err(error) = sequencer::lisp_effect::save_effect_ui(name, ui_source) {
        restore_effect_files(name, previous_source.as_deref(), previous_ui.as_deref());
        return Err(format!("Failed to save effect ui.lisp: {error}"));
    }
    Ok(())
}

fn restore_effect_files(name: &str, source: Option<&str>, ui_source: Option<&str>) {
    match source {
        Some(source) => {
            let _ = sequencer::lisp_effect::save_effect(name, source);
        }
        None => {
            let _ = std::fs::remove_file(sequencer::lisp_effect::effect_source_path(name));
        }
    }
    match ui_source {
        Some(ui_source) => {
            let _ = sequencer::lisp_effect::save_effect_ui(name, ui_source);
        }
        None => {
            let _ = std::fs::remove_file(sequencer::lisp_effect::effect_ui_path(name));
        }
    }
}

fn apply_agent_draft_to_effect_slot(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    conv_id: sequencer::agent::store::ConvId,
) -> Result<AgentEffectApplyResult, String> {
    if app.tracks.is_empty() {
        return Err("No current track is available for the effect artifact.".to_string());
    }
    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let draft = snapshot
        .state
        .effect_draft
        .ok_or_else(|| format!("Agent conversation {conv_id} has no validated effect draft"))?;

    let existing_target = snapshot.state.accepted_effect_target;
    let track_index = existing_target
        .as_ref()
        .map(|target| target.track_index)
        .unwrap_or(app.ui.cursor_track);
    if track_index >= app.tracks.len() {
        return Err("The target track for this effect artifact no longer exists.".to_string());
    }
    let slot_index = match existing_target.as_ref() {
        Some(target) => target.slot_index,
        None => app
            .next_free_custom_slot()
            .ok_or_else(|| "The current track has no free custom effect slot.".to_string())?,
    };
    let effect_name = existing_target
        .as_ref()
        .map(|target| target.effect_name.clone())
        .unwrap_or_else(|| format!("agent-effect-draft-{conv_id}/"));

    let previous_source = sequencer::lisp_effect::load_effect_source(&effect_name).ok();
    let previous_ui = sequencer::lisp_effect::load_effect_ui_source(&effect_name).ok();
    save_effect_with_ui_rollback(&effect_name, &draft.dsp_source, &draft.ui_source)?;
    if let Err(error) = app.load_saved_effect_to_slot_sync(track_index, slot_index, &effect_name) {
        restore_effect_files(
            &effect_name,
            previous_source.as_deref(),
            previous_ui.as_deref(),
        );
        return Err(format!("Failed to apply agent effect artifact: {error}"));
    }
    reload_custom_instrument_ui(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");

    app.agent_store
        .set_accepted_effect_target(
            conv_id,
            sequencer::agent::store::AcceptedEffectTarget {
                track_index,
                slot_index,
                effect_name: effect_name.clone(),
            },
        )
        .map_err(|error| format!("Failed to record effect target: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!(
                "Applied effect artifact to track {} slot {}",
                track_index + 1,
                slot_index + 1
            ),
        )
        .map_err(|error| format!("Failed to record effect apply message: {error}"))?;

    sync_after_agent_instrument_apply(
        app,
        editor,
        state,
        track_index,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );

    Ok(AgentEffectApplyResult {
        track_index,
        slot_index,
    })
}

fn finalize_agent_instrument(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    conv_id: sequencer::agent::store::ConvId,
    requested_name: &str,
) -> Result<AgentFinalizeResult, String> {
    let final_slug = sequencer::agent::actions::normalize_patch_name(
        requested_name,
        &format!("agent-instrument-{conv_id}"),
    );
    let final_name = format!("{final_slug}/");
    let (final_dir, legacy_file) = finalized_instrument_storage_paths(&final_slug);
    if final_dir.exists() || legacy_file.exists() {
        return Err(format!("Instrument '{final_slug}' already exists."));
    }

    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let target = snapshot
        .state
        .accepted_instrument_target
        .ok_or_else(|| "No applied agent artifact is available to finalize.".to_string())?;
    if target.track_index >= app.tracks.len()
        || app.graph.track_instrument_types.get(target.track_index)
            != Some(&sequencer::sequencer::InstrumentType::Custom)
    {
        return Err(
            "The applied agent artifact is no longer attached to a custom instrument track."
                .to_string(),
        );
    }

    let (dsp_source, ui_source) = if let Some(draft) = snapshot.state.draft {
        (draft.dsp_source, draft.ui_source)
    } else {
        (
            sequencer::lisp_effect::load_instrument_source(&target.instrument_name)
                .map_err(|error| format!("Failed to read draft dsp.lisp: {error}"))?,
            sequencer::lisp_effect::load_instrument_ui_source(&target.instrument_name)
                .map_err(|error| format!("Failed to read draft ui.lisp: {error}"))?,
        )
    };

    sequencer::lisp_effect::save_instrument(&final_name, &dsp_source)
        .map_err(|error| format!("Failed to save finalized dsp.lisp: {error}"))?;
    if let Err(error) = sequencer::lisp_effect::save_instrument_ui(&final_name, &ui_source) {
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(format!("Failed to save finalized ui.lisp: {error}"));
    }

    if let Err(error) =
        app.replace_custom_instrument_track_sync(target.track_index, &final_name, &dsp_source)
    {
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(format!("Failed to load finalized instrument: {error}"));
    }
    reload_custom_instrument_ui(editor);
    editor.refresh_visible_layouts_for_buffer_named("*fx*");

    app.agent_store
        .set_accepted_instrument_target(
            conv_id,
            sequencer::agent::store::AcceptedInstrumentTarget {
                track_index: target.track_index,
                instrument_name: final_name.clone(),
            },
        )
        .map_err(|error| format!("Failed to update artifact target: {error}"))?;
    app.agent_store
        .set_finalized_instrument_name(conv_id, final_name.clone())
        .map_err(|error| format!("Failed to mark artifact finalized: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!("Saved artifact as {}", display_instrument_name(&final_name)),
        )
        .map_err(|error| format!("Failed to record finalize message: {error}"))?;

    sync_after_agent_instrument_apply(
        app,
        editor,
        state,
        target.track_index,
        current_track,
        track_names,
        track_pan_ids,
        record_armed,
        selected_steps,
        accumulator_names,
        cached_track_peak_levels,
        cached_bus_peak_levels,
        ui_epoch,
        lg_raw,
    );
    cleanup_agent_draft_storage(&target.instrument_name);

    Ok(AgentFinalizeResult {
        track_index: target.track_index,
        instrument_name: final_name,
    })
}

fn finalize_agent_effect(
    app: &mut ui::App,
    editor: &mut Editor,
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    track_names: &mut Vec<String>,
    track_pan_ids: &Arc<Mutex<Vec<i32>>>,
    record_armed: &Arc<Mutex<Vec<bool>>>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    accumulator_names: &Arc<Mutex<Vec<String>>>,
    cached_track_peak_levels: &[f64],
    cached_bus_peak_levels: &[f64],
    ui_epoch: &Arc<AtomicUsize>,
    lg_raw: *mut sequencer::audiograph::LiveGraph,
    conv_id: sequencer::agent::store::ConvId,
    requested_name: &str,
) -> Result<AgentEffectFinalizeResult, String> {
    let final_slug = sequencer::agent::actions::normalize_patch_name(
        requested_name,
        &format!("agent-effect-{conv_id}"),
    );
    let final_name = format!("{final_slug}/");
    let (final_dir, legacy_file) = finalized_effect_storage_paths(&final_slug);
    if final_dir.exists() || legacy_file.exists() {
        return Err(format!("Effect '{final_slug}' already exists."));
    }

    let snapshot = app
        .agent_store
        .snapshot(conv_id)
        .ok_or_else(|| format!("Agent conversation {conv_id} not found"))?;
    let target = snapshot.state.accepted_effect_target;
    let (dsp_source, ui_source) = if let Some(target) = target.as_ref() {
        (
            sequencer::lisp_effect::load_effect_source(&target.effect_name)
                .map_err(|error| format!("Failed to read draft effect dsp.lisp: {error}"))?,
            sequencer::lisp_effect::load_effect_ui_source(&target.effect_name)
                .map_err(|error| format!("Failed to read draft effect ui.lisp: {error}"))?,
        )
    } else {
        let draft = snapshot
            .state
            .effect_draft
            .ok_or_else(|| "No effect artifact is available to finalize.".to_string())?;
        (draft.dsp_source, draft.ui_source)
    };

    save_effect_with_ui_rollback(&final_name, &dsp_source, &ui_source)?;

    if let Some(target) = target.as_ref() {
        if target.track_index >= app.tracks.len() {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err("The applied effect artifact target track no longer exists.".to_string());
        }
        if let Err(error) =
            app.load_saved_effect_to_slot_sync(target.track_index, target.slot_index, &final_name)
        {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err(format!("Failed to load finalized effect: {error}"));
        }
        reload_custom_instrument_ui(editor);
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
        app.agent_store
            .set_accepted_effect_target(
                conv_id,
                sequencer::agent::store::AcceptedEffectTarget {
                    track_index: target.track_index,
                    slot_index: target.slot_index,
                    effect_name: final_name.clone(),
                },
            )
            .map_err(|error| format!("Failed to update effect artifact target: {error}"))?;
        sync_after_agent_instrument_apply(
            app,
            editor,
            state,
            target.track_index,
            current_track,
            track_names,
            track_pan_ids,
            record_armed,
            selected_steps,
            accumulator_names,
            cached_track_peak_levels,
            cached_bus_peak_levels,
            ui_epoch,
            lg_raw,
        );
        cleanup_agent_effect_draft_storage(&target.effect_name);
    }

    app.agent_store
        .set_finalized_effect_name(conv_id, final_name.clone())
        .map_err(|error| format!("Failed to mark effect finalized: {error}"))?;
    app.agent_store
        .push_system_message(
            conv_id,
            format!(
                "Saved effect artifact as {}",
                display_instrument_name(&final_name)
            ),
        )
        .map_err(|error| format!("Failed to record effect finalize message: {error}"))?;

    Ok(AgentEffectFinalizeResult {
        track_index: target.as_ref().map(|target| target.track_index),
        slot_index: target.as_ref().map(|target| target.slot_index),
        effect_name: final_name,
    })
}

fn agent_generation_watermark(app: &ui::App) -> u64 {
    app.agent_store
        .list()
        .into_iter()
        .filter_map(|id| app.agent_store.snapshot(id).map(|snapshot| snapshot.state))
        .fold(0u64, |acc, state| {
            acc.wrapping_add(state.id)
                .wrapping_add(state.generation.wrapping_mul(31))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        build_custom_instrument_ui_source_with_overlay, effect_patcher_buffer_source,
        escape_lisp_string, instrument_patcher_buffer_source, key_should_reveal_sequencer_track,
        patcher_layout_sidecar_path_for_dsp, reconciled_track_index,
        restore_instrument_patcher_layout_source, should_clear_active_delete_target_for_buffer,
        show_instrument_patcher_layout_source, show_instrument_patcher_source_layout_source,
        ActiveDeleteTarget, ExpandedStepProjectionRegistry, FxDeleteChain, Runtime, StepParam,
        Value, AGENT_INSTRUMENT_STUB_UI, NEW_INSTRUMENT_STARTER_DSP,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use eseqlisp::parser::{ASTParser, Parser};
    use std::path::Path;

    #[test]
    fn active_delete_target_buffer_switch_preserves_target_claimed_in_new_buffer() {
        let mixer_target = ActiveDeleteTarget::MixerTrack { track: 0 };
        assert!(
            !should_clear_active_delete_target_for_buffer(Some(&mixer_target), "*mixer*"),
            "clicking a mixer delete target in an inactive mixer tile should survive the tile activation"
        );
        assert!(
            should_clear_active_delete_target_for_buffer(Some(&mixer_target), "*fx*"),
            "leaving mixer for another buffer should clear a mixer delete target"
        );

        let fx_target = ActiveDeleteTarget::FxEffect {
            chain: FxDeleteChain::Audio,
            bus: None,
            slot: 2,
        };
        assert!(
            !should_clear_active_delete_target_for_buffer(Some(&fx_target), "*fx*"),
            "clicking an FX delete target in an inactive FX tile should survive the tile activation"
        );
        assert!(
            should_clear_active_delete_target_for_buffer(Some(&fx_target), "*mixer*"),
            "leaving FX for mixer should clear an FX delete target"
        );
    }

    #[test]
    fn reconciles_stale_current_track_against_track_count() {
        assert_eq!(reconciled_track_index(2, 0, 4), Some(2));
        assert_eq!(reconciled_track_index(7, 1, 4), Some(1));
        assert_eq!(reconciled_track_index(7, 9, 4), Some(3));
        assert_eq!(reconciled_track_index(0, 0, 0), None);
    }

    #[test]
    fn step_selection_sync_updates_selected_steps_without_deadlocking() {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(1, Vec::new()));
        state.pattern.track_params[0].set_num_steps(8);
        let selected_steps = std::sync::Arc::new(std::sync::Mutex::new(
            [2_usize, 3, 4]
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
        ));
        let mut runtime = Runtime::new();

        let expanded_step_projection = std::sync::Arc::new(ExpandedStepProjectionRegistry::new());
        super::sync_step_selection_bindings(
            &mut runtime,
            &state,
            None,
            0,
            &selected_steps,
            0,
            &expanded_step_projection,
        );

        assert_eq!(
            runtime
                .eval_str("(nth SEQ.selected-steps 1)")
                .expect("read unselected step"),
            Some(Value::Bool(false))
        );
        assert_eq!(
            runtime
                .eval_str("(nth SEQ.selected-steps 3)")
                .expect("read selected step"),
            Some(Value::Bool(true))
        );
    }

    #[test]
    fn duration_span_sync_updates_covered_steps_after_source_duration_change() {
        let state = std::sync::Arc::new(sequencer::sequencer::SequencerState::new(1, Vec::new()));
        state.pattern.track_params[0].set_num_steps(8);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Duration, 3.0);
        let mut runtime = Runtime::new();

        super::sync_track_duration_span_binding_fields(&mut runtime, &state, 0, 2);

        assert_eq!(
            runtime
                .eval_str(r#"(reactive-get "SEQ" "seq-track-step-duration-0-4")"#)
                .expect("read covered step"),
            Some(Value::Bool(true)),
            "duration source at step 2 with length 3 should mark step 4 covered"
        );
        assert_eq!(
            runtime
                .eval_str(r#"(reactive-get "SEQ" "seq-track-step-duration-0-5")"#)
                .expect("read uncovered step"),
            Some(Value::Bool(false)),
            "duration source at step 2 with length 3 should not cover step 5"
        );
    }

    #[test]
    fn sequencer_reveal_is_limited_to_navigation_keys() {
        assert!(key_should_reveal_sequencer_track(&KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE
        )));
        assert!(key_should_reveal_sequencer_track(&KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE
        )));
        assert!(key_should_reveal_sequencer_track(&KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE
        )));
        assert!(
            !key_should_reveal_sequencer_track(&KeyEvent::new(
                KeyCode::Char('v'),
                KeyModifiers::NONE
            )),
            "parameter shortcuts should not reveal and scroll the current row"
        );
        assert!(
            !key_should_reveal_sequencer_track(&KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)),
            "non-track-navigation tab shortcuts should not reveal the sequencer row"
        );
    }

    #[test]
    fn new_instrument_starter_declares_standard_inputs_and_adsr() {
        let source = NEW_INSTRUMENT_STARTER_DSP;
        for (idx, name) in [
            (1, "gate"),
            (2, "pitch"),
            (3, "velocity"),
            (4, "trigger"),
            (5, "mod1"),
            (6, "mod2"),
            (7, "mod3"),
            (8, "mod4"),
        ] {
            assert!(
                source.contains(&format!("(def {name} (in {idx} @name {name}")),
                "starter source should declare input {idx} as {name}"
            );
        }
        for (idx, name) in [(1, "mod1"), (2, "mod2"), (3, "mod3"), (4, "mod4")] {
            assert!(
                source.contains(&format!(
                    "(def {name} (in {} @name {name} @modulator {idx}))",
                    idx + 4
                )),
                "starter source should mark {name} as modulator {idx}"
            );
        }
        for (name, role) in [
            ("attack", "attack"),
            ("decay", "decay"),
            ("sustain", "sustain"),
            ("release", "release"),
        ] {
            assert!(
                source.contains(&format!(
                    "(param {name} @group amp @env amp-env @role {role}"
                )),
                "starter source should tag {name} as amp-env {role}"
            );
        }
        assert!(source.contains("(def env (adsr gate trigger attack decay sustain release))"));
        assert!(source.contains("(out (* phase env velocity (mod gain)) 1 @name audio)"));
    }

    #[test]
    fn new_instrument_starter_compiles() {
        sequencer::lisp_effect::compile_instrument(NEW_INSTRUMENT_STARTER_DSP, 44_100)
            .expect("starter instrument should compile");
    }

    #[test]
    fn new_effect_starter_compiles() {
        sequencer::lisp_effect::compile_lisp(sequencer::lisp_effect::EFFECT_TEMPLATE, 44_100)
            .expect("starter effect should compile");
    }

    #[test]
    fn instrument_patcher_buffer_uses_user_source_path_and_preview_command() {
        let source = instrument_patcher_buffer_source(
            "*instrument-patcher:digitone*",
            Path::new("instruments/digitone/dsp.lisp"),
        );

        assert!(source.contains("(effect-buffer \"*instrument-patcher:digitone*\""));
        assert!(source.contains(":intent :instrument"));
        assert!(source.contains(":path \"instruments/digitone/dsp.lisp\""));
        assert!(source.contains("(host-command \"preview-instrument-patch\" event)"));
        assert!(!source.contains("defmacro"));
    }

    #[test]
    fn effect_patcher_buffer_uses_effect_intent_and_preview_command() {
        let source = effect_patcher_buffer_source(
            "*effect-patcher:lexilush*",
            Path::new("effects/lexilush/dsp.lisp"),
        );

        assert!(source.contains("(effect-buffer \"*effect-patcher:lexilush*\""));
        assert!(source.contains(":intent :effect"));
        assert!(source.contains(":path \"effects/lexilush/dsp.lisp\""));
        assert!(source.contains("(host-command \"preview-effect-patch\" event)"));
        assert!(!source.contains("defmacro"));
    }

    #[test]
    fn instrument_patcher_buffer_escapes_lisp_path_strings() {
        assert_eq!(escape_lisp_string("a\\b\"c"), "a\\\\b\\\"c");
    }

    #[test]
    fn instrument_patcher_layout_preserves_lower_panel_buffer() {
        let source = show_instrument_patcher_layout_source("*instrument-patcher:digitone*");

        assert_eq!(
            source,
            "(seq-apply-instrument-patcher-layout \"*instrument-patcher:digitone*\")"
        );
    }

    #[test]
    fn instrument_patcher_source_layout_includes_patcher_and_source_buffers() {
        let source = show_instrument_patcher_source_layout_source(
            "*instrument-patcher:digitone*",
            "*patcher-emitted:instruments/digitone/dsp.lisp*",
        );

        assert_eq!(
            source,
            "(seq-apply-instrument-patcher-source-layout \"*instrument-patcher:digitone*\" \"*patcher-emitted:instruments/digitone/dsp.lisp*\")"
        );
    }

    #[test]
    fn patcher_layout_sidecar_uses_stem_for_legacy_single_file_effects() {
        assert_eq!(
            patcher_layout_sidecar_path_for_dsp(Path::new("effects/legacy-delay.lisp")),
            Path::new("effects/legacy-delay.layout.json")
        );
        assert_eq!(
            patcher_layout_sidecar_path_for_dsp(Path::new("effects/lexilush/dsp.lisp")),
            Path::new("effects/lexilush/dsp.layout.json")
        );
    }

    #[test]
    fn instrument_patcher_layout_restore_uses_remembered_step_panel() {
        let source = restore_instrument_patcher_layout_source();

        assert_eq!(source, "(seq-restore-instrument-patcher-layout)");
    }

    #[test]
    fn agent_instrument_stub_ui_parses() {
        let tokens = Parser::new(AGENT_INSTRUMENT_STUB_UI.to_string())
            .parse()
            .expect("stub UI should tokenize");
        ASTParser::new(tokens)
            .parse()
            .expect("stub UI should parse");
    }

    #[test]
    fn agent_instrument_stub_ui_registers_as_custom_synth_ui() {
        const LEGACY_AGENT_INSTRUMENT_STUB_UI: &str = r#"(defwidget agent-instrument-stub-bg-legacy
  :width 70 :height 8.2
  :shader
  (sdf/fill
    (sdf/rounded-rect width height 0.45)
    (material :color (rgba (+ 0.1 (* 0.1 (sin itime))) 0.2 0.4 1.0))))

(defsynth-ui
  (box :width 70 :height 8.2 :padding 0 :debug-name "agent-instrument-stub-skeleton"
    (agent-instrument-stub-bg-legacy)))
"#;
        let custom_ui_source = build_custom_instrument_ui_source_with_overlay(Some((
            "agent-draft-1/".to_string(),
            "instruments/agent-draft-1/ui.lisp".to_string(),
            LEGACY_AGENT_INSTRUMENT_STUB_UI.to_string(),
        )));
        let mut runtime = Runtime::new();
        runtime
            .eval_str(
                r#"(def synth-ui-current-inst false)
                (def synth-ui-current-name "")
                (def custom-ui-current-kind "instrument")
                (def custom-ui-selected-section 0)
                (def custom-ui-selected-section-for-current-scope () 0)
                (def agent-instrument-stub-bg ()
                    (box :width 70 :height 8.2
                      (label "stub" :font-size 10 :color :gray :bg :transparent)))"#,
            )
            .expect("install stub widget test double");
        runtime
            .eval_str(&custom_ui_source)
            .expect("stub custom UI should evaluate");
        let rendered = runtime
            .eval_str(
                r#"(custom-instrument-synth-ui
                     (dict :name "agent-draft-1/"
                           :synth (list (dict :name "base_note"
                                              :control "base-note"
                                              :value 0
                                              :min -48
                                              :max 48))))"#,
            )
            .expect("stub custom UI should render");
        assert!(
            !matches!(rendered, Some(Value::Bool(false)) | None),
            "stub instrument should dispatch to its custom skeleton UI"
        );
    }

    struct SequencerDirGuard {
        original: std::path::PathBuf,
    }

    impl SequencerDirGuard {
        fn enter() -> Self {
            let original = std::env::current_dir().expect("read current dir");
            sequencer::paths::enter_sequencer_dir().expect("enter sequencer crate dir");
            Self { original }
        }
    }

    impl Drop for SequencerDirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    struct TestEngineGuard {
        lg_raw: *mut sequencer::audiograph::LiveGraph,
    }

    impl Drop for TestEngineGuard {
        fn drop(&mut self) {
            unsafe {
                sequencer::audiograph::clear_os_workgroup();
                sequencer::audiograph::engine_stop_workers();
                sequencer::audiograph::destroy_live_graph(self.lg_raw);
            }
        }
    }

    struct HeadlessAudioPump {
        running: std::sync::Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl HeadlessAudioPump {
        fn start(lg_ptr: sequencer::audiograph::LiveGraphPtr, channels: usize) -> Self {
            let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let worker_running = std::sync::Arc::clone(&running);
            let handle = std::thread::Builder::new()
                .name("project-92-headless-audio-pump".to_string())
                .spawn(move || {
                    let frames = 512;
                    let mut output = vec![0.0f32; frames * channels.max(1)];
                    while worker_running.load(std::sync::atomic::Ordering::Relaxed) {
                        unsafe {
                            lg_ptr.process_next_block(output.as_mut_ptr(), frames as i32);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                })
                .expect("spawn headless audio pump");
            Self {
                running,
                handle: Some(handle),
            }
        }
    }

    impl Drop for HeadlessAudioPump {
        fn drop(&mut self) {
            self.running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn find_layout_node_by_stable_key<'a>(
        node: &'a eseqlisp::layout::LayoutNode,
        key: &str,
    ) -> Option<&'a eseqlisp::layout::LayoutNode> {
        if node.stable_key.as_deref() == Some(key) {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_layout_node_by_stable_key(child, key))
    }

    fn visible_layout_revisions(editor: &eseqlisp::Editor) -> Vec<(String, u64)> {
        let mut revisions = editor
            .tile_root
            .leaf_ids()
            .into_iter()
            .filter_map(|tile_id| {
                let leaf = editor.tile_root.find_leaf(tile_id)?;
                let buffer = editor.buffers.get(leaf.buffer_idx)?;
                Some((buffer.name.clone(), leaf.layout_revision))
            })
            .collect::<Vec<_>>();
        revisions.sort_by(|a, b| a.0.cmp(&b.0));
        revisions
    }

    fn changed_layout_buffers(before: &[(String, u64)], after: &[(String, u64)]) -> Vec<String> {
        let before = before
            .iter()
            .cloned()
            .collect::<std::collections::HashMap<_, _>>();
        after
            .iter()
            .filter_map(|(name, revision)| {
                (before.get(name).copied() != Some(*revision)).then(|| name.clone())
            })
            .collect()
    }

    #[test]
    #[ignore = "perf probe: initializes the real metal_seq app graph and loads crates/sequencer/projects/92.json"]
    fn project_92_mixer_track_badge_switch_reports_layout_work() {
        std::thread::Builder::new()
            .name("project-92-track-switch-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(project_92_mixer_track_badge_switch_reports_layout_work_impl)
            .expect("spawn project 92 track switch probe")
            .join()
            .expect("project 92 track switch probe should pass");
    }

    fn project_92_mixer_track_badge_switch_reports_layout_work_impl() {
        use super::*;
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
            MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }
        }

        let _dir = SequencerDirGuard::enter();
        assert!(
            Path::new("projects/92.json").exists(),
            "project 92 must be available at crates/sequencer/projects/92.json"
        );

        let eng = engine::init_headless_engine(44_100, 2).expect("initialize headless app graph");
        let lg_raw = eng.lg_ptr.0;
        let state = eng.state.clone();
        let lg_ptr = eng.lg_ptr;
        let sample_rate = eng.sample_rate;
        let _engine_guard = TestEngineGuard { lg_raw };
        let _audio_pump = HeadlessAudioPump::start(lg_ptr, eng.channels as usize);
        let master_recorder = eng.master_recorder.clone();
        let mut app = ui::App::new(
            state.clone(),
            lg_ptr,
            sample_rate,
            eng.buses,
            eng.master_recorder,
            eng.keyboard_tx,
        );

        let mut track_names = Vec::<String>::new();
        let track_pan_ids = Arc::new(Mutex::new(Vec::<i32>::new()));
        let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
        let bus_state = Arc::new(Mutex::new(app.buses.clone()));
        let bus_node_ids = Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let selected_neural_neurons: sequencer::lisp_effect::SharedSelectedNeuralNeurons =
            Arc::new(Mutex::new(BTreeSet::new()));
        let piano_roll_selection = Arc::new(Mutex::new(HashSet::<u64>::new()));
        let piano_roll_move_state = Arc::new(Mutex::new(None));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let ui_invalidations = Arc::new(UiInvalidationQueue::new());
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        let recording = Arc::new(AtomicBool::new(false));
        let master_recording = Arc::new(AtomicBool::new(false));
        let record_armed = Arc::new(Mutex::new(Vec::<bool>::new()));
        let active_delete_target = Arc::new(Mutex::new(None));
        let active_delete_target_version = Arc::new(AtomicUsize::new(0));
        let auto_follow_override_until = Arc::new(Mutex::new(None));

        let RuntimeInit {
            runtime,
            accumulator_names,
            midi_fx_names: _,
            sample_browser: _,
        } = init_runtime(
            &app,
            state.clone(),
            &track_names,
            track_pan_ids.clone(),
            track_collapsed.clone(),
            bus_state.clone(),
            bus_node_ids.clone(),
            current_track.clone(),
            selected_steps.clone(),
            piano_roll_selection.clone(),
            piano_roll_move_state,
            recording.clone(),
            master_recording.clone(),
            master_recorder.clone(),
            record_armed.clone(),
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

        let mut editor = Editor::new(
            runtime,
            eseqlisp::EditorConfig {
                vim_mode: true,
                ..eseqlisp::EditorConfig::default()
            },
        );
        reload_custom_instrument_ui(&mut editor);
        let _ = editor.open_or_create_file_buffer("metal-seq-grid.lisp");
        let grid_source = editor.active_buffer().text();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(std::path::PathBuf::from("metal-seq-grid.lisp")),
            &grid_source,
            overlays,
        );
        assert!(
            report.success,
            "failed to load grid UI: {}",
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);
        editor.refresh_runtime_side_effects();
        reload_custom_instrument_ui(&mut editor);
        editor.set_layout_viewport(180, 70);
        editor.update_tile_rects(180, 70);
        let _ = editor.drain_host_commands();

        app.queue_project_load_named("92")
            .expect("queue project 92 load");
        for _ in 0..512 {
            if !app.has_pending_project_load() {
                break;
            }
            app.advance_pending_project_load()
                .expect("advance project 92 load");
        }
        assert!(
            !app.has_pending_project_load(),
            "project 92 load did not finish"
        );
        assert!(
            app.tracks.len() >= 2,
            "project 92 should have multiple tracks"
        );

        current_track.store(0, Ordering::Relaxed);
        *track_pan_ids.lock().unwrap() = app
            .graph
            .track_node_ids
            .iter()
            .map(|ids| ids.pan_id)
            .collect();
        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
        *record_armed.lock().unwrap() = vec![false; app.tracks.len()];
        sync_shared_track_collapsed(&track_collapsed, &app);
        push_project_scratch_to_named_buffer(&mut editor, &app);
        if let Err(error) = evaluate_project_scratch_on_ui_runtime(&mut editor, &app) {
            editor.handle_host_event(HostEvent::Status(format!("Scratch UI eval error: {error}")));
        }

        let cached_track_peak_levels = vec![0.0; track_names.len()];
        let cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
        let (cached_modulator_phases, cached_modulator_levels) =
            read_modulator_display_values(app.graph.lg, &app);

        {
            let rt = editor.runtime_mut();
            sync_project_state(rt, &app);
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                0,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
            rt.set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "bus-effects",
                build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
            );
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_modulator_phase_fields(rt, &cached_modulator_phases);
            sync_modulator_level_fields(rt, &cached_modulator_levels);
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &state,
                active_delete_target.lock().unwrap().as_ref(),
            );
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
            );
            rt.run_reactive_cycle();
        }
        editor.refresh_runtime_side_effects();
        refresh_visible_track_topology_layouts(&mut editor);
        editor.update_tile_rects(180, 70);
        let _ = editor.drain_host_commands();

        let mixer_buffer_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.name == "*mixer*")
            .expect("mixer buffer");
        let mixer_tile = editor
            .tile_root
            .find_leaf_by_buffer_idx(mixer_buffer_idx)
            .expect("visible mixer tile");
        let mixer_tile_id = mixer_tile.id;
        editor.switch_active_tile(mixer_tile_id);
        let mixer_layout = editor.widget_layout().expect("mixer active layout");
        let target_track = 1usize;
        let target_badge = find_layout_node_by_stable_key(
            &mixer_layout,
            &format!("mixer-v2-track-label-{target_track}"),
        )
        .expect("target mixer track badge");
        let click_col = target_badge.rect.col + target_badge.rect.width * 0.5;
        let click_row = target_badge.rect.row + target_badge.rect.height * 0.5;
        let content_width = mixer_layout.rect.width.ceil().max(1.0) as u16;
        let content_height = mixer_layout.rect.height.ceil().max(1.0) as u16;
        let before_revisions = visible_layout_revisions(&editor);

        let measured = Instant::now();
        let phase = Instant::now();
        editor.handle_mouse_precise(
            mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                click_col.floor() as u16,
                click_row.floor() as u16,
            ),
            0,
            0,
            content_width,
            content_height,
            click_col,
            click_row,
        );
        let commands = editor.drain_host_commands();
        let click_dispatch = phase.elapsed();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                HostCommand::Custom { name, .. } if name == "reveal-sequencer-track"
            )),
            "mixer track badge click should queue reveal-sequencer-track, got {commands:?}"
        );
        assert_eq!(
            current_track.load(Ordering::Relaxed),
            target_track,
            "mixer track badge click should select the target track"
        );

        let phase = Instant::now();
        let ct =
            current_track_for_app(&mut app, &current_track).expect("current track after click");
        editor.reset_widget_scroll_for_buffer_named("*metal*");
        editor.reset_widget_scroll_for_buffer_named("*fx*");
        editor
            .runtime_mut()
            .eval_str("(set! selected-bus -1)")
            .expect("clear selected bus");
        reset_sampler_waveform_view(&mut editor);
        let pre_sync = phase.elapsed();

        let phase = Instant::now();
        {
            let rt = editor.runtime_mut();
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                ct,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
        }
        let topology_sync = phase.elapsed();

        let phase = Instant::now();
        {
            let rt = editor.runtime_mut();
            rt.set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "bus-effects",
                build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
            );
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &state,
                active_delete_target.lock().unwrap().as_ref(),
            );
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
            );
            rt.run_reactive_cycle();
        }
        let reactive_cycle = phase.elapsed();

        let phase = Instant::now();
        editor.refresh_runtime_side_effects();
        let runtime_side_effects = phase.elapsed();

        let phase = Instant::now();
        reveal_sequencer_current_track(&mut editor, &app, ct);
        let sequencer_reveal = phase.elapsed();

        let phase = Instant::now();
        editor.mark_needs_redraw();
        let redraw_mark = phase.elapsed();
        let elapsed = measured.elapsed();

        let after_revisions = visible_layout_revisions(&editor);
        let changed_buffers = changed_layout_buffers(&before_revisions, &after_revisions);
        let trace = editor
            .runtime()
            .last_ui_invalidation_trace()
            .expect("track switch should produce an invalidation trace");
        let mut relayout_timings = Vec::<(String, String, f64)>::new();
        if trace.relayout_duration > std::time::Duration::ZERO {
            relayout_timings.push((
                editor.active_buffer().name.clone(),
                format!(
                    "active-{}",
                    trace.relayout_mode.as_deref().unwrap_or("unknown")
                ),
                trace.relayout_duration.as_secs_f64() * 1000.0,
            ));
        }
        relayout_timings.extend(editor.last_layout_refresh_timings().iter().map(|timing| {
            (
                timing.buffer_name.clone(),
                format!(
                    "inactive-{}-tile-{}",
                    timing.mode,
                    timing
                        .tile_id
                        .map(|tile_id| tile_id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                timing.elapsed.as_secs_f64() * 1000.0,
            )
        }));
        relayout_timings.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let worst_relayout = relayout_timings.first().cloned();

        eprintln!(
            "[project-92-track-switch] track=1 elapsed_ms={:.3} click_dispatch_ms={:.3} pre_sync_ms={:.3} topology_sync_ms={:.3} reactive_cycle_ms={:.3} runtime_side_effects_ms={:.3} sequencer_reveal_ms={:.3} redraw_mark_ms={:.3} changed_layout_buffers={:?} sequencer_relayout={} relayout_timings={:?} worst_relayout={:?} dirty_fields={} affected_buffers={:?} widget_tree_flushes={} full_reruns={} subtree_reruns={} relayout_mode={:?} relayout_ms={:.3} relayout_failure={:?}",
            elapsed.as_secs_f64() * 1000.0,
            click_dispatch.as_secs_f64() * 1000.0,
            pre_sync.as_secs_f64() * 1000.0,
            topology_sync.as_secs_f64() * 1000.0,
            reactive_cycle.as_secs_f64() * 1000.0,
            runtime_side_effects.as_secs_f64() * 1000.0,
            sequencer_reveal.as_secs_f64() * 1000.0,
            redraw_mark.as_secs_f64() * 1000.0,
            changed_buffers,
            changed_buffers.iter().any(|name| name == "*sequencer*"),
            relayout_timings,
            worst_relayout,
            trace.dirty_fields.len(),
            trace.affected_buffers,
            trace.widget_tree_flushes,
            trace.full_buffer_reruns,
            trace.subtree_reruns,
            trace.relayout_mode,
            trace.relayout_duration.as_secs_f64() * 1000.0,
            trace.relayout_failure_reason,
        );

        assert!(
            changed_buffers.iter().any(|name| name == "*fx*"),
            "fx layout should change after selecting a different track"
        );
        assert!(
            !changed_buffers.iter().any(|name| name == "*sequencer*"),
            "sequencer should reveal the selected track from its cached layout without relayout"
        );
        assert!(
            !trace.affected_buffers.iter().any(|name| name == "*mixer*"),
            "mixer track badge selection should use widget bindings instead of rerunning the mixer widget tree"
        );
        assert_eq!(
            trace.subtree_reruns, 0,
            "track switch should not rerun mixer/sequencer subtree work for badge styling"
        );
        assert!(
            trace.widget_tree_flushes > 0,
            "track switch should report widget tree work"
        );
    }

    #[test]
    #[ignore = "perf probe: initializes the real metal_seq app graph and loads crates/sequencer/projects/92.json"]
    fn project_92_scene_switch_reports_layout_work() {
        std::thread::Builder::new()
            .name("project-92-scene-switch-probe".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(project_92_scene_switch_reports_layout_work_impl)
            .expect("spawn project 92 scene switch probe")
            .join()
            .expect("project 92 scene switch probe should pass");
    }

    fn project_92_scene_switch_reports_layout_work_impl() {
        use super::*;
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        fn duration_ms(duration: Duration) -> f64 {
            duration.as_secs_f64() * 1000.0
        }

        let _dir = SequencerDirGuard::enter();
        assert!(
            Path::new("projects/92.json").exists(),
            "project 92 must be available at crates/sequencer/projects/92.json"
        );

        let eng = engine::init_headless_engine(44_100, 2).expect("initialize headless app graph");
        let lg_raw = eng.lg_ptr.0;
        let state = eng.state.clone();
        let lg_ptr = eng.lg_ptr;
        let sample_rate = eng.sample_rate;
        let _engine_guard = TestEngineGuard { lg_raw };
        let _audio_pump = HeadlessAudioPump::start(lg_ptr, eng.channels as usize);
        let master_recorder = eng.master_recorder.clone();
        let mut app = ui::App::new(
            state.clone(),
            lg_ptr,
            sample_rate,
            eng.buses,
            eng.master_recorder,
            eng.keyboard_tx,
        );

        let mut track_names = Vec::<String>::new();
        let track_pan_ids = Arc::new(Mutex::new(Vec::<i32>::new()));
        let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
        let bus_state = Arc::new(Mutex::new(app.buses.clone()));
        let bus_node_ids = Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
        let current_track = Arc::new(AtomicUsize::new(0));
        let selected_steps = Arc::new(Mutex::new(HashSet::<usize>::new()));
        let selected_neural_neurons: sequencer::lisp_effect::SharedSelectedNeuralNeurons =
            Arc::new(Mutex::new(BTreeSet::new()));
        let piano_roll_selection = Arc::new(Mutex::new(HashSet::<u64>::new()));
        let piano_roll_move_state = Arc::new(Mutex::new(None));
        let ui_epoch = Arc::new(AtomicUsize::new(0));
        let fx_epoch = Arc::new(AtomicUsize::new(0));
        let ui_invalidations = Arc::new(UiInvalidationQueue::new());
        let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
        let recording = Arc::new(AtomicBool::new(false));
        let master_recording = Arc::new(AtomicBool::new(false));
        let record_armed = Arc::new(Mutex::new(Vec::<bool>::new()));
        let active_delete_target = Arc::new(Mutex::new(None));
        let active_delete_target_version = Arc::new(AtomicUsize::new(0));
        let auto_follow_override_until = Arc::new(Mutex::new(None));

        let RuntimeInit {
            runtime,
            accumulator_names,
            midi_fx_names: _,
            sample_browser: _,
        } = init_runtime(
            &app,
            state.clone(),
            &track_names,
            track_pan_ids.clone(),
            track_collapsed.clone(),
            bus_state.clone(),
            bus_node_ids.clone(),
            current_track.clone(),
            selected_steps.clone(),
            piano_roll_selection.clone(),
            piano_roll_move_state,
            recording.clone(),
            master_recording.clone(),
            master_recorder.clone(),
            record_armed.clone(),
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

        let mut editor = Editor::new(
            runtime,
            eseqlisp::EditorConfig {
                vim_mode: true,
                ..eseqlisp::EditorConfig::default()
            },
        );
        reload_custom_instrument_ui(&mut editor);
        let _ = editor.open_or_create_file_buffer("metal-seq-grid.lisp");
        let grid_source = editor.active_buffer().text();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(std::path::PathBuf::from("metal-seq-grid.lisp")),
            &grid_source,
            overlays,
        );
        assert!(
            report.success,
            "failed to load grid UI: {}",
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);
        editor.refresh_runtime_side_effects();
        reload_custom_instrument_ui(&mut editor);
        editor.set_layout_viewport(180, 70);
        editor.update_tile_rects(180, 70);
        let _ = editor.drain_host_commands();

        app.queue_project_load_named("92")
            .expect("queue project 92 load");
        for _ in 0..512 {
            if !app.has_pending_project_load() {
                break;
            }
            app.advance_pending_project_load()
                .expect("advance project 92 load");
        }
        assert!(
            !app.has_pending_project_load(),
            "project 92 load did not finish"
        );
        assert!(
            app.state.scene_count() >= 2,
            "project 92 should have multiple scenes"
        );

        current_track.store(0, Ordering::Relaxed);
        *track_pan_ids.lock().unwrap() = app
            .graph
            .track_node_ids
            .iter()
            .map(|ids| ids.pan_id)
            .collect();
        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
        *record_armed.lock().unwrap() = vec![false; app.tracks.len()];
        sync_shared_track_collapsed(&track_collapsed, &app);
        push_project_scratch_to_named_buffer(&mut editor, &app);
        if let Err(error) = evaluate_project_scratch_on_ui_runtime(&mut editor, &app) {
            editor.handle_host_event(HostEvent::Status(format!("Scratch UI eval error: {error}")));
        }

        let cached_track_peak_levels = vec![0.0; app.tracks.len()];
        let cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
        let (cached_modulator_phases, cached_modulator_levels) =
            read_modulator_display_values(app.graph.lg, &app);

        {
            let rt = editor.runtime_mut();
            sync_project_state(rt, &app);
            sync_track_topology_state(
                rt,
                &app,
                &state,
                &mut track_names,
                0,
                &selected_steps,
                &piano_roll_selection,
                &accumulator_names,
                &record_armed,
                &cached_track_peak_levels,
            );
            rt.set_reactive(
                "SEQ",
                "selected-steps",
                build_selection_value(&selected_steps),
            );
            rt.set_reactive(
                "SEQ",
                "bus-effects",
                build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
            );
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_modulator_phase_fields(rt, &cached_modulator_phases);
            sync_modulator_level_fields(rt, &cached_modulator_levels);
            sync_mixer_delete_target_binding_fields(
                rt,
                app.tracks.len(),
                &state,
                active_delete_target.lock().unwrap().as_ref(),
            );
            rt.set_reactive(
                "SEQ",
                "delete-target-version",
                Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
            );
            rt.run_reactive_cycle();
        }
        editor.refresh_runtime_side_effects();
        refresh_visible_track_topology_layouts(&mut editor);
        editor.update_tile_rects(180, 70);
        let _ = editor.drain_host_commands();

        let before_revisions = visible_layout_revisions(&editor);
        let start_pattern = app.state.current_scene_index();
        let target_pattern = (start_pattern + 1) % app.state.scene_count();
        let ct = current_track.load(Ordering::Relaxed);
        let fx_visible = editor_has_visible_buffer(&editor, "*fx*");

        let measured = Instant::now();
        let switch_bus_elapsed;
        let state_switch_elapsed;
        let state_switch_profile;
        let apply_samples_elapsed;
        let restored_defaults_elapsed;
        let sync_names_pattern_elapsed;
        let sync_current_steps_elapsed;
        let sync_sequencer_elapsed;
        let sync_sequencer_profile;
        let sync_step_params_elapsed;
        let sync_mixer_elapsed;
        let sync_fx_lists_elapsed;
        let mut sync_effects_elapsed = Duration::ZERO;
        let mut sync_midi_effects_elapsed = Duration::ZERO;
        let mut sync_instrument_panel_elapsed = Duration::ZERO;
        let mut sync_accumulators_elapsed = Duration::ZERO;
        let sync_track_params_elapsed;
        let sync_fx_bindings_elapsed;
        let sync_plocks_sidebar_elapsed;
        let reactive_elapsed;
        let side_effects_elapsed;
        let mut mixer_refresh_elapsed = Duration::ZERO;

        let started = Instant::now();
        app.switch_bus_pattern(target_pattern);
        switch_bus_elapsed = started.elapsed();

        let started = Instant::now();
        let switched = app.state.switch_pattern_profiled(
            target_pattern,
            app.tracks.len(),
            &app.graph.track_buffer_ids,
            &app.graph.track_sample_rates,
            &app.tracks,
            &app.graph.track_instrument_types,
        );
        state_switch_elapsed = started.elapsed();
        let switched = switched.expect("project 92 scene switch should change sample ids");
        state_switch_profile = switched.profile;
        let sample_ids = switched.sample_ids;

        let started = Instant::now();
        app.graph_controller().apply_sample_ids(&sample_ids);
        app.graph_controller().sync_current_pattern_mod_routes();
        apply_samples_elapsed = started.elapsed();

        let started = Instant::now();
        app.push_all_restored_defaults();
        restored_defaults_elapsed = started.elapsed();

        {
            let rt = editor.runtime_mut();
            let started = Instant::now();
            sync_shared_track_collapsed(&track_collapsed, &app);
            sync_track_name_state(rt, &mut track_names, &app);
            sync_pattern_state(rt, &state);
            sync_names_pattern_elapsed = started.elapsed();

            let started = Instant::now();
            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
            sync_current_steps_elapsed = started.elapsed();

            let started = Instant::now();
            sync_sequencer_profile =
                sync_all_track_sequencer_state_profiled(rt, &state, &app, ct, &selected_steps);
            sync_sequencer_elapsed = started.elapsed();

            let started = Instant::now();
            sync_step_param_lists(rt, &state, ct);
            sync_step_params_elapsed = started.elapsed();

            let started = Instant::now();
            sync_track_mixer_state(rt, &app, &state);
            sync_bus_mixer_state(rt, &app);
            sync_track_peak_fields(rt, &cached_track_peak_levels);
            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
            sync_mixer_elapsed = started.elapsed();

            let started = Instant::now();
            if fx_visible {
                let sub_started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
                );
                sync_effects_elapsed = sub_started.elapsed();

                let sub_started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, ct, &selected_steps),
                );
                sync_midi_effects_elapsed = sub_started.elapsed();

                let sub_started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                sync_instrument_panel_elapsed = sub_started.elapsed();

                let sub_started = Instant::now();
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_accumulators_elapsed = sub_started.elapsed();
            } else {
                fx_epoch.fetch_add(1, Ordering::Relaxed);
            }
            sync_fx_lists_elapsed = started.elapsed();

            let started = Instant::now();
            let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
            sync_track_params_with_neural_selection(
                rt,
                &app,
                &state,
                ct,
                &selected_steps,
                Some(&selected_neural_snapshot),
            );
            sync_track_params_elapsed = started.elapsed();

            let started = Instant::now();
            sync_fx_param_binding_fields_with_neural_selection(
                rt,
                &app,
                &state,
                ct,
                &selected_steps,
                Some(&selected_neural_snapshot),
            );
            sync_fx_bindings_elapsed = started.elapsed();

            let started = Instant::now();
            rt.set_reactive(
                "SEQ",
                "step-has-plocks",
                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
            );
            sync_sidebar_browser(rt, &app, ct);
            sync_plocks_sidebar_elapsed = started.elapsed();

            let started = Instant::now();
            rt.run_reactive_cycle();
            reactive_elapsed = started.elapsed();
        }

        let started = Instant::now();
        editor.refresh_runtime_side_effects();
        side_effects_elapsed = started.elapsed();

        if editor_has_visible_buffer(&editor, "*mixer*") {
            let started = Instant::now();
            editor.refresh_visible_layouts_for_buffer_named("*mixer*");
            mixer_refresh_elapsed = started.elapsed();
        }
        let elapsed = measured.elapsed();

        let after_revisions = visible_layout_revisions(&editor);
        let changed_buffers = changed_layout_buffers(&before_revisions, &after_revisions);
        let trace = editor
            .runtime()
            .last_ui_invalidation_trace()
            .expect("scene switch should produce an invalidation trace");
        let mut reactive_hot = trace.reactive_exec_timings.clone();
        reactive_hot.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let reactive_hot = reactive_hot
            .into_iter()
            .take(6)
            .map(|(label, elapsed)| (label, duration_ms(elapsed)))
            .collect::<Vec<_>>();
        let mut relayout_timings = Vec::<(String, String, f64)>::new();
        if trace.relayout_duration > Duration::ZERO {
            relayout_timings.push((
                editor.active_buffer().name.clone(),
                format!(
                    "active-{}",
                    trace.relayout_mode.as_deref().unwrap_or("unknown")
                ),
                duration_ms(trace.relayout_duration),
            ));
        }
        relayout_timings.extend(editor.last_layout_refresh_timings().iter().map(|timing| {
            (
                timing.buffer_name.clone(),
                format!(
                    "inactive-{}-tile-{}",
                    timing.mode,
                    timing
                        .tile_id
                        .map(|tile_id| tile_id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                duration_ms(timing.elapsed),
            )
        }));
        relayout_timings.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let worst_relayout = relayout_timings.first().cloned();

        eprintln!(
            "[project-92-scene-switch] from={} to={} elapsed_ms={:.3} switch_bus_ms={:.3} state_switch_ms={:.3} apply_samples_ms={:.3} defaults_ms={:.3} names_pattern_ms={:.3} current_steps_ms={:.3} sequencer_bindings_ms={:.3} step_params_ms={:.3} mixer_ms={:.3} fx_lists_ms={:.3} effects_ms={:.3} midi_effects_ms={:.3} instrument_panel_ms={:.3} accumulators_ms={:.3} track_params_ms={:.3} fx_bindings_ms={:.3} plocks_sidebar_ms={:.3} reactive_ms={:.3} side_effects_ms={:.3} mixer_refresh_ms={:.3} changed_layout_buffers={:?} relayout_timings={:?} worst_relayout={:?} dirty_fields={} affected_buffers={:?} widget_tree_flushes={} full_reruns={} subtree_reruns={} relayout_mode={:?} relayout_ms={:.3} relayout_failure={:?}",
            start_pattern,
            target_pattern,
            duration_ms(elapsed),
            duration_ms(switch_bus_elapsed),
            duration_ms(state_switch_elapsed),
            duration_ms(apply_samples_elapsed),
            duration_ms(restored_defaults_elapsed),
            duration_ms(sync_names_pattern_elapsed),
            duration_ms(sync_current_steps_elapsed),
            duration_ms(sync_sequencer_elapsed),
            duration_ms(sync_step_params_elapsed),
            duration_ms(sync_mixer_elapsed),
            duration_ms(sync_fx_lists_elapsed),
            duration_ms(sync_effects_elapsed),
            duration_ms(sync_midi_effects_elapsed),
            duration_ms(sync_instrument_panel_elapsed),
            duration_ms(sync_accumulators_elapsed),
            duration_ms(sync_track_params_elapsed),
            duration_ms(sync_fx_bindings_elapsed),
            duration_ms(sync_plocks_sidebar_elapsed),
            duration_ms(reactive_elapsed),
            duration_ms(side_effects_elapsed),
            duration_ms(mixer_refresh_elapsed),
            changed_buffers,
            relayout_timings,
            worst_relayout,
            trace.dirty_fields.len(),
            trace.affected_buffers,
            trace.widget_tree_flushes,
            trace.full_buffer_reruns,
            trace.subtree_reruns,
            trace.relayout_mode,
            duration_ms(trace.relayout_duration),
            trace.relayout_failure_reason,
        );

        eprintln!(
            "[project-92-scene-switch-detail] state_total_ms={:.3} state_capture_ms={:.3} state_lock_wait_ms={:.3} state_save_current_ms={:.3} state_launch_data_ms={:.3} state_restore_tracks_ms={:.3} state_collect_samples_ms={:.3} state_update_atoms_ms={:.3} state_mod_resync_ms={:.3} state_publish_snapshot_ms={:.3} seq_total_ms={:.3} seq_track_steps_ms={:.3} seq_track_num_steps_ms={:.3} seq_track_timebases_ms={:.3} seq_track_duration_spans_ms={:.3} seq_track_step_has_plocks_ms={:.3} seq_track_playheads_ms={:.3} seq_track_velocities_ms={:.3} seq_track_durations_ms={:.3} seq_track_auxas_ms={:.3} seq_track_transposes_ms={:.3} seq_track_pans_ms={:.3} seq_track_syncs_ms={:.3} seq_track_delays_ms={:.3} seq_step_bindings_ms={:.3} seq_playhead_fields_ms={:.3} step_active_ms={:.3} step_duration_ms={:.3} step_plocked_ms={:.3} step_selected_ms={:.3} step_slider_ms={:.3} step_haptic_ms={:.3} step_active_sets={:?} step_duration_sets={:?} step_plocked_sets={:?} step_selected_sets={:?} step_slider_sets={:?} step_haptic_sets={:?} reactive_apply_ms={:.3} reactive_flush_ms={:.3} reactive_cycle_trace_ms={:.3} reactive_hot={:?}",
            duration_ms(state_switch_profile.total),
            duration_ms(state_switch_profile.capture_current_snapshot),
            duration_ms(state_switch_profile.scene_lock_wait),
            duration_ms(state_switch_profile.save_current_snapshot),
            duration_ms(state_switch_profile.launch_scene_data),
            duration_ms(state_switch_profile.restore_tracks),
            duration_ms(state_switch_profile.collect_sample_ids),
            duration_ms(state_switch_profile.update_pattern_atoms),
            duration_ms(state_switch_profile.schedule_mod_resync),
            duration_ms(state_switch_profile.publish_scheduler_snapshot),
            duration_ms(sync_sequencer_profile.elapsed),
            duration_ms(sync_sequencer_profile.track_steps),
            duration_ms(sync_sequencer_profile.track_num_steps),
            duration_ms(sync_sequencer_profile.track_timebases),
            duration_ms(sync_sequencer_profile.track_duration_spans),
            duration_ms(sync_sequencer_profile.track_step_has_plocks),
            duration_ms(sync_sequencer_profile.track_playheads),
            duration_ms(sync_sequencer_profile.track_velocities),
            duration_ms(sync_sequencer_profile.track_durations),
            duration_ms(sync_sequencer_profile.track_auxas),
            duration_ms(sync_sequencer_profile.track_transposes),
            duration_ms(sync_sequencer_profile.track_pans),
            duration_ms(sync_sequencer_profile.track_syncs),
            duration_ms(sync_sequencer_profile.track_delays),
            duration_ms(sync_sequencer_profile.step_bindings.elapsed),
            duration_ms(sync_sequencer_profile.playhead_fields),
            duration_ms(sync_sequencer_profile.step_bindings.active_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.duration_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.plocked_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.selected_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.slider_elapsed),
            duration_ms(sync_sequencer_profile.step_bindings.haptic_elapsed),
            sync_sequencer_profile.step_bindings.active_sets,
            sync_sequencer_profile.step_bindings.duration_sets,
            sync_sequencer_profile.step_bindings.plocked_sets,
            sync_sequencer_profile.step_bindings.selected_sets,
            sync_sequencer_profile.step_bindings.slider_sets,
            sync_sequencer_profile.step_bindings.haptic_sets,
            duration_ms(trace.reactive_apply_duration),
            duration_ms(trace.reactive_flush_duration),
            duration_ms(trace.reactive_cycle_duration),
            reactive_hot,
        );

        assert_eq!(
            app.state.current_scene_index(),
            target_pattern,
            "scene switch should update the current project scene"
        );
        assert!(
            trace.widget_tree_flushes > 0,
            "scene switch should report widget tree work"
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    sequencer::paths::enter_sequencer_dir()?;
    sequencer::crash::install()?;

    // 1. Init audio engine
    let eng = engine::init_engine()?;
    let lg_ptr = eng.lg_ptr;
    let state = eng.state.clone();
    let stream = eng._stream;

    // 2. Create App. Start intentionally empty so the first action is choosing
    // a sound instead of editing a canned pattern.
    let master_recorder = eng.master_recorder.clone();
    let mut app = ui::App::new(
        eng.state.clone(),
        eng.lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder,
        eng.keyboard_tx,
    );

    let mut track_names: Vec<String> = Vec::new();

    // Collect node IDs for param pushing to audiograph
    let track_pan_ids: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(
        app.graph.track_node_ids.iter().map(|n| n.pan_id).collect(),
    ));
    let track_collapsed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(app.track_collapsed.clone()));
    let bus_state: Arc<Mutex<Vec<ui::BusChannelState>>> = Arc::new(Mutex::new(app.buses.clone()));
    let bus_node_ids: Arc<Mutex<Vec<ui::BusNodeIds>>> =
        Arc::new(Mutex::new(app.graph.bus_node_ids.clone()));
    let lg_raw = lg_ptr.0;

    // Shared current track index
    let current_track = Arc::new(AtomicUsize::new(0));
    // Selected steps for p-locking
    let selected_steps: Arc<Mutex<HashSet<usize>>> = Arc::new(Mutex::new(HashSet::new()));
    let selected_neural_neurons: sequencer::lisp_effect::SharedSelectedNeuralNeurons =
        Arc::new(Mutex::new(BTreeSet::new()));
    let piano_roll_selection: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_move_state: Arc<Mutex<Option<PianoRollMoveState>>> = Arc::new(Mutex::new(None));
    let step_clipboard: Arc<
        Mutex<Option<(usize, Vec<(usize, sequencer::sequencer::StepSnapshot)>)>>,
    > = Arc::new(Mutex::new(None));
    // UI-only counter for changes that shouldn't affect pattern_epoch (e.g. volume, selection)
    let ui_epoch = Arc::new(AtomicUsize::new(0));
    // FX/instrument panel refresh counter for changes that affect *fx* but
    // should not force *fx* to rerun on unrelated step-grid edits.
    let fx_epoch = Arc::new(AtomicUsize::new(0));
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
    } = init_runtime(
        &app,
        state.clone(),
        &track_names,
        track_pan_ids.clone(),
        track_collapsed.clone(),
        bus_state.clone(),
        bus_node_ids.clone(),
        current_track.clone(),
        selected_steps.clone(),
        piano_roll_selection.clone(),
        piano_roll_move_state.clone(),
        recording.clone(),
        master_recording.clone(),
        master_recorder.clone(),
        record_armed.clone(),
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

    let (mut editor, mut backend) = create_editor_and_backend(runtime, &app)?;

    // 5. Metal event loop
    let idle_frame_interval = Duration::from_secs_f64(1.0 / 30.0);
    let animation_frame_interval = Duration::from_secs_f64(1.0 / 60.0);
    let mut last_render_at = Instant::now() - idle_frame_interval;
    let mut stub_animation_cache = StubAnimationRenderCache::new();
    let mut pending_drag: Option<(Event, (f32, f32))> = None;
    let mut scroll_accum_y: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0;
    let mut soft_step_param_edit = SoftStepParamEdit::default();
    let mut lisp_hot_reload_watcher =
        LispHotReloadWatcher::start(editor.runtime().lisp_source_paths());
    let mut lisp_hot_reload_source_revision = editor.runtime().lisp_source_revision();

    // Inline editor session state (instrument/effect creation/editing)
    let mut editor_buffer_name: Option<String> = None;
    let mut editor_mode: Option<String> = None;
    let mut instrument_edit_session: Option<InstrumentEditSession> = None;
    let mut pending_instrument_preview: Option<PendingInstrumentPreview> = None;
    let mut pending_instrument_cancel_restore: Option<PendingInstrumentCancelRestore> = None;
    let mut effect_edit_session: Option<EffectEditSession> = None;
    let mut pending_effect_preview: Option<PendingEffectPreview> = None;
    let mut pending_effect_cancel_restore: Option<PendingEffectCancelRestore> = None;
    let mut pending_agentic_bubbles: HashMap<String, PendingAgenticBubble> = HashMap::new();
    let mut prev_playing = false;
    let mut prev_bpm: u32 = 0;
    let mut prev_playhead: u32 = u32::MAX;
    let mut prev_transport_playhead: u32 = u32::MAX;
    let mut prev_pattern_epoch: u64 = 0;
    let mut prev_current_track: usize = usize::MAX;
    let mut prev_cpu_load_bits: u32 = u32::MAX;
    let mut prev_peak_l_level = -1.0f64;
    let mut prev_peak_r_level = -1.0f64;
    let mut prev_master_recording = false;
    let mut prev_track_peak_levels: Vec<f64> = Vec::new();
    let mut prev_bus_peak_levels: Vec<f64> = Vec::new();
    let mut prev_modulator_phases: Vec<f64> = Vec::new();
    let mut prev_modulator_levels: Vec<f64> = Vec::new();
    let mut prev_bus_playheads: Vec<usize> = Vec::new();
    let mut prev_track_playheads: Vec<u32> = Vec::new();
    let mut prev_track_button_states = track_button_state_snapshot(&state);
    let mut prev_current_track_playhead_visible = false;
    let mut prev_ui_epoch: usize = 0;
    let mut prev_fx_epoch: usize = 0;
    let mut prev_active_buffer_name = editor.active_buffer().name.clone();
    let mut prev_selected_neural_neurons = selected_neural_neurons.lock().unwrap().clone();
    let mut prev_agent_generation_watermark = agent_generation_watermark(&app);
    let mut prev_sampler_analysis_key: Option<(usize, i32, u32, u32, usize)> = None;
    let mut prev_auto_follow = true;
    let mut watched_sampler_voice_track: Option<usize> = None;
    let mut watched_sampler_voice_ids: Vec<i32> = Vec::new();
    let mut cached_peak_l_level = 0.0f64;
    let mut cached_peak_r_level = 0.0f64;
    let mut cached_track_peak_levels = vec![0.0; track_names.len()];
    let mut cached_bus_peak_levels = read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
    let (mut cached_modulator_phases, mut cached_modulator_levels) =
        read_modulator_display_values(app.graph.lg, &app);
    let mut last_meter_poll_at = Instant::now() - METER_POLL_INTERVAL;
    let mut last_neural_visualization_poll_at = Instant::now() - NEURAL_VISUALIZATION_POLL_INTERVAL;
    let mut last_cpu_ui_poll_at = Instant::now() - CPU_UI_POLL_INTERVAL;
    let mut last_voice_count_log_at = Instant::now() - VOICE_COUNT_LOG_INTERVAL;
    let log_voice_counts = std::env::var_os("TINYSEQ_LOG_VOICE_COUNTS").is_some();
    if log_voice_counts {
        sequencer::voice_modulator::set_process_stats_enabled(true);
    }
    let mut cached_cpu_load_bits: u32 = 0.0f32.to_bits();

    eprintln!("metal_seq: entering event loop");
    let mut ui_loop_stats = UiLoopStats::new();

    loop {
        let sample_browser_ready = { sample_browser.borrow_mut().poll_ready() };
        match sample_browser_ready {
            Ok(true) => {
                if let Err(error) = refresh_sample_browser_buffer(&mut editor) {
                    editor.handle_host_event(HostEvent::Error(format!(
                        "Failed to refresh sample browser search: {error}"
                    )));
                }
            }
            Ok(false) => {}
            Err(error) => {
                editor.handle_host_event(HostEvent::Error(format!(
                    "Failed to query samples.db browser state: {error}"
                )));
            }
        }
        if let Some(watcher) = lisp_hot_reload_watcher.as_mut() {
            let source_revision = editor.runtime().lisp_source_revision();
            if source_revision != lisp_hot_reload_source_revision {
                watcher.set_watched_paths(editor.runtime().lisp_source_paths());
                lisp_hot_reload_source_revision = source_revision;
            }
            let changed_paths = watcher.poll_ready_paths();
            if !changed_paths.is_empty()
                && process_lisp_hot_reload_paths(&mut editor, changed_paths)
            {
                ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        if backend.poll_editable_shader_overrides() {
            editor.mark_needs_redraw();
        }
        pull_shared_bus_state(&mut app, &bus_state);
        pull_named_scratch_buffer_into_project(&editor, &mut app);
        editor.update_timers();
        let active_buffer_name = editor.active_buffer().name.clone();
        if active_buffer_name != prev_active_buffer_name {
            prev_active_buffer_name = active_buffer_name;
            let mut guard = active_delete_target.lock().unwrap();
            let should_clear = should_clear_active_delete_target_for_buffer(
                guard.as_ref(),
                &prev_active_buffer_name,
            );
            if should_clear {
                guard.take();
                drop(guard);
                active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                ui_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        let agent_generation = agent_generation_watermark(&app);
        if agent_generation != prev_agent_generation_watermark {
            eprintln!(
                "[agent-ui] generation changed {} -> {}; refreshing *agent*",
                prev_agent_generation_watermark, agent_generation
            );
            prev_agent_generation_watermark = agent_generation;
            {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "AGENT",
                    "generation",
                    Value::Number(agent_generation as f64),
                );
                rt.run_reactive_cycle();
            }
            editor.refresh_runtime_side_effects();
            editor.refresh_visible_layouts_for_buffer_named("*agent*");
            editor.refresh_visible_layouts_for_buffer_named("*agent-artifacts*");
            editor.mark_needs_redraw();
        }
        let (cols, rows) = backend.viewport_size();
        let (cell_w, cell_h) = backend.cell_dimensions();
        if cell_w > 0.0 {
            editor.set_layout_aspect(cell_h / cell_w);
        }
        editor.update_tile_rects(cols as u16, rows as u16);
        editor.sync_reactive_bindings_for_visible_layouts();
        if log_voice_counts && last_voice_count_log_at.elapsed() >= VOICE_COUNT_LOG_INTERVAL {
            log_active_voice_counts(&state, &track_names);
            last_voice_count_log_at = Instant::now();
        }

        let viewport_size = (cols, rows);
        let stub_animation_active = stub_animation_cache.is_active(
            viewport_size,
            backend.agent_instrument_stub_animation_visible(),
        );
        let widget_animation_active = editor.visible_widgets_animating();
        let frame_interval = if stub_animation_active || widget_animation_active {
            animation_frame_interval
        } else {
            idle_frame_interval
        };

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
        if widget_animation_active {
            editor.mark_needs_redraw();
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
                    if handle_metal_command_shortcut_with_ui_epoch(
                        &mut editor,
                        &raw_key,
                        &state,
                        &current_track,
                        &selected_steps,
                        &step_clipboard,
                        &ui_epoch,
                    ) {
                        if key_should_reveal_sequencer_track(&raw_key) {
                            let track = current_track.load(Ordering::Relaxed);
                            reveal_sequencer_current_track(&mut editor, &app, track);
                        }
                        editor.mark_needs_redraw();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    let key = normalize_command_shortcuts(raw_key);
                    if key.kind == crossterm::event::KeyEventKind::Press
                        && key.code == crossterm::event::KeyCode::Esc
                    {
                        let cleared_neural_selection = {
                            let mut selection = selected_neural_neurons.lock().unwrap();
                            let had_selection = !selection.is_empty();
                            selection.clear();
                            had_selection
                        };
                        if cleared_neural_selection {
                            let selection = selected_neural_neurons.lock().unwrap().clone();
                            sync_selected_neural_neuron_bindings(
                                editor.runtime_mut(),
                                &state,
                                &selection,
                            );
                            let track = current_track.load(Ordering::Relaxed);
                            sync_fx_param_binding_fields_with_neural_selection(
                                editor.runtime_mut(),
                                &app,
                                &state,
                                track,
                                &selected_steps,
                                Some(&selection),
                            );
                            sync_track_plocks_for_neural_selection(
                                editor.runtime_mut(),
                                &app,
                                &state,
                                track,
                                &selected_steps,
                                &selection,
                            );
                            prev_selected_neural_neurons = selection;
                            editor.mark_needs_redraw();
                            ui_loop_stats.note_event(event_started.elapsed());
                            continue;
                        }
                    }
                    if should_toggle_play_on_space(&editor, &key) {
                        let _ = editor.runtime_mut().eval_str("(seq-toggle-play)");
                        editor.refresh_runtime_side_effects();
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    if handle_metal_soft_step_param_key(
                        &mut editor,
                        &key,
                        &state,
                        &current_track,
                        &mut soft_step_param_edit,
                    ) {
                        ui_loop_stats.note_event(event_started.elapsed());
                        continue;
                    }
                    // Intercept keyboard for live recording when any track is armed
                    let any_armed = record_armed.lock().unwrap().iter().any(|a| *a);
                    let recording_key_outcome = if (any_armed
                        || held_note_for_key(&held_notes, &key))
                        && should_route_to_live_keyboard(&editor, &key, &held_notes)
                    {
                        handle_recording_key(
                            &key,
                            &state,
                            &record_armed,
                            &recording,
                            &keyboard_tx,
                            &keyboard_octave,
                            &current_track,
                            &held_notes,
                            &ui_epoch,
                        )
                    } else {
                        RecordingKeyOutcome::Ignored
                    };
                    let intercepted = recording_key_outcome.consumed();
                    if recording_key_outcome.recorded() {
                        let ct = current_track.load(Ordering::Relaxed);
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                        sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                        editor.mark_needs_redraw();
                    }
                    // Only pass Press events to the editor (Release is only for note-off)
                    if !intercepted && key.kind == crossterm::event::KeyEventKind::Press {
                        let should_reload_custom_ui = should_reload_custom_ui_after_key(&key);
                        let previous_track = current_track.load(Ordering::Relaxed);
                        editor.handle_key(key);
                        if should_reload_custom_ui {
                            reload_custom_instrument_ui(&mut editor);
                        }
                        let next_track = current_track.load(Ordering::Relaxed);
                        if editor.active_buffer().name == "*sequencer*"
                            && next_track != previous_track
                        {
                            let track = current_track.load(Ordering::Relaxed);
                            reveal_sequencer_current_track(&mut editor, &app, track);
                        }
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
                        backend.set_widget_cursor(editor.widget_cursor());
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
            backend.set_widget_cursor(editor.widget_cursor());
        }
        ui_loop_stats.note_gestures(gestures_started.elapsed());

        // 1b. Drain host commands (sample browser etc.)
        let host_commands_started = Instant::now();
        for command in editor.drain_host_commands() {
            if let HostCommand::Custom { name, payload } = command {
                let _ = current_track_for_app(&mut app, &current_track);
                match name.as_str() {
                    "reveal-sequencer-track" => {
                        if let Some(track) = extract_usize_from_payload(&payload, "track") {
                            if track < app.tracks.len() {
                                reveal_sequencer_current_track(&mut editor, &app, track);
                            }
                        }
                    }
                    "audition-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        eprintln!(
                            "sample-host-command: audition-sample payload={payload:?}; extracted_path={path_str:?}"
                        );
                        if let Some(path_str) = path_str {
                            if app.tracks.is_empty() {
                                editor.handle_host_event(HostEvent::Status(
                                    "Add a track before auditioning samples".to_string(),
                                ));
                                continue;
                            }
                            let path = Path::new(&path_str);
                            let Some(track) = current_track_for_app(&mut app, &current_track)
                            else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Add a track before auditioning samples".to_string(),
                                ));
                                continue;
                            };
                            match load_sample_into_sampler_track(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &selected_steps,
                                lg_raw,
                                track,
                                path,
                            ) {
                                Ok(new_name) => {
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
                    "add-track-sampler" => match app.graph_controller().add_blank_sampler_track() {
                        Ok(idx) => {
                            current_track.store(idx, Ordering::Relaxed);
                            let new_name = app.tracks[idx].clone();
                            track_names.push(new_name.clone());
                            {
                                let mut pan_ids = track_pan_ids.lock().unwrap();
                                pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                                push_solo_mutes(lg_raw, &state, &pan_ids);
                            }
                            record_armed.lock().unwrap().push(false);
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "num-tracks",
                                Value::Number(track_names.len() as f64),
                            );
                            rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                            set_current_track_reactive(rt, app.tracks.len(), idx);
                            rt.set_reactive("SEQ", "track-names", build_track_names(&track_names));
                            sync_all_track_sequencer_state(rt, &state, &app, idx, &selected_steps);
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, idx));
                            sync_step_param_lists(rt, &state, idx);
                            sync_track_mixer_state(rt, &app, &state);
                            sync_bus_mixer_state(rt, &app);
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                                "midi-effects",
                                build_midi_effects_value(&state, idx, &selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, idx, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            let selected_neural_snapshot =
                                selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                idx,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                idx,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(&state, idx, &app.graph.effect_descriptors),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added sampler track {}: {new_name}",
                                idx + 1
                            )));
                        }
                        Err(e) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error adding sampler track: {e}"
                            )));
                        }
                    },
                    "add-track-modulator" => match app.graph_controller().add_modulator_track() {
                        Ok(idx) => {
                            sync_after_agent_instrument_apply(
                                &mut app,
                                &mut editor,
                                &state,
                                idx,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                            );
                            let new_name = app.tracks[idx].clone();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Added modulator track {}: {new_name}",
                                idx + 1
                            )));
                        }
                        Err(e) => {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Error adding modulator track: {e}"
                            )));
                        }
                    },
                    "reanalyze-sample" => {
                        let Some(track) = current_track_for_app(&mut app, &current_track) else {
                            editor.handle_host_event(HostEvent::Status(
                                "No sample loaded on this track".to_string(),
                            ));
                            continue;
                        };
                        let Some(path) = app
                            .sampler_paths
                            .get(track)
                            .and_then(|path| path.as_ref())
                            .cloned()
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "No sample loaded on this track".to_string(),
                            ));
                            continue;
                        };
                        match sequencer::sampler::load_wav_buffer(lg_raw, &path) {
                            Ok(loaded) => {
                                app.submit_sample_analysis(&loaded);
                                let new_buffer_id = loaded.buffer_id;
                                let sample_rate = loaded.sample_rate;
                                app.graph_controller().send_sample_to_all_voices(
                                    track,
                                    new_buffer_id,
                                    sample_rate,
                                );
                                app.graph.track_buffer_ids[track] = new_buffer_id;
                                app.graph.track_sample_rates[track] = sample_rate;
                                let sample_name = app.tracks[track].clone();
                                app.register_loaded_sample_path(
                                    &sample_name,
                                    new_buffer_id,
                                    path.clone(),
                                );
                                app.publish_sampler_analysis_runtime(track);
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "instrument-panel",
                                    build_instrument_panel_value(&app, track, &selected_steps),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.handle_host_event(HostEvent::Status(
                                    "Re-analyzing sample".to_string(),
                                ));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error re-analyzing sample: {error}"
                                )));
                            }
                        }
                    }
                    "set-convolution-reverb-ir" => {
                        let path_str = extract_path_from_payload(&payload);
                        // bus >= 0 means a bus effect; absent/-1 means a track effect.
                        let bus = extract_usize_from_payload(&payload, "bus");
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        match (slot, path_str) {
                            (Some(slot), Some(path_str)) => {
                                let path = Path::new(&path_str);
                                let reference = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(path_str.as_str())
                                    .to_string();
                                let result = if let Some(bus_idx) = bus {
                                    app.set_conv_reverb_ir_bus(bus_idx, slot, path, &reference)
                                } else if let Some(track) = track {
                                    app.set_conv_reverb_ir(track, slot, path, &reference)
                                } else {
                                    Err("need a track or bus".to_string())
                                };
                                match result {
                                    Ok(()) => {
                                        // Refresh the relevant effects view so the label updates.
                                        let rt = editor.runtime_mut();
                                        if bus.is_some() {
                                            rt.set_reactive(
                                                "SEQ",
                                                "bus-effects",
                                                build_bus_effects_value_for_selection(
                                                    &app,
                                                    Some(&selected_steps),
                                                ),
                                            );
                                        } else if let Some(track) = track {
                                            rt.set_reactive(
                                                "SEQ",
                                                "effects",
                                                build_effects_value(
                                                    &state,
                                                    track,
                                                    &app.graph.effect_descriptors,
                                                    &selected_steps,
                                                ),
                                            );
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Loaded IR: {reference}"
                                        )));
                                    }
                                    Err(e) => editor.handle_host_event(HostEvent::Status(format!(
                                        "Error loading IR: {e}"
                                    ))),
                                }
                            }
                            _ => editor.handle_host_event(HostEvent::Status(
                                "set-convolution-reverb-ir: need slot, path".to_string(),
                            )),
                        }
                    }
                    "load-sample-into-track" => {
                        let path_str = extract_path_from_payload(&payload);
                        let track = extract_usize_from_payload(&payload, "track");
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        eprintln!(
                            "sample-host-command: load-sample-into-track payload={payload:?}; extracted_path={path_str:?}; extracted_track={track:?}; preserve_browser_context={preserve_browser_context}"
                        );
                        match (track, path_str) {
                            (Some(track), Some(path_str)) => {
                                if preserve_browser_context {
                                    preserve_sample_browser_context_for_loaded_sample(
                                        &mut editor,
                                        &path_str,
                                    );
                                }
                                let path = Path::new(&path_str);
                                match load_sample_into_sampler_track(
                                    &mut app,
                                    &mut editor,
                                    &state,
                                    &current_track,
                                    &mut track_names,
                                    &selected_steps,
                                    lg_raw,
                                    track,
                                    path,
                                ) {
                                    Ok(new_name) => {
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Loaded sample on track {}: {new_name}",
                                            track + 1
                                        )));
                                    }
                                    Err(e) => {
                                        if preserve_browser_context {
                                            preserve_sample_browser_context_for_loaded_sample(
                                                &mut editor,
                                                "",
                                            );
                                        }
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Error loading sample: {e}"
                                        )));
                                    }
                                }
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Status(
                                    "Sample drop is missing a track or path".to_string(),
                                ));
                            }
                        }
                    }
                    "add-track-sample" => {
                        let path_str = extract_path_from_payload(&payload);
                        let preserve_browser_context =
                            extract_bool_from_payload(&payload, "preserve-browser-context");
                        eprintln!(
                            "sample-host-command: add-track-sample payload={payload:?}; extracted_path={path_str:?}; preserve_browser_context={preserve_browser_context}"
                        );
                        if let Some(path_str) = path_str {
                            if preserve_browser_context {
                                preserve_sample_browser_context_for_loaded_sample(
                                    &mut editor,
                                    &path_str,
                                );
                            }
                            let path = Path::new(&path_str);
                            match app.graph_controller().add_track(path) {
                                Ok(idx) => {
                                    register_waveform_sample(path);
                                    current_track.store(idx, Ordering::Relaxed);
                                    let new_name = app.tracks[idx].clone();
                                    track_names.push(new_name.clone());
                                    // Update pan IDs for new track
                                    {
                                        let mut pan_ids = track_pan_ids.lock().unwrap();
                                        pan_ids.push(app.graph.track_node_ids[idx].pan_id);
                                        push_solo_mutes(lg_raw, &state, &pan_ids);
                                    }
                                    // Extend record_armed for new track
                                    record_armed.lock().unwrap().push(false);
                                    // Update reactive state
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "num-tracks",
                                        Value::Number(track_names.len() as f64),
                                    );
                                    rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
                                    set_current_track_reactive(rt, app.tracks.len(), idx);
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&track_names),
                                    );
                                    sync_all_track_sequencer_state(
                                        rt,
                                        &state,
                                        &app,
                                        idx,
                                        &selected_steps,
                                    );
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, idx));
                                    sync_step_param_lists(rt, &state, idx);
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_bus_mixer_state(rt, &app);
                                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                                        "midi-effects",
                                        build_midi_effects_value(&state, idx, &selected_steps),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(&app, idx, &selected_steps),
                                    );
                                    *accumulator_names.lock().unwrap() =
                                        build_accumulator_names(&app);
                                    let selected_neural_snapshot =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    sync_track_params_with_neural_selection(
                                        rt,
                                        &app,
                                        &state,
                                        idx,
                                        &selected_steps,
                                        Some(&selected_neural_snapshot),
                                    );
                                    sync_fx_param_binding_fields_with_neural_selection(
                                        rt,
                                        &app,
                                        &state,
                                        idx,
                                        &selected_steps,
                                        Some(&selected_neural_snapshot),
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
                                        "Added track {}: {new_name}",
                                        idx + 1
                                    )));
                                }
                                Err(e) => {
                                    if preserve_browser_context {
                                        preserve_sample_browser_context_for_loaded_sample(
                                            &mut editor,
                                            "",
                                        );
                                    }
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Error adding track: {e}"
                                    )));
                                }
                            }
                        }
                    }
                    "add-track-instrument" => {
                        handle_add_track_instrument_command(
                            &payload,
                            AddTrackInstrumentCtx {
                                app: &mut app,
                                editor: &mut editor,
                                state: &state,
                                current_track: &current_track,
                                track_names: &mut track_names,
                                track_pan_ids: &track_pan_ids,
                                record_armed: &record_armed,
                                selected_steps: &selected_steps,
                                accumulator_names: &accumulator_names,
                                cached_track_peak_levels: &cached_track_peak_levels,
                                ui_epoch: &ui_epoch,
                                lg_raw,
                            },
                        );
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
                                {
                                    let mut pan_ids = track_pan_ids.lock().unwrap();
                                    *pan_ids = app
                                        .graph
                                        .track_node_ids
                                        .iter()
                                        .map(|ids| ids.pan_id)
                                        .collect();
                                    push_solo_mutes(lg_raw, &state, &pan_ids);
                                }
                                cached_track_peak_levels = read_track_peak_levels(
                                    app.graph.lg,
                                    &track_pan_ids.lock().unwrap(),
                                );
                                cached_bus_peak_levels =
                                    read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                                (cached_modulator_phases, cached_modulator_levels) =
                                    read_modulator_display_values(app.graph.lg, &app);
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
                                sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                                sync_modulator_phase_fields(rt, &cached_modulator_phases);
                                sync_modulator_level_fields(rt, &cached_modulator_levels);
                                rt.clear_subtree_effects_for_named_target("*sequencer*");
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                refresh_visible_track_topology_layouts(&mut editor);
                                prev_track_playheads = track_playheads_snapshot(&state, &app);
                                prev_track_button_states = track_button_state_snapshot(&state);
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
                    "new-project" => {
                        app.start_new_project();
                        push_project_scratch_to_named_buffer(&mut editor, &app);
                        if let Err(error) =
                            evaluate_project_scratch_on_ui_runtime(&mut editor, &app)
                        {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scratch UI eval error: {error}"
                            )));
                        }
                        selected_steps.lock().unwrap().clear();
                        piano_roll_selection.lock().unwrap().clear();
                        track_names = app.tracks.clone();
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        current_track.store(0, Ordering::Relaxed);
                        {
                            let mut pan_ids = track_pan_ids.lock().unwrap();
                            pan_ids.clear();
                            push_solo_mutes(lg_raw, &state, &pan_ids);
                        }
                        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
                        *record_armed.lock().unwrap() = Vec::new();
                        *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                        cached_track_peak_levels.clear();
                        cached_bus_peak_levels =
                            read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                        (cached_modulator_phases, cached_modulator_levels) =
                            read_modulator_display_values(app.graph.lg, &app);
                        last_meter_poll_at = Instant::now();

                        let bpm = state.transport.bpm.load(Ordering::Relaxed);
                        let playing = state.transport.playing.load(Ordering::Relaxed);
                        let transport_playhead = state.transport.playhead.load(Ordering::Relaxed);
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
                        sync_bus_mixer_state(rt, &app);
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        sync_modulator_phase_fields(rt, &cached_modulator_phases);
                        sync_modulator_level_fields(rt, &cached_modulator_levels);
                        rt.set_reactive("SEQ", "num-tracks", Value::Number(0.0));
                        set_current_track_reactive(rt, 0, 0);
                        rt.set_reactive("SEQ", "track-ids", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-names", Value::List(vec![]));
                        rt.set_reactive("SEQ", "record-armed", Value::List(vec![]));
                        rt.set_reactive("SEQ", "selected-steps", Value::List(vec![]));
                        sync_playhead_fields(rt, 0, 1);
                        rt.set_reactive("SEQ", "steps", Value::List(vec![]));
                        rt.set_reactive("SEQ", "velocities", Value::List(vec![]));
                        rt.set_reactive("SEQ", "durations", Value::List(vec![]));
                        rt.set_reactive("SEQ", "transposes", Value::List(vec![]));
                        rt.set_reactive("SEQ", "pans", Value::List(vec![]));
                        rt.set_reactive("SEQ", "syncs", Value::List(vec![]));
                        rt.set_reactive("SEQ", "delays", Value::List(vec![]));
                        sync_track_mixer_empty_state(rt);
                        rt.set_reactive("SEQ", "effects", Value::List(vec![]));
                        rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
                        rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
                        rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
                        rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
                        sync_sidebar_browser(rt, &app, 0);
                        rt.clear_subtree_effects_for_named_target("*sequencer*");
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        refresh_visible_track_topology_layouts(&mut editor);

                        prev_current_track = 0;
                        prev_playhead = 0;
                        prev_transport_playhead = transport_playhead;
                        prev_bpm = bpm;
                        prev_playing = playing;
                        prev_pattern_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        prev_track_peak_levels.clear();
                        prev_modulator_phases = cached_modulator_phases.clone();
                        prev_modulator_levels = cached_modulator_levels.clone();
                        prev_bus_playheads = bus_playhead_snapshot(&app);
                        prev_track_playheads = track_playheads_snapshot(&state, &app);
                        prev_track_button_states = track_button_state_snapshot(&state);
                        prev_ui_epoch = ui_epoch.fetch_add(1, Ordering::Relaxed) + 1;

                        editor.handle_host_event(HostEvent::Status("New project".to_string()));
                    }
                    "save-project" => {
                        let _ = current_track_for_app(&mut app, &current_track);
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
                        ui_invalidations.clear();
                        expanded_step_projection.clear();
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
                                    let (neural_selection, wrote_neural_plock) =
                                        record_selected_neural_instrument_plock(
                                            &mut editor,
                                            &state,
                                            &selected_neural_neurons,
                                            track,
                                            param_idx,
                                            stored,
                                        );
                                    if !wrote_neural_plock {
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentParam {
                                                track,
                                                param_idx,
                                                value: stored,
                                            },
                                        );
                                    }
                                    sync_instrument_param_authoring_display(
                                        &mut editor,
                                        InstrumentParamDisplaySync {
                                            app: &app,
                                            state: &state,
                                            selected_steps: &selected_steps,
                                            selection: &neural_selection,
                                            track,
                                            param_idx,
                                            display_step: None,
                                            sync_plock_list: wrote_neural_plock,
                                            sync_sampler_times: true,
                                        },
                                    );
                                    if param_change_needs_fx_rebuild(&desc) {
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "toggle-instrument-param" => {
                        if let Value::Map(ref map) = payload {
                            let param_idx =
                                map.get("param-idx").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let Some(param_idx) = param_idx {
                                let track = current_track.load(Ordering::Relaxed);
                                if let Some(desc) = app
                                    .graph
                                    .instrument_descriptors
                                    .get(track)
                                    .and_then(|d| d.params.get(param_idx))
                                    .cloned()
                                {
                                    let slot = &app.state.pattern.instrument_slots[track];
                                    let selected: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    let neural_selection =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    let default = if param_idx
                                        < slot.num_params.load(Ordering::Relaxed) as usize
                                    {
                                        slot.defaults.get(param_idx)
                                    } else {
                                        desc.default
                                    };
                                    let current = sequencer::lisp_effect::selected_neural_instrument_plock_value(
                                        &state,
                                        &neural_selection,
                                        track,
                                        param_idx,
                                    )
                                    .or_else(|| {
                                        selected
                                            .iter()
                                            .copied()
                                            .min()
                                            .and_then(|step| slot.plocks.get(step, param_idx))
                                    })
                                    .unwrap_or(default);
                                    let next = desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                    let wrote_neural_plock = write_selected_neural_instrument_plock(
                                        &mut editor,
                                        &state,
                                        &neural_selection,
                                        track,
                                        param_idx,
                                        next,
                                    );
                                    if wrote_neural_plock {
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: true,
                                                sync_sampler_times: false,
                                            },
                                        );
                                    } else if selected.is_empty() {
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentParam {
                                                track,
                                                param_idx,
                                                value: next,
                                            },
                                        );
                                        if sync_instrument_param_value_field(
                                            editor.runtime_mut(),
                                            &app,
                                            track,
                                            param_idx,
                                            None,
                                        ) {
                                            editor.mark_needs_redraw();
                                        }
                                    } else {
                                        ui::apply_command(
                                            &mut app,
                                            ui::AppCommand::SetInstrumentPlockMulti {
                                                track,
                                                steps: selected,
                                                param_idx,
                                                value: next,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-effect-param" => {
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
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(value)) =
                                (slot_idx, param_idx, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let desc = app
                                    .graph
                                    .effect_descriptors
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                    .and_then(|desc| desc.params.get(param_idx))
                                    .cloned();
                                let clamped = desc
                                    .as_ref()
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                let (neural_selection, wrote_neural_plock) =
                                    record_selected_neural_effect_plock(
                                        &mut editor,
                                        &state,
                                        &selected_neural_neurons,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        clamped,
                                    );
                                if !wrote_neural_plock {
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetEffectParam {
                                            track,
                                            slot_idx,
                                            param_idx,
                                            value: clamped,
                                        },
                                    );
                                }
                                sync_effect_param_authoring_display(
                                    &mut editor,
                                    EffectParamDisplaySync {
                                        state: &state,
                                        effect_descriptors: &app.graph.effect_descriptors,
                                        app: &app,
                                        selected_steps: &selected_steps,
                                        selection: &neural_selection,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        display_step: None,
                                        sync_plock_list: wrote_neural_plock,
                                    },
                                );
                                if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "toggle-effect-param" => {
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
                            if let (Some(slot_idx), Some(param_idx)) = (slot_idx, param_idx) {
                                let selected: Vec<usize> =
                                    selected_steps.lock().unwrap().iter().copied().collect();
                                if map_bool(map, "bus-fx") {
                                    let bus_idx =
                                        map.get("bus").and_then(|cell| match &*cell.borrow() {
                                            Value::Number(n) => Some(*n as usize),
                                            _ => None,
                                        });
                                    if let Some(bus_idx) = bus_idx {
                                        let desc = app
                                            .buses
                                            .get(bus_idx)
                                            .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                            .and_then(|desc| desc.params.get(param_idx))
                                            .cloned();
                                        if let Some(desc) = desc {
                                            let current = app
                                                .buses
                                                .get(bus_idx)
                                                .and_then(|bus| bus.effect_slots.get(slot_idx))
                                                .map(|slot| {
                                                    let default = slot
                                                        .defaults
                                                        .get(param_idx)
                                                        .copied()
                                                        .unwrap_or(desc.default);
                                                    selected
                                                        .iter()
                                                        .copied()
                                                        .min()
                                                        .and_then(|step| {
                                                            slot.plocks
                                                                .get(step)
                                                                .and_then(|step_plocks| {
                                                                    step_plocks.get(param_idx)
                                                                })
                                                                .copied()
                                                                .flatten()
                                                        })
                                                        .unwrap_or(default)
                                                })
                                                .unwrap_or(desc.default);
                                            let next =
                                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                            if selected.is_empty() {
                                                match app.set_bus_effect_param(
                                                    bus_idx, slot_idx, param_idx, next,
                                                ) {
                                                    Ok(()) => {
                                                        app.publish_bus_gate_runtime();
                                                        *bus_state.lock().unwrap() =
                                                            app.buses.clone();
                                                        if sync_bus_effect_param_value_field(
                                                            editor.runtime_mut(),
                                                            &app,
                                                            bus_idx,
                                                            slot_idx,
                                                            param_idx,
                                                        ) {
                                                            editor.mark_needs_redraw();
                                                        }
                                                    }
                                                    Err(error) => {
                                                        editor.handle_host_event(
                                                            HostEvent::Status(format!(
                                                                "Error toggling bus effect param: {error}"
                                                            )),
                                                        );
                                                        continue;
                                                    }
                                                }
                                            } else if let Some(bus) = app.buses.get_mut(bus_idx) {
                                                if let Some(slot) =
                                                    bus.effect_slots.get_mut(slot_idx)
                                                {
                                                    for step in selected {
                                                        if step < slot.plocks.len()
                                                            && param_idx < slot.plocks[step].len()
                                                        {
                                                            slot.plocks[step][param_idx] =
                                                                Some(next);
                                                        }
                                                    }
                                                    app.publish_bus_gate_runtime();
                                                    *bus_state.lock().unwrap() = app.buses.clone();
                                                }
                                            }
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                } else if map_bool(map, "midi-fx") {
                                    let track = current_track.load(Ordering::Relaxed);
                                    let chain = state.pattern.track_params[track].midi_fx_chain();
                                    let desc = chain
                                        .get(slot_idx)
                                        .and_then(|name| {
                                            sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                        })
                                        .and_then(|desc| desc.params.get(param_idx).cloned());
                                    if let Some(desc) = desc {
                                        if let Some(slot) = state
                                            .pattern
                                            .midi_fx_slots
                                            .get(track)
                                            .and_then(|slots| slots.get(slot_idx))
                                        {
                                            let default = slot.defaults.get(param_idx);
                                            let current = selected
                                                .iter()
                                                .copied()
                                                .min()
                                                .and_then(|step| slot.plocks.get(step, param_idx))
                                                .unwrap_or(default);
                                            let next =
                                                desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                            if selected.is_empty() {
                                                slot.defaults.set(param_idx, next);
                                                if sync_midi_fx_param_value_field(
                                                    editor.runtime_mut(),
                                                    &state,
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    None,
                                                ) {
                                                    editor.mark_needs_redraw();
                                                }
                                            } else {
                                                for step in selected {
                                                    slot.set_plock(step, param_idx, next);
                                                }
                                            }
                                            state.publish_scheduler_snapshot();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                } else {
                                    let track = current_track.load(Ordering::Relaxed);
                                    let desc = app
                                        .graph
                                        .effect_descriptors
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                        .and_then(|desc| desc.params.get(param_idx))
                                        .cloned();
                                    if let Some(desc) = desc {
                                        let chain = &state.pattern.effect_chains[track];
                                        let neural_selection =
                                            selected_neural_neurons.lock().unwrap().clone();
                                        let current = chain
                                            .get(slot_idx)
                                            .map(|slot| {
                                                let default = slot.defaults.get(param_idx);
                                                sequencer::lisp_effect::selected_neural_effect_plock_value(
                                                    &state,
                                                    &neural_selection,
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                )
                                                .or_else(|| {
                                                    selected
                                                        .iter()
                                                        .copied()
                                                        .min()
                                                        .and_then(|step| {
                                                            slot.plocks.get(step, param_idx)
                                                        })
                                                })
                                                .unwrap_or(default)
                                            })
                                            .unwrap_or(desc.default);
                                        let next =
                                            desc.clamp(if current > 0.5 { 0.0 } else { 1.0 });
                                        let wrote_neural_plock = write_selected_neural_effect_plock(
                                            &mut editor,
                                            &state,
                                            &neural_selection,
                                            track,
                                            slot_idx,
                                            param_idx,
                                            next,
                                        );
                                        if wrote_neural_plock {
                                            sync_effect_param_authoring_display(
                                                &mut editor,
                                                EffectParamDisplaySync {
                                                    state: &state,
                                                    effect_descriptors: &app
                                                        .graph
                                                        .effect_descriptors,
                                                    app: &app,
                                                    selected_steps: &selected_steps,
                                                    selection: &neural_selection,
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    display_step: None,
                                                    sync_plock_list: true,
                                                },
                                            );
                                        } else if selected.is_empty() {
                                            ui::apply_command(
                                                &mut app,
                                                ui::AppCommand::SetEffectParam {
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    value: next,
                                                },
                                            );
                                            if sync_track_effect_param_value_field(
                                                editor.runtime_mut(),
                                                &state,
                                                &app.graph.effect_descriptors,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                None,
                                            ) {
                                                editor.mark_needs_redraw();
                                            }
                                        } else {
                                            ui::apply_command(
                                                &mut app,
                                                ui::AppCommand::SetEffectPlockMulti {
                                                    track,
                                                    slot_idx,
                                                    steps: selected,
                                                    param_idx,
                                                    value: next,
                                                },
                                            );
                                        }
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
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
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock) =
                                            record_selected_neural_instrument_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                param_idx,
                                                value,
                                            );
                                        if !wrote_neural_plock {
                                            ui::apply_command(
                                                &mut app,
                                                ui::AppCommand::SetInstrumentParam {
                                                    track,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                                sync_sampler_times: false,
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
                                    let (neural_selection, wrote_neural_plock) =
                                        record_selected_neural_instrument_plock(
                                            &mut editor,
                                            &state,
                                            &selected_neural_neurons,
                                            track,
                                            param_idx,
                                            stored,
                                        );
                                    if !wrote_neural_plock {
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
                                                value: stored,
                                            },
                                        );
                                    }
                                    let display_step = displayed_plock_step(
                                        &state,
                                        track,
                                        selected_plock_step(&selected_steps),
                                    );
                                    sync_instrument_param_authoring_display(
                                        &mut editor,
                                        InstrumentParamDisplaySync {
                                            app: &app,
                                            state: &state,
                                            selected_steps: &selected_steps,
                                            selection: &neural_selection,
                                            track,
                                            param_idx,
                                            display_step,
                                            sync_plock_list: wrote_neural_plock,
                                            sync_sampler_times: true,
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
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock) =
                                            record_selected_neural_instrument_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                param_idx,
                                                value,
                                            );
                                        if !wrote_neural_plock {
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
                                                    value,
                                                },
                                            );
                                        }
                                        sync_instrument_param_authoring_display(
                                            &mut editor,
                                            InstrumentParamDisplaySync {
                                                app: &app,
                                                state: &state,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                                sync_sampler_times: false,
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
                                        let is_delay_time = app
                                            .graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .map(|d| d.name == "Delay")
                                            .unwrap_or(false)
                                            && param_idx == 2;
                                        is_delay_time.then(|| {
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
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock) =
                                            record_selected_neural_effect_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value,
                                            );
                                        if !wrote_neural_plock {
                                            ui::apply_command(
                                                &mut app,
                                                ui::AppCommand::SetEffectParam {
                                                    track,
                                                    slot_idx,
                                                    param_idx,
                                                    value,
                                                },
                                            );
                                        }
                                        sync_effect_param_authoring_display(
                                            &mut editor,
                                            EffectParamDisplaySync {
                                                state: &state,
                                                effect_descriptors: &app.graph.effect_descriptors,
                                                app: &app,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-track-output" => {
                        if let Value::Map(ref map) = payload {
                            let label = map.get("label").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let payload_track =
                                map.get("track").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let Some(label) = label {
                                let track = payload_track
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                                let output = if label == "main" {
                                    Some(TrackOutput::Mix)
                                } else if label == "sends only" {
                                    Some(TrackOutput::None)
                                } else {
                                    app.buses
                                        .iter()
                                        .filter(|bus| bus.id != sequencer::sequencer::BusId::MIX)
                                        .find(|bus| bus.name == label)
                                        .map(|bus| TrackOutput::Bus(bus.id))
                                };
                                if let Some(output) = output {
                                    ui::apply_command(
                                        &mut app,
                                        ui::AppCommand::SetTrackOutput { track, output },
                                    );
                                    let rt = editor.runtime_mut();
                                    sync_track_mixer_state(rt, &app, &state);
                                    if track == current_track.load(Ordering::Relaxed) {
                                        let selected_neural_snapshot =
                                            selected_neural_neurons.lock().unwrap().clone();
                                        sync_track_params_with_neural_selection(
                                            rt,
                                            &app,
                                            &state,
                                            track,
                                            &selected_steps,
                                            Some(&selected_neural_snapshot),
                                        );
                                        sync_fx_param_binding_fields_with_neural_selection(
                                            rt,
                                            &app,
                                            &state,
                                            track,
                                            &selected_steps,
                                            Some(&selected_neural_snapshot),
                                        );
                                    }
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-mod-route" => {
                        if let Value::Map(ref map) = payload {
                            let source = map.get("source").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let dest = map.get("dest").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let input = map
                                .get("input")
                                .and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            if let (Some(source), Some(dest)) = (source, dest) {
                                match app.graph_controller().set_mod_route(source, dest, input) {
                                    Ok(()) => {
                                        let message = format!(
                                            "Connected mod route: track {} out -> track {} Ext{}",
                                            source + 1,
                                            dest + 1,
                                            input + 1
                                        );
                                        eprintln!("[mod-route] {message}");
                                        let rt = editor.runtime_mut();
                                        sync_track_mixer_state(rt, &app, &state);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(message));
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "[mod-route] rejected connect {} -> {}: {}",
                                            source + 1,
                                            dest + 1,
                                            error
                                        );
                                        editor.handle_host_event(HostEvent::Status(error));
                                    }
                                }
                            }
                        }
                    }
                    "delete-mod-route" => {
                        if let Value::Map(ref map) = payload {
                            let source = map.get("source").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let dest = map.get("dest").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let input = map
                                .get("input")
                                .and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            if let (Some(source), Some(dest)) = (source, dest) {
                                match app.graph_controller().delete_mod_route(source, dest, input) {
                                    Ok(()) => {
                                        let message = format!(
                                            "Disconnected mod route: track {} out -> track {} Ext{}",
                                            source + 1,
                                            dest + 1,
                                            input + 1
                                        );
                                        eprintln!("[mod-route] {message}");
                                        let rt = editor.runtime_mut();
                                        sync_track_mixer_state(rt, &app, &state);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(message));
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "[mod-route] rejected disconnect {} -> {}: {}",
                                            source + 1,
                                            dest + 1,
                                            error
                                        );
                                        editor.handle_host_event(HostEvent::Status(error));
                                    }
                                }
                            }
                        }
                    }
                    "refresh-mixer-ui" => {
                        let rt = editor.runtime_mut();
                        sync_track_mixer_state(rt, &app, &state);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                    }
                    "set-track-bus-send" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let amount = map.get("amount").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            let payload_track =
                                map.get("track").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                });
                            if let (Some(bus_idx), Some(amount)) = (bus_idx, amount) {
                                let Some(bus) = app.buses.get(bus_idx) else {
                                    continue;
                                };
                                if bus.id == sequencer::sequencer::BusId::MIX {
                                    continue;
                                }
                                let track = payload_track
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed));
                                if track >= state.active_track_count() {
                                    continue;
                                }
                                let mut sends = app.state.pattern.track_params[track].sends();
                                if let Some(send) =
                                    sends.iter_mut().find(|send| send.destination == bus.id)
                                {
                                    send.amount = amount;
                                } else {
                                    sends.push(TrackSendSnapshot {
                                        destination: bus.id,
                                        amount,
                                    });
                                }
                                sends.retain(|send| send.amount > 0.0);
                                ui::apply_command(
                                    &mut app,
                                    ui::AppCommand::SetTrackSends { track, sends },
                                );
                                let rt = editor.runtime_mut();
                                sync_track_bus_send_binding_field(rt, &app, &state, track, bus_idx);
                                let current = current_track.load(Ordering::Relaxed);
                                if track == current {
                                    sync_current_track_bus_send_binding_field(
                                        rt, &app, &state, track, bus_idx,
                                    );
                                }
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                            }
                        }
                    }
                    "toggle-bus-step" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    bus.gate_sequence.toggle_step(step);
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-bus-step-active" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            let active = map_bool(map, "active");
                            if let (Some(bus_idx), Some(step)) = (bus_idx, step) {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    if let Some(slot) = bus.gate_sequence.steps.get_mut(step) {
                                        if *slot != active {
                                            *slot = active;
                                            app.publish_bus_gate_runtime();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-step-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let step = map_number(map, "step").map(|value| value as usize);
                            let param = map_string(map, "param");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(bus_idx), Some(step), Some(param), Some(value)) =
                                (bus_idx, step, param, value)
                            {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    match param.as_str() {
                                        "duration" | "dur" => {
                                            bus.gate_sequence.set_step_duration(step, value);
                                        }
                                        "sync" | "syn" => {
                                            bus.gate_sequence.set_step_sync(step, value);
                                        }
                                        _ => {
                                            bus.gate_sequence.set_step_velocity(step, value);
                                        }
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-selected-bus-step-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let param = map_string(map, "param");
                            let value = map_number(map, "value").map(|value| value as f32);
                            if let (Some(bus_idx), Some(param), Some(value)) =
                                (bus_idx, param, value)
                            {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    for step in steps {
                                        if step >= bus.gate_sequence.num_steps {
                                            continue;
                                        }
                                        match param.as_str() {
                                            "duration" | "dur" => {
                                                bus.gate_sequence.set_step_duration(step, value);
                                            }
                                            "sync" | "syn" => {
                                                bus.gate_sequence.set_step_sync(step, value);
                                            }
                                            _ => {
                                                bus.gate_sequence.set_step_velocity(step, value);
                                            }
                                        }
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "select-bus-step-range" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let start = map_number(map, "start").map(|value| value as usize);
                            let end = map_number(map, "end").map(|value| value as usize);
                            if let (Some(bus_idx), Some(start), Some(end)) = (bus_idx, start, end) {
                                if let Some(bus) = app.buses.get(bus_idx) {
                                    let num_steps = bus.gate_sequence.num_steps.max(1);
                                    let a = start.min(num_steps - 1);
                                    let b = end.min(num_steps - 1);
                                    let lo = a.min(b);
                                    let hi = a.max(b);
                                    {
                                        let mut set = selected_steps.lock().unwrap();
                                        set.clear();
                                        set.extend(lo..=hi);
                                    }
                                    editor.runtime_mut().set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "select-all-bus-steps" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            if let Some(bus_idx) = bus_idx {
                                if let Some(bus) = app.buses.get(bus_idx) {
                                    let mut set = selected_steps.lock().unwrap();
                                    set.clear();
                                    set.extend(0..bus.gate_sequence.num_steps);
                                    drop(set);
                                    editor.runtime_mut().set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "delete-selected-bus-steps" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            if let Some(bus_idx) = bus_idx {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    let steps: Vec<usize> = {
                                        let mut set = selected_steps.lock().unwrap();
                                        let mut steps: Vec<usize> = set.iter().copied().collect();
                                        steps.sort_unstable();
                                        set.clear();
                                        steps
                                    };
                                    for step in steps {
                                        if step >= bus.gate_sequence.num_steps {
                                            continue;
                                        }
                                        bus.gate_sequence.steps[step] = false;
                                        bus.gate_sequence.velocities[step] = 1.0;
                                        bus.gate_sequence.durations[step] = 1.0;
                                        bus.gate_sequence.syncs[step] = 0.0;
                                        bus.gate_sequence.timebase_plocks[step] = None;
                                        bus.gate_sequence.swing_plocks[step] = None;
                                        bus.gate_sequence.swing_resolution_plocks[step] = None;
                                        for slot in &mut bus.effect_slots {
                                            if let Some(step_plocks) = slot.plocks.get_mut(step) {
                                                for value in step_plocks {
                                                    *value = None;
                                                }
                                            }
                                        }
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive(
                                        "SEQ",
                                        "selected-steps",
                                        build_selection_value(&selected_steps),
                                    );
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "move-bus-step-drag" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let start = map_number(map, "start").map(|value| value as usize);
                            let target = map_number(map, "target").map(|value| value as usize);
                            if let (Some(bus_idx), Some(start), Some(target)) =
                                (bus_idx, start, target)
                            {
                                if start != target {
                                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                                        let num_steps = bus.gate_sequence.num_steps;
                                        if start < num_steps && target < num_steps {
                                            let delta = target as isize - start as isize;
                                            let mut move_selection = false;
                                            let steps: Vec<usize> = {
                                                let set = selected_steps.lock().unwrap();
                                                if set.contains(&start) {
                                                    move_selection = true;
                                                    let mut steps: Vec<usize> =
                                                        set.iter().copied().collect();
                                                    steps.sort_unstable();
                                                    steps
                                                } else {
                                                    vec![start]
                                                }
                                            };
                                            if let (Some(&first), Some(&last)) =
                                                (steps.first(), steps.last())
                                            {
                                                let new_first = first as isize + delta;
                                                let new_last = last as isize + delta;
                                                if new_first >= 0 && new_last < num_steps as isize {
                                                    let snapshots: Vec<_> = steps
                                                        .iter()
                                                        .map(|&step| {
                                                            (
                                                                step,
                                                                bus.gate_sequence.steps[step],
                                                                bus.gate_sequence.velocities[step],
                                                                bus.gate_sequence.durations[step],
                                                                bus.gate_sequence.syncs[step],
                                                                bus.gate_sequence.timebase_plocks
                                                                    [step],
                                                                bus.gate_sequence.swing_plocks
                                                                    [step],
                                                                bus.gate_sequence
                                                                    .swing_resolution_plocks[step],
                                                                bus.effect_slots
                                                                    .iter()
                                                                    .map(|slot| {
                                                                        slot.plocks
                                                                            .get(step)
                                                                            .cloned()
                                                                            .unwrap_or_default()
                                                                    })
                                                                    .collect::<Vec<_>>(),
                                                            )
                                                        })
                                                        .collect();
                                                    for &step in &steps {
                                                        bus.gate_sequence.steps[step] = false;
                                                        bus.gate_sequence.velocities[step] = 1.0;
                                                        bus.gate_sequence.durations[step] = 1.0;
                                                        bus.gate_sequence.syncs[step] = 0.0;
                                                        bus.gate_sequence.timebase_plocks[step] =
                                                            None;
                                                        bus.gate_sequence.swing_plocks[step] = None;
                                                        bus.gate_sequence.swing_resolution_plocks
                                                            [step] = None;
                                                        for slot in &mut bus.effect_slots {
                                                            if let Some(step_plocks) =
                                                                slot.plocks.get_mut(step)
                                                            {
                                                                for value in step_plocks {
                                                                    *value = None;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    let moved_steps: Vec<usize> = snapshots
                                                        .iter()
                                                        .map(|(step, ..)| {
                                                            (*step as isize + delta) as usize
                                                        })
                                                        .collect();
                                                    for (snapshot, dst_step) in snapshots
                                                        .iter()
                                                        .zip(moved_steps.iter().copied())
                                                    {
                                                        bus.gate_sequence.steps[dst_step] =
                                                            snapshot.1;
                                                        bus.gate_sequence.velocities[dst_step] =
                                                            snapshot.2;
                                                        bus.gate_sequence.durations[dst_step] =
                                                            snapshot.3;
                                                        bus.gate_sequence.syncs[dst_step] =
                                                            snapshot.4;
                                                        bus.gate_sequence.timebase_plocks
                                                            [dst_step] = snapshot.5;
                                                        bus.gate_sequence.swing_plocks[dst_step] =
                                                            snapshot.6;
                                                        bus.gate_sequence.swing_resolution_plocks
                                                            [dst_step] = snapshot.7;
                                                        for (slot_idx, slot_plocks) in
                                                            snapshot.8.iter().enumerate()
                                                        {
                                                            let Some(slot) =
                                                                bus.effect_slots.get_mut(slot_idx)
                                                            else {
                                                                continue;
                                                            };
                                                            let Some(dst_plocks) =
                                                                slot.plocks.get_mut(dst_step)
                                                            else {
                                                                continue;
                                                            };
                                                            for (param_idx, value) in
                                                                slot_plocks.iter().enumerate()
                                                            {
                                                                if param_idx < dst_plocks.len() {
                                                                    dst_plocks[param_idx] = *value;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if move_selection {
                                                        let mut set =
                                                            selected_steps.lock().unwrap();
                                                        set.clear();
                                                        set.extend(moved_steps);
                                                    }
                                                    app.publish_bus_gate_runtime();
                                                    let rt = editor.runtime_mut();
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "selected-steps",
                                                        build_selection_value(&selected_steps),
                                                    );
                                                    sync_bus_mixer_state(rt, &app);
                                                    rt.run_reactive_cycle();
                                                    editor.refresh_runtime_side_effects();
                                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "shift-selected-bus-steps" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let direction =
                                map_number(map, "direction").map(|value| value.signum() as isize);
                            if let (Some(bus_idx), Some(delta)) = (bus_idx, direction) {
                                if delta != 0 {
                                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                                        let steps: Vec<usize> = {
                                            let set = selected_steps.lock().unwrap();
                                            let mut steps: Vec<usize> =
                                                set.iter().copied().collect();
                                            steps.sort_unstable();
                                            steps
                                        };
                                        if let (Some(&first), Some(&last)) =
                                            (steps.first(), steps.last())
                                        {
                                            let num_steps = bus.gate_sequence.num_steps;
                                            let can_shift = if delta < 0 {
                                                first > 0
                                            } else {
                                                last + 1 < num_steps
                                            };
                                            if can_shift {
                                                let snapshots: Vec<_> = steps
                                                    .iter()
                                                    .map(|&step| {
                                                        (
                                                            step,
                                                            bus.gate_sequence.steps[step],
                                                            bus.gate_sequence.velocities[step],
                                                            bus.gate_sequence.durations[step],
                                                            bus.gate_sequence.syncs[step],
                                                            bus.gate_sequence.timebase_plocks[step],
                                                            bus.gate_sequence.swing_plocks[step],
                                                            bus.gate_sequence
                                                                .swing_resolution_plocks[step],
                                                            bus.effect_slots
                                                                .iter()
                                                                .map(|slot| {
                                                                    slot.plocks
                                                                        .get(step)
                                                                        .cloned()
                                                                        .unwrap_or_default()
                                                                })
                                                                .collect::<Vec<_>>(),
                                                        )
                                                    })
                                                    .collect();
                                                for &step in &steps {
                                                    bus.gate_sequence.steps[step] = false;
                                                    bus.gate_sequence.velocities[step] = 1.0;
                                                    bus.gate_sequence.durations[step] = 1.0;
                                                    bus.gate_sequence.syncs[step] = 0.0;
                                                    bus.gate_sequence.timebase_plocks[step] = None;
                                                    bus.gate_sequence.swing_plocks[step] = None;
                                                    bus.gate_sequence.swing_resolution_plocks
                                                        [step] = None;
                                                    for slot in &mut bus.effect_slots {
                                                        if let Some(step_plocks) =
                                                            slot.plocks.get_mut(step)
                                                        {
                                                            for value in step_plocks {
                                                                *value = None;
                                                            }
                                                        }
                                                    }
                                                }
                                                let shifted_steps: Vec<usize> = snapshots
                                                    .iter()
                                                    .map(|(step, ..)| {
                                                        (*step as isize + delta) as usize
                                                    })
                                                    .collect();
                                                for (snapshot, dst_step) in snapshots
                                                    .iter()
                                                    .zip(shifted_steps.iter().copied())
                                                {
                                                    bus.gate_sequence.steps[dst_step] = snapshot.1;
                                                    bus.gate_sequence.velocities[dst_step] =
                                                        snapshot.2;
                                                    bus.gate_sequence.durations[dst_step] =
                                                        snapshot.3;
                                                    bus.gate_sequence.syncs[dst_step] = snapshot.4;
                                                    bus.gate_sequence.timebase_plocks[dst_step] =
                                                        snapshot.5;
                                                    bus.gate_sequence.swing_plocks[dst_step] =
                                                        snapshot.6;
                                                    bus.gate_sequence.swing_resolution_plocks
                                                        [dst_step] = snapshot.7;
                                                    for (slot_idx, slot_plocks) in
                                                        snapshot.8.iter().enumerate()
                                                    {
                                                        let Some(slot) =
                                                            bus.effect_slots.get_mut(slot_idx)
                                                        else {
                                                            continue;
                                                        };
                                                        let Some(dst_plocks) =
                                                            slot.plocks.get_mut(dst_step)
                                                        else {
                                                            continue;
                                                        };
                                                        for (param_idx, value) in
                                                            slot_plocks.iter().enumerate()
                                                        {
                                                            if param_idx < dst_plocks.len() {
                                                                dst_plocks[param_idx] = *value;
                                                            }
                                                        }
                                                    }
                                                }
                                                {
                                                    let mut set = selected_steps.lock().unwrap();
                                                    set.clear();
                                                    set.extend(shifted_steps);
                                                }
                                                app.publish_bus_gate_runtime();
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "selected-steps",
                                                    build_selection_value(&selected_steps),
                                                );
                                                sync_bus_mixer_state(rt, &app);
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-sequencer-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map_number(map, "bus").map(|value| value as usize);
                            let param = map_string(map, "param");
                            let value = map_number(map, "value").map(|value| value as f32);
                            let label = map_string(map, "label");
                            if let (Some(bus_idx), Some(param)) = (bus_idx, param) {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    let selected_bus_steps: Vec<usize> = selected_steps
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .copied()
                                        .filter(|step| *step < bus.gate_sequence.num_steps)
                                        .collect();
                                    let write_plock =
                                        !selected_bus_steps.is_empty() && param != "num-steps";
                                    match param.as_str() {
                                        "num-steps" => {
                                            if let Some(value) = value {
                                                bus.gate_sequence.set_num_steps(value as usize);
                                            }
                                        }
                                        "swing" => {
                                            if let Some(value) = value {
                                                let swing = value.clamp(50.0, 75.0);
                                                if write_plock {
                                                    for step in &selected_bus_steps {
                                                        bus.gate_sequence.swing_plocks[*step] =
                                                            Some(swing);
                                                    }
                                                } else {
                                                    bus.gate_sequence.swing = swing;
                                                }
                                            }
                                        }
                                        "timebase" => {
                                            if let Some(label) = label {
                                                let normalized = label.to_ascii_lowercase();
                                                if let Some(idx) =
                                                    Timebase::LABELS.iter().position(|candidate| {
                                                        candidate.to_ascii_lowercase() == normalized
                                                    })
                                                {
                                                    let timebase = Timebase::ALL[idx];
                                                    if write_plock {
                                                        for step in &selected_bus_steps {
                                                            bus.gate_sequence.timebase_plocks
                                                                [*step] = Some(timebase);
                                                        }
                                                    } else {
                                                        bus.gate_sequence.timebase = timebase;
                                                    }
                                                }
                                            }
                                        }
                                        "swing-resolution" => {
                                            if let Some(label) = label {
                                                let normalized = label.to_ascii_lowercase();
                                                if let Some(idx) = SwingResolution::LABELS
                                                    .iter()
                                                    .position(|candidate| {
                                                        candidate.to_ascii_lowercase() == normalized
                                                    })
                                                {
                                                    let resolution = SwingResolution::ALL[idx];
                                                    if write_plock {
                                                        for step in &selected_bus_steps {
                                                            bus.gate_sequence
                                                                .swing_resolution_plocks[*step] =
                                                                Some(resolution);
                                                        }
                                                    } else {
                                                        bus.gate_sequence.swing_resolution =
                                                            resolution;
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                    app.publish_bus_gate_runtime();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-bus-effect-param" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(value)) =
                                (bus_idx, slot_idx, param_idx, value)
                            {
                                let desc = app
                                    .buses
                                    .get(bus_idx)
                                    .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                    .and_then(|desc| desc.params.get(param_idx))
                                    .cloned();
                                match app.set_bus_effect_param(bus_idx, slot_idx, param_idx, value)
                                {
                                    Ok(()) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        sync_bus_effect_param_value_field(
                                            editor.runtime_mut(),
                                            &app,
                                            bus_idx,
                                            slot_idx,
                                            param_idx,
                                        );
                                        if desc.as_ref().is_some_and(param_change_needs_fx_rebuild)
                                        {
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error setting bus effect param: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "set-bus-effect-plock" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(value)) =
                                (bus_idx, slot_idx, param_idx, value)
                            {
                                if let Some(bus) = app.buses.get_mut(bus_idx) {
                                    if let Some(slot) = bus.effect_slots.get_mut(slot_idx) {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        for step in steps {
                                            if step < slot.plocks.len()
                                                && param_idx < slot.plocks[step].len()
                                            {
                                                slot.plocks[step][param_idx] = Some(value);
                                            }
                                        }
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-effect-param-option" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(label)) =
                                (bus_idx, slot_idx, param_idx, label)
                            {
                                if let Some(selected_idx) = app.bus_effect_param_option_index(
                                    bus_idx, slot_idx, param_idx, &label,
                                ) {
                                    let is_host_sidechain = matches!(
                                        app.buses
                                            .get(bus_idx)
                                            .and_then(|bus| bus.effect_descriptors.get(slot_idx))
                                            .and_then(|desc| desc.params.get(param_idx))
                                            .and_then(|param| param.host_control.as_ref()),
                                        Some(sequencer::effects::HostControl::FxSidechain { .. })
                                    );
                                    if is_host_sidechain {
                                        app.apply_bus_effect_sidechain_selection(
                                            bus_idx,
                                            slot_idx,
                                            param_idx,
                                            selected_idx,
                                        );
                                    }
                                    match app.set_bus_effect_param(
                                        bus_idx,
                                        slot_idx,
                                        param_idx,
                                        selected_idx as f32,
                                    ) {
                                        Ok(()) => {
                                            app.publish_bus_gate_runtime();
                                            *bus_state.lock().unwrap() = app.buses.clone();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.set_reactive(
                                                "SEQ",
                                                "bus-effects",
                                                build_bus_effects_value_for_selection(
                                                    &app,
                                                    Some(&selected_steps),
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error setting bus effect option: {error}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "set-bus-effect-plock-option" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            if let (Some(bus_idx), Some(slot_idx), Some(param_idx), Some(label)) =
                                (bus_idx, slot_idx, param_idx, label)
                            {
                                if let Some(selected_idx) = app.bus_effect_param_option_index(
                                    bus_idx, slot_idx, param_idx, &label,
                                ) {
                                    if let Some(bus) = app.buses.get_mut(bus_idx) {
                                        if let Some(slot) = bus.effect_slots.get_mut(slot_idx) {
                                            let steps: Vec<usize> = selected_steps
                                                .lock()
                                                .unwrap()
                                                .iter()
                                                .copied()
                                                .collect();
                                            for step in steps {
                                                if step < slot.plocks.len()
                                                    && param_idx < slot.plocks[step].len()
                                                {
                                                    slot.plocks[step][param_idx] =
                                                        Some(selected_idx as f32);
                                                }
                                            }
                                            app.publish_bus_gate_runtime();
                                            *bus_state.lock().unwrap() = app.buses.clone();
                                            let rt = editor.runtime_mut();
                                            sync_bus_mixer_state(rt, &app);
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
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
                                        let is_delay_time = app
                                            .graph
                                            .effect_descriptors
                                            .get(track)
                                            .and_then(|d| d.get(slot_idx))
                                            .map(|d| d.name == "Delay")
                                            .unwrap_or(false)
                                            && param_idx == 2;
                                        is_delay_time.then(|| {
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
                                        let value = selected_idx as f32;
                                        let (neural_selection, wrote_neural_plock) =
                                            record_selected_neural_effect_plock(
                                                &mut editor,
                                                &state,
                                                &selected_neural_neurons,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                value,
                                            );
                                        if !wrote_neural_plock {
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
                                                    value,
                                                },
                                            );
                                        }
                                        sync_effect_param_authoring_display(
                                            &mut editor,
                                            EffectParamDisplaySync {
                                                state: &state,
                                                effect_descriptors: &app.graph.effect_descriptors,
                                                app: &app,
                                                selected_steps: &selected_steps,
                                                selection: &neural_selection,
                                                track,
                                                slot_idx,
                                                param_idx,
                                                display_step: None,
                                                sync_plock_list: wrote_neural_plock,
                                            },
                                        );
                                    }
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-midi-fx-param" => {
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
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(value)) =
                                (slot_idx, param_idx, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                let desc = chain
                                    .get(slot_idx)
                                    .and_then(|name| {
                                        sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                    })
                                    .and_then(|desc| desc.params.get(param_idx).cloned());
                                let clamped = desc
                                    .as_ref()
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                if let Some(slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    slot.defaults.set(param_idx, clamped);
                                    state.publish_scheduler_snapshot();
                                    sync_midi_fx_param_value_field(
                                        editor.runtime_mut(),
                                        &state,
                                        track,
                                        slot_idx,
                                        param_idx,
                                        None,
                                    );
                                    if desc.as_ref().is_some_and(param_change_needs_fx_rebuild) {
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-midi-fx-plock" => {
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
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(slot_idx), Some(param_idx), Some(value)) =
                                (slot_idx, param_idx, value)
                            {
                                let track = current_track.load(Ordering::Relaxed);
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                let clamped = chain
                                    .get(slot_idx)
                                    .and_then(|name| {
                                        sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                    })
                                    .and_then(|desc| desc.params.get(param_idx).cloned())
                                    .map(|p| value.clamp(p.min, p.max))
                                    .unwrap_or(value);
                                if let Some(slot) = state
                                    .pattern
                                    .midi_fx_slots
                                    .get(track)
                                    .and_then(|slots| slots.get(slot_idx))
                                {
                                    let steps: Vec<usize> =
                                        selected_steps.lock().unwrap().iter().copied().collect();
                                    for step in steps {
                                        slot.set_plock(step, param_idx, clamped);
                                    }
                                    state.publish_scheduler_snapshot();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "set-midi-fx-param-option" => {
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
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                if let Some(selected_idx) = chain
                                    .get(slot_idx)
                                    .and_then(|name| midi_fx_option_index(name, param_idx, &label))
                                {
                                    if let Some(slot) = state
                                        .pattern
                                        .midi_fx_slots
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                    {
                                        slot.defaults.set(param_idx, selected_idx as f32);
                                        state.publish_scheduler_snapshot();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    }
                    "set-midi-fx-plock-option" => {
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
                                let chain = state.pattern.track_params[track].midi_fx_chain();
                                if let Some(selected_idx) = chain
                                    .get(slot_idx)
                                    .and_then(|name| midi_fx_option_index(name, param_idx, &label))
                                {
                                    if let Some(slot) = state
                                        .pattern
                                        .midi_fx_slots
                                        .get(track)
                                        .and_then(|slots| slots.get(slot_idx))
                                    {
                                        let steps: Vec<usize> = selected_steps
                                            .lock()
                                            .unwrap()
                                            .iter()
                                            .copied()
                                            .collect();
                                        for step in steps {
                                            slot.set_plock(step, param_idx, selected_idx as f32);
                                        }
                                        state.publish_scheduler_snapshot();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
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
                                sync_instrument_base_note_value_field(
                                    editor.runtime_mut(),
                                    &app,
                                    track,
                                );
                            }
                        }
                    }
                    "set-track-plock-entry" => {
                        if let Value::Map(ref map) = payload {
                            let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            let value = map.get("value").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as f32),
                                _ => None,
                            });
                            if let (Some(target), Some(step), Some(value)) = (target, step, value) {
                                let track = current_track.load(Ordering::Relaxed);
                                match target.as_str() {
                                    "timebase" => {
                                        let idx = (value.round() as usize)
                                            .min(sequencer::sequencer::Timebase::ALL.len() - 1);
                                        state.pattern.timebase_plocks[track]
                                            .set(step, sequencer::sequencer::Timebase::ALL[idx]);
                                        state.publish_scheduler_snapshot();
                                    }
                                    "swing" => {
                                        state.pattern.swing_plocks[track].set(step, value);
                                        state.publish_scheduler_snapshot();
                                    }
                                    "swing-resolution" => {
                                        let idx = (value.round() as usize).min(
                                            sequencer::sequencer::SwingResolution::ALL.len() - 1,
                                        );
                                        state.pattern.swing_resolution_plocks[track].set(
                                            step,
                                            sequencer::sequencer::SwingResolution::ALL[idx],
                                        );
                                        state.publish_scheduler_snapshot();
                                    }
                                    "instrument" => {
                                        if let Some(param_idx) = param_idx {
                                            if let Some(desc) = app
                                                .graph
                                                .instrument_descriptors
                                                .get(track)
                                                .and_then(|d| d.params.get(param_idx))
                                                .cloned()
                                            {
                                                let stored =
                                                    desc.clamp(desc.user_input_to_stored(value));
                                                state.pattern.instrument_slots[track]
                                                    .set_plock(step, param_idx, stored);
                                                state.publish_scheduler_snapshot();
                                            }
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
                                                .pattern
                                                .effect_chains
                                                .get(track)
                                                .and_then(|chain| chain.get(slot_idx))
                                            {
                                                let clamped = app
                                                    .graph
                                                    .effect_descriptors
                                                    .get(track)
                                                    .and_then(|d| d.get(slot_idx))
                                                    .and_then(|d| d.params.get(param_idx))
                                                    .map(|p| value.clamp(p.min, p.max))
                                                    .unwrap_or(value);
                                                slot.set_plock(step, param_idx, clamped);
                                                state.publish_scheduler_snapshot();
                                            }
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
                                                .pattern
                                                .midi_fx_slots
                                                .get(track)
                                                .and_then(|slots| slots.get(slot_idx))
                                            {
                                                let chain = state.pattern.track_params[track]
                                                    .midi_fx_chain();
                                                let clamped = chain
                                                    .get(slot_idx)
                                                    .and_then(|name| {
                                                        sequencer::lisp_effect::load_midi_fx_descriptor(name)
                                                    })
                                                    .and_then(|desc| {
                                                        desc.params.get(param_idx).cloned()
                                                    })
                                                    .map(|p| value.clamp(p.min, p.max))
                                                    .unwrap_or(value);
                                                slot.set_plock(step, param_idx, clamped);
                                                state.publish_scheduler_snapshot();
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "set-track-plock-entry-option" => {
                        if let Value::Map(ref map) = payload {
                            let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            if let (Some(target), Some(step), Some(label)) = (target, step, label) {
                                let track = current_track.load(Ordering::Relaxed);
                                match target.as_str() {
                                    "timebase" => {
                                        if let Some(idx) = sequencer::sequencer::Timebase::LABELS
                                            .iter()
                                            .position(|item| *item == label)
                                        {
                                            state.pattern.timebase_plocks[track].set(
                                                step,
                                                sequencer::sequencer::Timebase::ALL[idx],
                                            );
                                            state.publish_scheduler_snapshot();
                                        }
                                    }
                                    "swing-resolution" => {
                                        if let Some(idx) =
                                            sequencer::sequencer::SwingResolution::LABELS
                                                .iter()
                                                .position(|item| *item == label)
                                        {
                                            state.pattern.swing_resolution_plocks[track].set(
                                                step,
                                                sequencer::sequencer::SwingResolution::ALL[idx],
                                            );
                                            state.publish_scheduler_snapshot();
                                        }
                                    }
                                    "instrument" => {
                                        if let Some(param_idx) = param_idx {
                                            if let Some(selected_idx) = app
                                                .graph
                                                .instrument_descriptors
                                                .get(track)
                                                .and_then(|d| d.params.get(param_idx))
                                                .and_then(|p| match &p.kind {
                                                    sequencer::effects::ParamKind::Enum {
                                                        labels,
                                                    } => labels
                                                        .iter()
                                                        .position(|item| item == &label),
                                                    sequencer::effects::ParamKind::Boolean => {
                                                        match label.as_str() {
                                                            "on" | "ON" => Some(1),
                                                            "off" | "OFF" => Some(0),
                                                            _ => None,
                                                        }
                                                    }
                                                    _ => None,
                                                })
                                            {
                                                state.pattern.instrument_slots[track].set_plock(
                                                    step,
                                                    param_idx,
                                                    selected_idx as f32,
                                                );
                                                state.publish_scheduler_snapshot();
                                            }
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            if let Some(selected_idx) = app
                                                .graph
                                                .effect_descriptors
                                                .get(track)
                                                .and_then(|d| d.get(slot_idx))
                                                .and_then(|d| d.params.get(param_idx))
                                                .and_then(|p| match &p.kind {
                                                    sequencer::effects::ParamKind::Enum {
                                                        labels,
                                                    } => labels
                                                        .iter()
                                                        .position(|item| item == &label),
                                                    sequencer::effects::ParamKind::Boolean => {
                                                        match label.as_str() {
                                                            "on" | "ON" => Some(1),
                                                            "off" | "OFF" => Some(0),
                                                            _ => None,
                                                        }
                                                    }
                                                    _ => None,
                                                })
                                            {
                                                if let Some(slot) = state
                                                    .pattern
                                                    .effect_chains
                                                    .get(track)
                                                    .and_then(|chain| chain.get(slot_idx))
                                                {
                                                    slot.set_plock(
                                                        step,
                                                        param_idx,
                                                        selected_idx as f32,
                                                    );
                                                    state.publish_scheduler_snapshot();
                                                }
                                            }
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(slot_idx), Some(param_idx)) =
                                            (slot_idx, param_idx)
                                        {
                                            let chain =
                                                state.pattern.track_params[track].midi_fx_chain();
                                            if let Some(selected_idx) = chain
                                                .get(slot_idx)
                                                .and_then(|name| {
                                                    sequencer::lisp_effect::load_midi_fx_descriptor(
                                                        name,
                                                    )
                                                })
                                                .and_then(|desc| {
                                                    desc.params.get(param_idx).and_then(|p| {
                                                        match &p.kind {
                                                            sequencer::effects::ParamKind::Enum {
                                                                labels,
                                                            } => labels
                                                                .iter()
                                                                .position(|item| item == &label),
                                                            sequencer::effects::ParamKind::Boolean => {
                                                                match label.as_str() {
                                                                    "on" | "ON" => Some(1),
                                                                    "off" | "OFF" => Some(0),
                                                                    _ => None,
                                                                }
                                                            }
                                                            _ => None,
                                                        }
                                                    })
                                                })
                                            {
                                                if let Some(slot) = state
                                                    .pattern
                                                    .midi_fx_slots
                                                    .get(track)
                                                    .and_then(|slots| slots.get(slot_idx))
                                                {
                                                    slot.set_plock(
                                                        step,
                                                        param_idx,
                                                        selected_idx as f32,
                                                    );
                                                    state.publish_scheduler_snapshot();
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    "clear-track-plock-entry" => {
                        if let Value::Map(ref map) = payload {
                            let target = map.get("target").and_then(|cell| match &*cell.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            });
                            let step = map.get("step-idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
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
                            let target_track =
                                map.get("target-track")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) => Some(*n as usize),
                                        _ => None,
                                    });
                            let network_id =
                                map.get("network-id")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) => Some(*n as u64),
                                        _ => None,
                                    });
                            let neuron_idx =
                                map.get("neuron-idx")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) => Some(*n as usize),
                                        _ => None,
                                    });
                            if let Some(target) = target {
                                let track = current_track.load(Ordering::Relaxed);
                                let mut changed = false;
                                match target.as_str() {
                                    "timebase" => {
                                        if let Some(step) = step {
                                            state.pattern.timebase_plocks[track].clear(step);
                                        }
                                    }
                                    "swing" => {
                                        if let Some(step) = step {
                                            state.pattern.swing_plocks[track].clear(step);
                                        }
                                    }
                                    "swing-resolution" => {
                                        if let Some(step) = step {
                                            state.pattern.swing_resolution_plocks[track].clear(step)
                                        }
                                    }
                                    "instrument" => {
                                        if let (Some(step), Some(param_idx)) = (step, param_idx) {
                                            state.pattern.instrument_slots[track]
                                                .plocks
                                                .clear_param(step, param_idx);
                                        }
                                    }
                                    "effect" => {
                                        if let (Some(step), Some(slot_idx), Some(param_idx)) =
                                            (step, slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
                                                .pattern
                                                .effect_chains
                                                .get(track)
                                                .and_then(|chain| chain.get(slot_idx))
                                            {
                                                slot.plocks.clear_param(step, param_idx);
                                            }
                                        }
                                    }
                                    "midi-fx" => {
                                        if let (Some(step), Some(slot_idx), Some(param_idx)) =
                                            (step, slot_idx, param_idx)
                                        {
                                            if let Some(slot) = state
                                                .pattern
                                                .midi_fx_slots
                                                .get(track)
                                                .and_then(|slots| slots.get(slot_idx))
                                            {
                                                slot.plocks.clear_param(step, param_idx);
                                            }
                                        }
                                    }
                                    "neural-instrument" => {
                                        if let (
                                            Some(network_id),
                                            Some(neuron_idx),
                                            Some(target_track),
                                            Some(param_idx),
                                        ) = (network_id, neuron_idx, target_track, param_idx)
                                        {
                                            match sequencer::lisp_effect::clear_neural_instrument_plock_by_network_id(
                                                &state,
                                                network_id,
                                                neuron_idx,
                                                target_track,
                                                param_idx,
                                            ) {
                                                Ok(removed) => changed |= removed,
                                                Err(error) => editor.handle_host_event(
                                                    HostEvent::Status(format!(
                                                        "Error clearing neuron instrument p-lock: {error}"
                                                    )),
                                                ),
                                            }
                                        }
                                    }
                                    "neural-effect" => {
                                        if let (
                                            Some(network_id),
                                            Some(neuron_idx),
                                            Some(target_track),
                                            Some(slot_idx),
                                            Some(param_idx),
                                        ) = (
                                            network_id,
                                            neuron_idx,
                                            target_track,
                                            slot_idx,
                                            param_idx,
                                        ) {
                                            match sequencer::lisp_effect::clear_neural_effect_plock_by_network_id(
                                                &state,
                                                network_id,
                                                neuron_idx,
                                                target_track,
                                                slot_idx,
                                                param_idx,
                                            ) {
                                                Ok(removed) => changed |= removed,
                                                Err(error) => editor.handle_host_event(
                                                    HostEvent::Status(format!(
                                                        "Error clearing neuron effect p-lock: {error}"
                                                    )),
                                                ),
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                if changed {
                                    let selection = selected_neural_neurons.lock().unwrap().clone();
                                    sync_track_plocks_for_neural_selection(
                                        editor.runtime_mut(),
                                        &app,
                                        &state,
                                        track,
                                        &selected_steps,
                                        &selection,
                                    );
                                    editor.runtime_mut().run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.mark_needs_redraw();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                } else if step.is_some() {
                                    state.publish_scheduler_snapshot();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    "add-bus-effect" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let effect_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                });
                            if let (Some(bus_idx), Some(effect_name)) = (bus_idx, effect_name) {
                                match app.add_bus_effect_sync(bus_idx, &effect_name) {
                                    Ok(slot_idx) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        editor.reset_widget_scroll_for_buffer_named("*fx*");
                                        let fx_render_status =
                                            editor.runtime_mut().take_status_message();
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        if let Some(status) = fx_render_status {
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "FX UI error after adding bus effect: {status}"
                                            )));
                                        } else {
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added bus effect '{}' to slot {}",
                                                effect_name,
                                                slot_idx + 1
                                            )));
                                        }
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error adding bus effect: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "add-builtin-bus-effect" => {
                        if let Value::Map(ref map) = payload {
                            let bus_idx = map.get("bus").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            let effect_name =
                                map.get("name").and_then(|cell| match &*cell.borrow() {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                });
                            if let (Some(bus_idx), Some(effect_name)) = (bus_idx, effect_name) {
                                match app.add_builtin_bus_effect_sync(bus_idx, &effect_name) {
                                    Ok(slot_idx) => {
                                        app.publish_bus_gate_runtime();
                                        *bus_state.lock().unwrap() = app.buses.clone();
                                        let rt = editor.runtime_mut();
                                        sync_bus_mixer_state(rt, &app);
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        editor.reset_widget_scroll_for_buffer_named("*fx*");
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Added built-in bus effect '{}' to slot {}",
                                            effect_name,
                                            slot_idx + 1
                                        )));
                                    }
                                    Err(error) => editor.handle_host_event(HostEvent::Status(
                                        format!("Error adding built-in bus effect: {error}"),
                                    )),
                                }
                            }
                        }
                    }
                    "insert-builtin-bus-effect-before-slot" => {
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(bus_idx), Some(slot), Some(effect_name)) =
                            (bus_idx, slot, effect_name)
                        {
                            match app.insert_builtin_bus_effect_before_slot_sync(
                                bus_idx,
                                slot,
                                &effect_name,
                            ) {
                                Ok(slot_idx) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted built-in bus effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting built-in bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-bus-effect-before-slot" => {
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(bus_idx), Some(slot), Some(effect_name)) =
                            (bus_idx, slot, effect_name)
                        {
                            match app.insert_bus_effect_before_slot_sync(
                                bus_idx,
                                slot,
                                &effect_name,
                            ) {
                                Ok(slot_idx) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted bus effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting bus effect: {error}"
                                ))),
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
                    "add-effect-to-track" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(effect_name)) = (track, effect_name) {
                            if track >= app.tracks.len() {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Track {} does not exist",
                                    track + 1
                                )));
                                continue;
                            }
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            if let Some(slot_idx) = app.next_free_custom_slot() {
                                app.start_effect_compile(&effect_name, slot_idx);
                                let rt = editor.runtime_mut();
                                set_current_track_reactive(rt, app.tracks.len(), track);
                                rt.set_reactive("SEQ", "compiling", Value::Bool(true));
                                sync_track_mixer_state(rt, &app, &state);
                                sync_sidebar_browser(rt, &app, track);
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Adding effect '{}' to track {}",
                                    effect_name,
                                    track + 1
                                )));
                            } else {
                                editor.handle_host_event(HostEvent::Status(
                                    "No free effect slots available".to_string(),
                                ));
                            }
                        }
                    }
                    "add-builtin-effect" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(effect_name) = &*cell.borrow() {
                                    let effect_name = effect_name.clone();
                                    let track = current_track.load(Ordering::Relaxed);
                                    app.ui.cursor_track = track;
                                    match app.add_builtin_effect_sync(track, &effect_name) {
                                        Ok(slot_idx) => {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "effects",
                                                build_effects_value(
                                                    &state,
                                                    track,
                                                    &app.graph.effect_descriptors,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "step-has-plocks",
                                                build_step_has_plocks(
                                                    &state,
                                                    track,
                                                    &app.graph.effect_descriptors,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            editor.reset_widget_scroll_for_buffer_named("*fx*");
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added built-in effect '{}' to slot {}",
                                                effect_name,
                                                slot_idx + 1
                                            )));
                                        }
                                        Err(error) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error adding built-in effect: {error}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "add-builtin-effect-to-track" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(effect_name)) = (track, effect_name) {
                            if track >= app.tracks.len() {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Track {} does not exist",
                                    track + 1
                                )));
                                continue;
                            }
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.add_builtin_effect_sync(track, &effect_name) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.reset_widget_scroll_for_buffer_named("*fx*");
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Added built-in effect '{}' to track {} slot {}",
                                        effect_name,
                                        track + 1,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding built-in effect: {error}"
                                ))),
                            }
                        }
                    }
                    "add-midi-fx" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(fx_name) = &*cell.borrow() {
                                    let fx_name = fx_name.clone();
                                    let track = current_track.load(Ordering::Relaxed);
                                    match app.add_midi_fx_to_track_sync(track, &fx_name) {
                                        Ok(slot_idx) => {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "midi-effects",
                                                build_midi_effects_value(
                                                    &state,
                                                    track,
                                                    &selected_steps,
                                                ),
                                            );
                                            rt.set_reactive(
                                                "SEQ",
                                                "step-has-plocks",
                                                build_step_has_plocks(
                                                    &state,
                                                    track,
                                                    &app.graph.effect_descriptors,
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Added MIDI FX '{}' to slot {}",
                                                fx_name,
                                                slot_idx + 1
                                            )));
                                        }
                                        Err(e) => editor.handle_host_event(HostEvent::Status(
                                            format!("Error adding MIDI FX: {e}"),
                                        )),
                                    }
                                }
                            }
                        }
                    }
                    "add-midi-fx-to-track" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let fx_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(fx_name)) = (track, fx_name) {
                            if track >= app.tracks.len() {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Track {} does not exist",
                                    track + 1
                                )));
                                continue;
                            }
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.add_midi_fx_to_track_sync(track, &fx_name) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(&state, track, &selected_steps),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.reset_widget_scroll_for_buffer_named("*fx*");
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Added MIDI FX '{}' to track {} slot {}",
                                        fx_name,
                                        track + 1,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error adding MIDI FX: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-builtin-effect-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(slot), Some(effect_name)) =
                            (track, slot, effect_name)
                        {
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.insert_builtin_effect_before_slot_sync(
                                track,
                                slot,
                                &effect_name,
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted built-in effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting built-in effect: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-effect-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let effect_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(slot), Some(effect_name)) =
                            (track, slot, effect_name)
                        {
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.insert_saved_effect_before_slot_sync(
                                track,
                                slot,
                                &effect_name,
                            ) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted effect '{}' at slot {}",
                                        effect_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting effect: {error}"
                                ))),
                            }
                        }
                    }
                    "insert-midi-fx-before-slot" => {
                        let track = extract_usize_from_payload(&payload, "track");
                        let slot = extract_usize_from_payload(&payload, "slot");
                        let fx_name = extract_string_from_payload(&payload, "name");
                        if let (Some(track), Some(slot), Some(fx_name)) = (track, slot, fx_name) {
                            current_track.store(track, Ordering::Relaxed);
                            app.ui.cursor_track = track;
                            match app.insert_midi_fx_before_slot_sync(track, slot, &fx_name) {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(&state, track, &selected_steps),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_sidebar_browser(rt, &app, track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Inserted MIDI FX '{}' at slot {}",
                                        fx_name,
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error inserting MIDI FX: {error}"
                                ))),
                            }
                        }
                    }
                    "move-effect-slot" => {
                        let source_track = extract_usize_from_payload(&payload, "source-track");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let target_track = extract_usize_from_payload(&payload, "target-track");
                        let target_slot = extract_usize_from_payload(&payload, "target-slot");
                        if let (Some(source_track), Some(source_slot), Some(target_track)) =
                            (source_track, source_slot, target_track)
                        {
                            if source_track != target_track {
                                editor.handle_host_event(HostEvent::Status(
                                    "Move audio effects within the same track for now".to_string(),
                                ));
                                continue;
                            }
                            current_track.store(target_track, Ordering::Relaxed);
                            app.ui.cursor_track = target_track;
                            match app.move_effect_slot_sync(target_track, source_slot, target_slot)
                            {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), target_track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            target_track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            target_track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved effect to slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error moving effect: {error}"
                                ))),
                            }
                        }
                    }
                    "move-midi-fx-slot" => {
                        let source_track = extract_usize_from_payload(&payload, "source-track");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let target_track = extract_usize_from_payload(&payload, "target-track");
                        let target_slot = extract_usize_from_payload(&payload, "target-slot");
                        if let (Some(source_track), Some(source_slot), Some(target_track)) =
                            (source_track, source_slot, target_track)
                        {
                            if source_track != target_track {
                                editor.handle_host_event(HostEvent::Status(
                                    "Move MIDI effects within the same track for now".to_string(),
                                ));
                                continue;
                            }
                            current_track.store(target_track, Ordering::Relaxed);
                            app.ui.cursor_track = target_track;
                            match app.move_midi_fx_slot_sync(target_track, source_slot, target_slot)
                            {
                                Ok(slot_idx) => {
                                    let rt = editor.runtime_mut();
                                    set_current_track_reactive(rt, app.tracks.len(), target_track);
                                    rt.set_reactive(
                                        "SEQ",
                                        "midi-effects",
                                        build_midi_effects_value(
                                            &state,
                                            target_track,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "step-has-plocks",
                                        build_step_has_plocks(
                                            &state,
                                            target_track,
                                            &app.graph.effect_descriptors,
                                        ),
                                    );
                                    sync_track_mixer_state(rt, &app, &state);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved MIDI FX to slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error moving MIDI FX: {error}"
                                ))),
                            }
                        }
                    }
                    "move-bus-effect-slot" => {
                        let bus_idx = extract_usize_from_payload(&payload, "bus");
                        let source_slot = extract_usize_from_payload(&payload, "source-slot");
                        let target_slot = extract_usize_from_payload(&payload, "target-slot");
                        if let (Some(bus_idx), Some(source_slot)) = (bus_idx, source_slot) {
                            match app.move_bus_effect_slot_sync(bus_idx, source_slot, target_slot) {
                                Ok(slot_idx) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Moved bus effect to slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error moving bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "delete-bus-effect" => {
                        let bus_idx = match &payload {
                            Value::Map(map) => {
                                map.get("bus").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            _ => None,
                        };
                        let slot_idx = match &payload {
                            Value::Map(map) => {
                                map.get("slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            _ => None,
                        };
                        if let (Some(bus_idx), Some(slot_idx)) = (bus_idx, slot_idx) {
                            match app.delete_bus_effect_slot(bus_idx, slot_idx) {
                                Ok(()) => {
                                    app.publish_bus_gate_runtime();
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    let rt = editor.runtime_mut();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Deleted bus effect slot {}",
                                        slot_idx + 1
                                    )));
                                }
                                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting bus effect: {error}"
                                ))),
                            }
                        }
                    }
                    "delete-effect" => {
                        let slot_idx = match &payload {
                            Value::Map(map) => {
                                map.get("slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        };
                        let Some(slot_idx) = slot_idx else {
                            editor.handle_host_event(HostEvent::Status(
                                "No effect selected".to_string(),
                            ));
                            continue;
                        };
                        let track = current_track.load(Ordering::Relaxed);
                        match app
                            .graph_controller()
                            .delete_custom_effect_slot(track, slot_idx)
                        {
                            Ok(()) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "effects",
                                    build_effects_value(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                        &selected_steps,
                                    ),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "midi-effects",
                                    build_midi_effects_value(&state, track, &selected_steps),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "step-has-plocks",
                                    build_step_has_plocks(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Deleted effect slot {}",
                                    slot_idx + 1
                                )));
                            }
                            Err(e) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Error deleting effect: {e}"
                                )));
                            }
                        }
                    }
                    "delete-midi-fx" => {
                        let slot_idx = match &payload {
                            Value::Map(map) => {
                                map.get("slot").and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                            }
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        };
                        let Some(slot_idx) = slot_idx else {
                            editor.handle_host_event(HostEvent::Status(
                                "No MIDI FX selected".to_string(),
                            ));
                            continue;
                        };
                        let track = current_track.load(Ordering::Relaxed);
                        match app.delete_midi_fx_slot(track, slot_idx) {
                            Ok(()) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "midi-effects",
                                    build_midi_effects_value(&state, track, &selected_steps),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "step-has-plocks",
                                    build_step_has_plocks(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                    ),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Deleted MIDI FX slot {}",
                                    slot_idx + 1
                                )));
                            }
                            Err(e) => editor.handle_host_event(HostEvent::Status(format!(
                                "Error deleting MIDI FX: {e}"
                            ))),
                        }
                    }
                    "fork-track-pattern" => {
                        let track = match payload {
                            Value::Map(ref map) => map
                                .get("track")
                                .and_then(|cell| match &*cell.borrow() {
                                    Value::Number(n) => Some(*n as usize),
                                    _ => None,
                                })
                                .unwrap_or_else(|| current_track.load(Ordering::Relaxed)),
                            Value::Number(n) => n as usize,
                            _ => current_track.load(Ordering::Relaxed),
                        };
                        let num_tracks = app.tracks.len();
                        if track >= num_tracks {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern fork failed: track {} is out of range",
                                track + 1
                            )));
                            continue;
                        }
                        let Some(pattern_id) = app.state.fork_current_track_pattern(
                            track,
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern fork failed for track {}",
                                track + 1
                            )));
                            continue;
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Forked track {} pattern {}",
                            track + 1,
                            pattern_id.0
                        )));
                    }
                    "clone-track-pattern" => {
                        let (track, source_pattern_id) = match payload {
                            Value::Map(ref map) => (
                                map.get("track")
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) if *n >= 0.0 => Some(*n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or_else(|| current_track.load(Ordering::Relaxed)),
                                map.get("pattern-id")
                                    .or_else(|| map.get("pattern_id"))
                                    .and_then(|cell| match &*cell.borrow() {
                                        Value::Number(n) if *n >= 0.0 => Some(PatternId(*n as u64)),
                                        _ => None,
                                    }),
                            ),
                            Value::Number(n) => (n as usize, None),
                            _ => (current_track.load(Ordering::Relaxed), None),
                        };
                        let num_tracks = app.tracks.len();
                        if track >= num_tracks {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern clone failed: track {} is out of range",
                                track + 1
                            )));
                            continue;
                        }
                        let cloned = if let Some(source_id) = source_pattern_id {
                            app.state.clone_track_pattern_id_into_current_scene(
                                track,
                                source_id,
                                num_tracks,
                                &app.graph.track_buffer_ids,
                                &app.graph.track_sample_rates,
                                &app.tracks,
                                &app.graph.track_instrument_types,
                            )
                        } else {
                            app.state.clone_current_scene_track_pattern(
                                track,
                                num_tracks,
                                &app.graph.track_buffer_ids,
                                &app.graph.track_sample_rates,
                                &app.tracks,
                                &app.graph.track_instrument_types,
                            )
                        };
                        let Some(pattern_id) = cloned else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern clone failed for track {}",
                                track + 1
                            )));
                            continue;
                        };
                        let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
                        app.graph_controller().apply_sample_ids(&sample_ids);
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message = Some((
                                format!("Track pattern clone failed: {error}"),
                                Instant::now(),
                            ));
                        }
                        app.push_all_restored_defaults();
                        {
                            let mut guard = active_delete_target.lock().unwrap();
                            *guard = Some(ActiveDeleteTarget::TrackPattern { track, pattern_id });
                        }
                        active_delete_target_version.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cloned track {} pattern {}",
                            track + 1,
                            pattern_id.0
                        )));
                    }
                    "delete-track-pattern" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern delete failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let pattern_id = map
                            .get("pattern-id")
                            .or_else(|| map.get("pattern_id"))
                            .and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                                _ => None,
                            });
                        let (Some(track), Some(pattern_id)) = (track, pattern_id) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern delete failed: missing track or pattern id"
                                    .to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        if !app.state.delete_track_pattern(
                            track,
                            PatternId(pattern_id),
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern delete failed: track {}, pattern {}",
                                track + 1,
                                pattern_id
                            )));
                            continue;
                        }
                        let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
                        app.graph_controller().apply_sample_ids(&sample_ids);
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message = Some((
                                format!("Track pattern delete failed: {error}"),
                                Instant::now(),
                            ));
                        }
                        app.push_all_restored_defaults();
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Deleted track {} pattern {}",
                            track + 1,
                            pattern_id
                        )));
                    }
                    "set-scene-cell" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell share failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let pattern_id = map
                            .get("pattern-id")
                            .or_else(|| map.get("pattern_id"))
                            .and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                                _ => None,
                            });
                        let (Some(scene), Some(track), Some(pattern_id)) =
                            (scene, track, pattern_id)
                        else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell share failed: missing scene, track, or pattern id"
                                    .to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        if !app.state.set_scene_cell(
                            scene,
                            track,
                            PatternId(pattern_id),
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scene cell share failed: scene {}, track {}, pattern {}",
                                scene + 1,
                                track + 1,
                                pattern_id
                            )));
                            continue;
                        }
                        let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
                        app.graph_controller().apply_sample_ids(&sample_ids);
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message =
                                Some((format!("Scene cell share failed: {error}"), Instant::now()));
                        }
                        app.push_all_restored_defaults();
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Shared track {} pattern {} into scene {}",
                            track + 1,
                            pattern_id,
                            scene + 1
                        )));
                    }
                    "clear-scene-cell" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell clear failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let scene = map.get("scene").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let (Some(scene), Some(track)) = (scene, track) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Scene cell clear failed: missing scene or track".to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        let Some(pattern_id) = app.state.clear_scene_cell(
                            scene,
                            track,
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) else {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scene cell clear failed: scene {}, track {}",
                                scene + 1,
                                track + 1
                            )));
                            continue;
                        };
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Cleared scene {} track {} pattern {}",
                            scene + 1,
                            track + 1,
                            pattern_id.0
                        )));
                    }
                    "launch-track-pattern" => {
                        let Value::Map(ref map) = payload else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern launch failed: invalid payload".to_string(),
                            ));
                            continue;
                        };
                        let track = map.get("track").and_then(|cell| match &*cell.borrow() {
                            Value::Number(n) => Some(*n as usize),
                            _ => None,
                        });
                        let pattern_id = map
                            .get("pattern-id")
                            .or_else(|| map.get("pattern_id"))
                            .and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) if *n >= 1.0 => Some(*n as u64),
                                _ => None,
                            });
                        let (Some(track), Some(pattern_id)) = (track, pattern_id) else {
                            editor.handle_host_event(HostEvent::Status(
                                "Track pattern launch failed: missing track or pattern id"
                                    .to_string(),
                            ));
                            continue;
                        };
                        let num_tracks = app.tracks.len();
                        if track >= num_tracks {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern launch failed: track {} is out of range",
                                track + 1
                            )));
                            continue;
                        }
                        let launched = app.state.launch_track_pattern(
                            track,
                            PatternId(pattern_id),
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        );
                        if !launched {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Track pattern launch failed: pattern id {} is unavailable",
                                pattern_id
                            )));
                            continue;
                        }

                        let sample_ids = app.state.effective_pattern_sample_ids(num_tracks);
                        app.graph_controller().apply_sample_ids(&sample_ids);
                        if let Err(error) = app
                            .graph_controller()
                            .sync_track_instrument_run_modes_from_live_state()
                        {
                            app.editor.status_message = Some((
                                format!("Track pattern launch failed: {error}"),
                                Instant::now(),
                            ));
                        }
                        app.push_all_restored_defaults();

                        let ct = current_track_for_app(&mut app, &current_track).unwrap_or(track);
                        let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
                        let selected_neural_snapshot =
                            selected_neural_neurons.lock().unwrap().clone();
                        let rt = editor.runtime_mut();
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        sync_track_name_state(rt, &mut track_names, &app);
                        sync_pattern_state(rt, &state);
                        set_current_track_reactive(rt, app.tracks.len(), ct);
                        rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                        sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                        sync_step_param_lists(rt, &state, ct);
                        sync_track_mixer_state(rt, &app, &state);
                        sync_track_peak_fields(rt, &cached_track_peak_levels);
                        if fx_visible {
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
                                "midi-effects",
                                build_midi_effects_value(&state, ct, &selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                        } else {
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                        }
                        sync_track_params_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                        sync_fx_param_binding_fields_with_neural_selection(
                            rt,
                            &app,
                            &state,
                            ct,
                            &selected_steps,
                            Some(&selected_neural_snapshot),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "step-has-plocks",
                            build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                        );
                        sync_sidebar_browser(rt, &app, ct);
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        if editor_has_visible_buffer(&editor, "*mixer*") {
                            editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                        }
                        prev_pattern_epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
                        prev_track_button_states = track_button_state_snapshot(&state);
                        prev_track_playheads = track_playheads_snapshot(&state, &app);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Launched track {} pattern {}",
                            track + 1,
                            pattern_id
                        )));
                    }
                    "switch-pattern" => {
                        let profile_switch = pattern_switch_profile_enabled();
                        let profile_total_started = Instant::now();
                        if let Value::Map(ref map) = payload {
                            let idx = map.get("idx").and_then(|cell| match &*cell.borrow() {
                                Value::Number(n) => Some(*n as usize),
                                _ => None,
                            });
                            if let Some(idx) = idx {
                                let mut switch_bus_elapsed = Duration::ZERO;
                                let state_switch_elapsed;
                                let mut apply_samples_elapsed = Duration::ZERO;
                                let mut restored_defaults_elapsed = Duration::ZERO;
                                let mut sync_names_pattern_elapsed = Duration::ZERO;
                                let mut sync_current_steps_elapsed = Duration::ZERO;
                                let mut sync_sequencer_elapsed = Duration::ZERO;
                                let mut sync_step_params_elapsed = Duration::ZERO;
                                let mut sync_mixer_elapsed = Duration::ZERO;
                                let mut sync_fx_lists_elapsed = Duration::ZERO;
                                let mut sync_effects_elapsed = Duration::ZERO;
                                let mut sync_midi_effects_elapsed = Duration::ZERO;
                                let mut sync_instrument_panel_elapsed = Duration::ZERO;
                                let mut sync_accumulators_elapsed = Duration::ZERO;
                                let mut sync_track_params_elapsed = Duration::ZERO;
                                let mut sync_fx_bindings_elapsed = Duration::ZERO;
                                let mut sync_plocks_sidebar_elapsed = Duration::ZERO;
                                let mut reactive_elapsed = Duration::ZERO;
                                let mut side_effects_elapsed = Duration::ZERO;
                                let num_tracks = app.tracks.len();
                                let current_pattern = app.state.current_scene_index();
                                let num_patterns = app.state.scene_count();
                                if idx != current_pattern && idx < num_patterns {
                                    let started = Instant::now();
                                    app.switch_bus_pattern(idx);
                                    switch_bus_elapsed = started.elapsed();
                                }
                                let started = Instant::now();
                                let switched = app.state.switch_pattern(
                                    idx,
                                    num_tracks,
                                    &app.graph.track_buffer_ids,
                                    &app.graph.track_sample_rates,
                                    &app.tracks,
                                    &app.graph.track_instrument_types,
                                );
                                state_switch_elapsed = started.elapsed();
                                let pattern_changed = switched.is_some();
                                if let Some(sample_ids) = switched {
                                    let started = Instant::now();
                                    app.graph_controller().apply_sample_ids(&sample_ids);
                                    if let Err(error) = app
                                        .graph_controller()
                                        .sync_track_instrument_run_modes_from_live_state()
                                    {
                                        app.editor.status_message = Some((
                                            format!("Pattern switch failed: {error}"),
                                            Instant::now(),
                                        ));
                                    }
                                    app.graph_controller().sync_current_pattern_mod_routes();
                                    apply_samples_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    app.push_all_restored_defaults();
                                    restored_defaults_elapsed = started.elapsed();
                                    let ct = current_track.load(Ordering::Relaxed);
                                    let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
                                    let rt = editor.runtime_mut();
                                    let started = Instant::now();
                                    sync_shared_track_collapsed(&track_collapsed, &app);
                                    sync_track_name_state(rt, &mut track_names, &app);
                                    sync_pattern_state(rt, &state);
                                    sync_names_pattern_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                                    sync_current_steps_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    sync_all_track_sequencer_state(
                                        rt,
                                        &state,
                                        &app,
                                        ct,
                                        &selected_steps,
                                    );
                                    sync_sequencer_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    sync_step_param_lists(rt, &state, ct);
                                    sync_step_params_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    sync_track_mixer_state(rt, &app, &state);
                                    sync_bus_mixer_state(rt, &app);
                                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                                    sync_mixer_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    if fx_visible {
                                        let sub_started = Instant::now();
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
                                        sync_effects_elapsed = sub_started.elapsed();

                                        let sub_started = Instant::now();
                                        rt.set_reactive(
                                            "SEQ",
                                            "midi-effects",
                                            build_midi_effects_value(&state, ct, &selected_steps),
                                        );
                                        sync_midi_effects_elapsed = sub_started.elapsed();

                                        let sub_started = Instant::now();
                                        rt.set_reactive(
                                            "SEQ",
                                            "instrument-panel",
                                            build_instrument_panel_value(&app, ct, &selected_steps),
                                        );
                                        sync_instrument_panel_elapsed = sub_started.elapsed();

                                        let sub_started = Instant::now();
                                        *accumulator_names.lock().unwrap() =
                                            build_accumulator_names(&app);
                                        sync_accumulators_elapsed = sub_started.elapsed();
                                    } else {
                                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    }
                                    sync_fx_lists_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    let selected_neural_snapshot =
                                        selected_neural_neurons.lock().unwrap().clone();
                                    sync_track_params_with_neural_selection(
                                        rt,
                                        &app,
                                        &state,
                                        ct,
                                        &selected_steps,
                                        Some(&selected_neural_snapshot),
                                    );
                                    sync_track_params_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    sync_fx_param_binding_fields_with_neural_selection(
                                        rt,
                                        &app,
                                        &state,
                                        ct,
                                        &selected_steps,
                                        Some(&selected_neural_snapshot),
                                    );
                                    sync_fx_bindings_elapsed = started.elapsed();
                                    let started = Instant::now();
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
                                    sync_plocks_sidebar_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    rt.run_reactive_cycle();
                                    reactive_elapsed = started.elapsed();
                                    let started = Instant::now();
                                    editor.refresh_runtime_side_effects();
                                    side_effects_elapsed = started.elapsed();
                                    if editor_has_visible_buffer(&editor, "*mixer*") {
                                        editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                                    }
                                    prev_pattern_epoch =
                                        state.transport.pattern_epoch.load(Ordering::Relaxed);
                                    prev_track_button_states = track_button_state_snapshot(&state);
                                    prev_track_playheads = track_playheads_snapshot(&state, &app);
                                }
                                if profile_switch {
                                    eprintln!(
                                        "[pattern-switch-profile][host] idx={} changed={} total={:.2}ms switch_bus={:.2}ms state_switch={:.2}ms apply_samples={:.2}ms defaults={:.2}ms names_pattern={:.2}ms current_steps={:.2}ms sequencer_bindings={:.2}ms step_params={:.2}ms mixer={:.2}ms fx_lists={:.2}ms effects={:.2}ms midi_effects={:.2}ms instrument_panel={:.2}ms accumulators={:.2}ms track_params={:.2}ms fx_bindings={:.2}ms plocks_sidebar={:.2}ms reactive={:.2}ms side_effects={:.2}ms",
                                        idx,
                                        pattern_changed,
                                        duration_ms(profile_total_started.elapsed()),
                                        duration_ms(switch_bus_elapsed),
                                        duration_ms(state_switch_elapsed),
                                        duration_ms(apply_samples_elapsed),
                                        duration_ms(restored_defaults_elapsed),
                                        duration_ms(sync_names_pattern_elapsed),
                                        duration_ms(sync_current_steps_elapsed),
                                        duration_ms(sync_sequencer_elapsed),
                                        duration_ms(sync_step_params_elapsed),
                                        duration_ms(sync_mixer_elapsed),
                                        duration_ms(sync_fx_lists_elapsed),
                                        duration_ms(sync_effects_elapsed),
                                        duration_ms(sync_midi_effects_elapsed),
                                        duration_ms(sync_instrument_panel_elapsed),
                                        duration_ms(sync_accumulators_elapsed),
                                        duration_ms(sync_track_params_elapsed),
                                        duration_ms(sync_fx_bindings_elapsed),
                                        duration_ms(sync_plocks_sidebar_elapsed),
                                        duration_ms(reactive_elapsed),
                                        duration_ms(side_effects_elapsed),
                                    );
                                }
                            }
                        }
                    }
                    "propagate-current-track-to-all-patterns" => {
                        let track = match payload {
                            Value::Number(n) => n as usize,
                            _ => current_track.load(Ordering::Relaxed),
                        };
                        let num_patterns = state.scene_count();
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
                            &app.graph.track_sample_rates,
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
                        app.save_current_bus_pattern();
                        let source_pattern = app.state.current_scene_index();
                        let new_idx = app.state.clone_pattern(
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        );
                        app.graph_controller().sync_current_pattern_mod_routes();
                        app.clone_bus_pattern_from_to(source_pattern, new_idx);
                        let rt = editor.runtime_mut();
                        sync_pattern_state(rt, &state);
                        sync_bus_mixer_state(rt, &app);
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
                        app.save_current_bus_pattern();
                        let deleted_pattern = app.state.current_scene_index();
                        if let Some(sample_ids) = app.state.delete_pattern(
                            num_tracks,
                            &app.graph.track_buffer_ids,
                            &app.graph.track_sample_rates,
                            &app.tracks,
                            &app.graph.track_instrument_types,
                        ) {
                            app.graph_controller().apply_sample_ids(&sample_ids);
                            app.graph_controller().sync_current_pattern_mod_routes();
                            app.push_all_restored_defaults();
                            let new_pattern = app.state.current_scene_index();
                            app.delete_bus_pattern_at(deleted_pattern, new_pattern);
                            let ct = current_track.load(Ordering::Relaxed);
                            let rt = editor.runtime_mut();
                            sync_shared_track_collapsed(&track_collapsed, &app);
                            sync_track_name_state(rt, &mut track_names, &app);
                            sync_pattern_state(rt, &state);
                            rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                            sync_step_param_lists(rt, &state, ct);
                            sync_track_mixer_state(rt, &app, &state);
                            sync_bus_mixer_state(rt, &app);
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                                "midi-effects",
                                build_midi_effects_value(&state, ct, &selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            let selected_neural_snapshot =
                                selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                ct,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                ct,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
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
                    "agent-accept" => {
                        let conv_id = match payload {
                            Value::Number(id) if id >= 1.0 => id as sequencer::agent::store::ConvId,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-accept: expected conversation id".to_string(),
                                ));
                                continue;
                            }
                        };
                        let snapshot = app.agent_store.snapshot(conv_id);
                        let apply_as_effect =
                            match snapshot.as_ref().map(|snapshot| &snapshot.state) {
                                Some(state) => match state.kind {
                                    sequencer::agent::store::AgentKind::Effect => true,
                                    sequencer::agent::store::AgentKind::Instrument => false,
                                    sequencer::agent::store::AgentKind::General => {
                                        state.effect_draft.is_some()
                                            || state.accepted_effect_target.is_some()
                                    }
                                },
                                None => false,
                            };
                        if !apply_as_effect {
                            match apply_agent_draft_to_owned_instrument(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                            ) {
                                Ok(result) => {
                                    let verb = if result.created_track {
                                        "Accepted agent draft as track"
                                    } else {
                                        "Updated agent draft on track"
                                    };
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "{verb} {}",
                                        result.track_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        } else {
                            match apply_agent_draft_to_effect_slot(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                            ) {
                                Ok(result) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Accepted agent effect as track {} slot {}",
                                        result.track_index + 1,
                                        result.slot_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        }
                    }
                    "agent-send" => {
                        let Value::Map(map) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "agent-send: expected payload map".to_string(),
                            ));
                            continue;
                        };
                        let conv_id = match map.get("id").map(|cell| cell.borrow().clone()) {
                            Some(Value::Number(id)) if id >= 1.0 => {
                                id as sequencer::agent::store::ConvId
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-send: expected conversation id".to_string(),
                                ));
                                continue;
                            }
                        };
                        let prompt = match map.get("prompt").map(|cell| cell.borrow().clone()) {
                            Some(Value::String(prompt)) => prompt,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-send: expected prompt string".to_string(),
                                ));
                                continue;
                            }
                        };

                        let needs_stub = app
                            .agent_store
                            .snapshot(conv_id)
                            .map(|snapshot| {
                                let state = snapshot.state;
                                state.kind == sequencer::agent::store::AgentKind::Instrument
                                    && state.draft.is_none()
                                    && state.stub_instrument_target.is_none()
                                    && state.accepted_instrument_target.is_none()
                                    && state.finalized_instrument_name.is_none()
                            })
                            .unwrap_or(false);
                        if needs_stub {
                            match ensure_agent_instrument_stub_track(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                            ) {
                                Ok(track_index) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Created working instrument track {}",
                                        track_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                    continue;
                                }
                            }
                        }

                        let session_context = metal_agent_session_context(
                            &app,
                            &current_track,
                            &active_delete_target,
                        );
                        if let Err(error) =
                            app.agent_store
                                .send_with_context(conv_id, prompt, session_context)
                        {
                            editor.handle_host_event(HostEvent::Error(error));
                        }
                    }
                    "agent-ensure-instrument-stub" => {
                        let conv_id = match payload {
                            Value::Number(id) if id >= 1.0 => id as sequencer::agent::store::ConvId,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-ensure-instrument-stub: expected conversation id"
                                        .to_string(),
                                ));
                                continue;
                            }
                        };
                        match ensure_agent_instrument_stub_track(
                            &mut app,
                            &mut editor,
                            &state,
                            &current_track,
                            &mut track_names,
                            &track_pan_ids,
                            &record_armed,
                            &selected_steps,
                            &accumulator_names,
                            &cached_track_peak_levels,
                            &cached_bus_peak_levels,
                            &ui_epoch,
                            lg_raw,
                            conv_id,
                        ) {
                            Ok(track_index) => {
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Created working instrument track {}",
                                    track_index + 1
                                )));
                            }
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                            }
                        }
                    }
                    "agent-finalize" => {
                        let Value::Map(map) = payload else {
                            editor.handle_host_event(HostEvent::Error(
                                "agent-finalize: expected payload map".to_string(),
                            ));
                            continue;
                        };
                        let conv_id = match map.get("id").map(|cell| cell.borrow().clone()) {
                            Some(Value::Number(id)) if id >= 1.0 => {
                                id as sequencer::agent::store::ConvId
                            }
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-finalize: expected conversation id".to_string(),
                                ));
                                continue;
                            }
                        };
                        let requested_name = match map.get("name").map(|cell| cell.borrow().clone())
                        {
                            Some(Value::String(name)) if !name.trim().is_empty() => name,
                            _ => {
                                editor.handle_host_event(HostEvent::Error(
                                    "agent-finalize: expected non-empty artifact name".to_string(),
                                ));
                                continue;
                            }
                        };
                        let snapshot = app.agent_store.snapshot(conv_id);
                        let finalize_as_effect =
                            match snapshot.as_ref().map(|snapshot| &snapshot.state) {
                                Some(state) => match state.kind {
                                    sequencer::agent::store::AgentKind::Effect => true,
                                    sequencer::agent::store::AgentKind::Instrument => false,
                                    sequencer::agent::store::AgentKind::General => {
                                        state.effect_draft.is_some()
                                            || state.accepted_effect_target.is_some()
                                    }
                                },
                                None => false,
                            };
                        if !finalize_as_effect {
                            match finalize_agent_instrument(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                                &requested_name,
                            ) {
                                Ok(result) => {
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Saved agent artifact {} as track {}",
                                        display_instrument_name(&result.instrument_name),
                                        result.track_index + 1
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        } else {
                            match finalize_agent_effect(
                                &mut app,
                                &mut editor,
                                &state,
                                &current_track,
                                &mut track_names,
                                &track_pan_ids,
                                &record_armed,
                                &selected_steps,
                                &accumulator_names,
                                &cached_track_peak_levels,
                                &cached_bus_peak_levels,
                                &ui_epoch,
                                lg_raw,
                                conv_id,
                                &requested_name,
                            ) {
                                Ok(result) => {
                                    let target = match (result.track_index, result.slot_index) {
                                        (Some(track), Some(slot)) => {
                                            format!(" on track {} slot {}", track + 1, slot + 1)
                                        }
                                        _ => String::new(),
                                    };
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Saved agent effect artifact {}{}",
                                        display_instrument_name(&result.effect_name),
                                        target
                                    )));
                                }
                                Err(error) => {
                                    editor.handle_host_event(HostEvent::Error(error));
                                }
                            }
                        }
                    }
                    // ── Inline instrument/effect editor commands ──
                    "enter-new-instrument-editor" => {
                        if editor_mode.is_some() || instrument_edit_session.is_some() {
                            editor.handle_host_event(HostEvent::Error(
                                "Close the current editor before creating a new instrument"
                                    .to_string(),
                            ));
                            continue;
                        }
                        let original_track = current_track.load(Ordering::Relaxed);
                        let temp_dir = match create_new_instrument_draft_dir() {
                            Ok(dir) => dir,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                                continue;
                            }
                        };
                        let file_path = temp_dir.join("dsp.lisp");
                        if let Err(error) = std::fs::write(&file_path, NEW_INSTRUMENT_STARTER_DSP) {
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write starter instrument: {error}"
                            )));
                            continue;
                        }

                        let draft_track = match app.add_transient_instrument_track_sync(
                            NEW_INSTRUMENT_DRAFT_NAME,
                            NEW_INSTRUMENT_STARTER_DSP,
                            Some(&temp_dir),
                        ) {
                            Ok(track) => track,
                            Err(error) => {
                                let _ = std::fs::remove_dir_all(&temp_dir);
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to create draft instrument track: {error}"
                                )));
                                continue;
                            }
                        };
                        let _ = app.force_instrument_enabled(draft_track);
                        sync_after_agent_instrument_apply(
                            &mut app,
                            &mut editor,
                            &state,
                            draft_track,
                            &current_track,
                            &mut track_names,
                            &track_pan_ids,
                            &record_armed,
                            &selected_steps,
                            &accumulator_names,
                            &cached_track_peak_levels,
                            &cached_bus_peak_levels,
                            &ui_epoch,
                            lg_raw,
                        );

                        let Some(engine_id) = app
                            .graph
                            .track_engine_ids
                            .get(draft_track)
                            .and_then(|id| *id)
                        else {
                            let _ = app.graph_controller().delete_track(draft_track);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(
                                "Draft instrument track has no engine binding".to_string(),
                            ));
                            continue;
                        };

                        let buf_name = "*instrument-patcher:new-instrument*".to_string();
                        editor.remove_buffer_by_name(&buf_name);
                        editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                        let patcher_source =
                            instrument_patcher_buffer_source(&buf_name, &file_path);
                        if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                            let _ = app.graph_controller().delete_track(draft_track);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to build patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        reset_instrument_patcher_state(&file_path);
                        let layout_source = show_instrument_patcher_layout_source(&buf_name);
                        if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                            let _ = app.graph_controller().delete_track(draft_track);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        editor_buffer_name = Some(buf_name.clone());
                        editor_mode = Some("new-instrument".to_string());
                        instrument_edit_session = Some(InstrumentEditSession::begin_create_draft(
                            NEW_INSTRUMENT_DRAFT_NAME.to_string(),
                            file_path,
                            buf_name.clone(),
                            engine_id,
                            NEW_INSTRUMENT_STARTER_DSP.to_string(),
                            temp_dir,
                            draft_track,
                            original_track,
                        ));
                        let rt = editor.runtime_mut();
                        let _ = rt.eval_str("(set! sbrowser-editor-name \"\")");
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String("new-instrument".to_string()),
                        );
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(buf_name.clone()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "editor-instrument-run-mode",
                            Value::String("instrument".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Created draft instrument track {}",
                            draft_track + 1
                        )));
                    }

                    "set-draft-instrument-run-mode" => {
                        let Some(session) = instrument_edit_session.as_mut() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No instrument edit session is active".to_string(),
                            ));
                            continue;
                        };
                        if !matches!(&session.mode, InstrumentEditMode::CreateDraft { .. }) {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Run mode can only be changed for draft instruments"
                                        .to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let requested = extract_string_from_payload(&payload, "run-mode")
                            .unwrap_or_else(|| "instrument".to_string());
                        let Some(run_mode) = instrument_run_mode_from_label(&requested) else {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Unknown instrument run mode '{requested}'")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        };
                        match app
                            .graph_controller()
                            .set_track_instrument_run_mode(session.track, run_mode)
                        {
                            Ok(()) => {
                                session.run_mode = run_mode;
                                if let Some(engine_id) = app.graph.track_engine_ids[session.track] {
                                    session.engine_id = engine_id;
                                }
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-instrument-run-mode",
                                    Value::String(instrument_run_mode_label(run_mode).to_string()),
                                );
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(String::new()),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Draft instrument mode: {}",
                                    match run_mode {
                                        CustomInstrumentRunMode::Instrument => "Instrument",
                                        CustomInstrumentRunMode::FreePatch => "Free Patch",
                                    }
                                )));
                            }
                            Err(error) => {
                                let rt = editor.runtime_mut();
                                rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                            }
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
                                    let Some(session) = instrument_edit_session.as_ref() else {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "No draft instrument session is active".to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    };
                                    if !matches!(
                                        &session.mode,
                                        InstrumentEditMode::CreateDraft { .. }
                                    ) {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Current editor session is not a draft instrument"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if !session.visible_revision_valid {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Cannot finalize: the current patch has errors"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let final_slug =
                                        sequencer::agent::actions::normalize_patch_name(
                                            &inst_name,
                                            "new-instrument",
                                        );
                                    let final_name = format!("{final_slug}/");
                                    let (final_dir, legacy_file) =
                                        finalized_instrument_storage_paths(&final_slug);
                                    if final_dir.exists() || legacy_file.exists() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Instrument '{final_slug}' already exists"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let source = session.last_valid_source.clone();
                                    let draft_track = match &session.mode {
                                        InstrumentEditMode::CreateDraft { draft_track, .. } => {
                                            *draft_track
                                        }
                                        InstrumentEditMode::EditExisting { .. } => unreachable!(),
                                    };
                                    if let Err(error) = sequencer::lisp_effect::save_instrument(
                                        &final_name,
                                        &source,
                                    ) {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to save finalized instrument: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if let Err(error) =
                                        sequencer::lisp_effect::save_instrument_run_mode(
                                            &final_name,
                                            session.run_mode,
                                        )
                                    {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to save finalized instrument mode: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let target_dsp = final_dir.join("dsp.lisp");
                                    if let Some(layout) = session.last_valid_layout.as_deref() {
                                        if let Err(error) =
                                            write_patcher_layout_sidecar(&target_dsp, layout)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save finalized instrument layout: {error}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    } else if let InstrumentEditMode::CreateDraft {
                                        temp_dir, ..
                                    } = &session.mode
                                    {
                                        let source_dsp = temp_dir.join("dsp.lisp");
                                        if let Err(error) =
                                            copy_patcher_layout_sidecar(&source_dsp, &target_dsp)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save finalized instrument layout: {error}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    }
                                    if let Err(error) = app.replace_custom_instrument_track_sync(
                                        draft_track,
                                        &final_name,
                                        &source,
                                    ) {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to load finalized instrument: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if let Err(error) =
                                        app.graph_controller().set_track_instrument_run_mode(
                                            draft_track,
                                            session.run_mode,
                                        )
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to apply finalized instrument mode: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let session =
                                        instrument_edit_session.take().expect("session checked");
                                    if let InstrumentEditMode::CreateDraft { temp_dir, .. } =
                                        session.mode
                                    {
                                        let _ = std::fs::remove_dir_all(temp_dir);
                                    }
                                    reset_instrument_patcher_state(&session.path);
                                    let buf_name = session.buffer_name;
                                    if let Err(error) = editor
                                        .runtime_mut()
                                        .eval_str(restore_instrument_patcher_layout_source())
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to restore main editor layout: {error:?}"
                                        )));
                                    }
                                    editor.refresh_runtime_side_effects();
                                    editor.remove_buffer_by_name(&buf_name);
                                    editor_buffer_name = None;
                                    editor_mode = None;
                                    current_track.store(draft_track, Ordering::Relaxed);
                                    app.ui.cursor_track = draft_track;
                                    track_names = app.tracks.clone();
                                    sync_shared_track_collapsed(&track_collapsed, &app);

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
                                        "editor-instrument-run-mode",
                                        Value::String("instrument".to_string()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-buffer-name",
                                        Value::String(String::new()),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "track-names",
                                        build_track_names(&app.tracks),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "instrument-panel",
                                        build_instrument_panel_value(
                                            &app,
                                            draft_track,
                                            &selected_steps,
                                        ),
                                    );
                                    sync_sidebar_browser(rt, &app, draft_track);
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Finalized instrument '{}' on track {}",
                                        display_instrument_name(&final_name),
                                        draft_track + 1
                                    )));
                                }
                            }
                        }
                    }

                    "enter-edit-instrument" => {
                        if let Value::Map(ref map) = payload {
                            if let Some(cell) = map.get("name") {
                                if let Value::String(inst_name) = &*cell.borrow() {
                                    let inst_name = inst_name.clone();
                                    let file_path =
                                        match sequencer::lisp_effect::instrument_source_path(
                                            &inst_name,
                                        ) {
                                            Ok(path) => path,
                                            Err(e) => {
                                                editor.handle_host_event(HostEvent::Error(
                                                    format!("Instrument file not found: {e}"),
                                                ));
                                                continue;
                                            }
                                        };
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Instrument file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let track = current_track.load(Ordering::Relaxed);
                                    let Some(engine_id) =
                                        app.graph.track_engine_ids.get(track).and_then(|id| *id)
                                    else {
                                        editor.handle_host_event(HostEvent::Error(
                                            "Current instrument track has no engine binding"
                                                .to_string(),
                                        ));
                                        continue;
                                    };
                                    let persisted_source = match std::fs::read_to_string(&file_path)
                                    {
                                        Ok(source) => source,
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to read '{}': {error}",
                                                file_path.display()
                                            )));
                                            continue;
                                        }
                                    };
                                    let run_mode =
                                        match sequencer::lisp_effect::load_instrument_run_mode(
                                            &inst_name,
                                        ) {
                                            Ok(run_mode) => run_mode,
                                            Err(error) => {
                                                editor.handle_host_event(HostEvent::Error(
                                                    format!(
                                                        "Failed to load instrument mode: {error}"
                                                    ),
                                                ));
                                                continue;
                                            }
                                        };
                                    let buf_name = format!("*instrument-patcher:{inst_name}*");
                                    editor.remove_buffer_by_name(&buf_name);
                                    editor.create_scratch_buffer(
                                        &buf_name,
                                        "",
                                        BufferMode::ESeqLisp,
                                    );
                                    let patcher_source =
                                        instrument_patcher_buffer_source(&buf_name, &file_path);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&patcher_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to build patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    reset_instrument_patcher_state(&file_path);
                                    let layout_source =
                                        show_instrument_patcher_layout_source(&buf_name);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&layout_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to show patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    editor.refresh_runtime_side_effects();
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-instrument".to_string());
                                    instrument_edit_session =
                                        Some(InstrumentEditSession::begin_edit_existing(
                                            inst_name,
                                            file_path,
                                            buf_name.clone(),
                                            engine_id,
                                            track,
                                            persisted_source,
                                            run_mode,
                                        ));
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
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-instrument-run-mode",
                                        Value::String(
                                            instrument_run_mode_label(run_mode).to_string(),
                                        ),
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
                                    if let Some(session) = instrument_edit_session.as_ref() {
                                        if !session.visible_revision_valid {
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(
                                                    "Cannot save: the current patch has errors"
                                                        .to_string(),
                                                ),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                        if let Err(e) = std::fs::write(
                                            &session.path,
                                            &session.last_valid_source,
                                        ) {
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
                                        if let Some(layout) = session.last_valid_layout.as_deref() {
                                            if let Err(e) =
                                                write_patcher_layout_sidecar(&session.path, layout)
                                            {
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "editor-error",
                                                    Value::String(format!(
                                                        "Failed to save layout: {e}"
                                                    )),
                                                );
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                continue;
                                            }
                                        }

                                        let buf_name = session.buffer_name.clone();
                                        reset_instrument_patcher_state(&session.path);
                                        if let Err(error) = editor
                                            .runtime_mut()
                                            .eval_str(restore_instrument_patcher_layout_source())
                                        {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to restore main editor layout: {error:?}"
                                            )));
                                            continue;
                                        }
                                        editor.refresh_runtime_side_effects();
                                        editor.remove_buffer_by_name(&buf_name);
                                        editor_buffer_name = None;
                                        editor_mode = None;
                                        instrument_edit_session = None;

                                        let ct = current_track.load(Ordering::Relaxed);
                                        track_names[ct] = inst_name.clone();
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
                                            "editor-instrument-run-mode",
                                            Value::String("instrument".to_string()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "track-names",
                                            build_track_names(&track_names),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Saved instrument '{inst_name}'"
                                        )));
                                        continue;
                                    }
                                    let buf_name = editor_buffer_name.clone().unwrap_or_default();
                                    let source =
                                        editor.read_buffer_text(&buf_name).unwrap_or_default();

                                    if let Err(e) =
                                        sequencer::lisp_effect::save_instrument(&inst_name, &source)
                                    {
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
                                                "editor-instrument-run-mode",
                                                Value::String("instrument".to_string()),
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
                                            rt.set_reactive(
                                                "SEQ",
                                                "midi-effects",
                                                build_midi_effects_value(
                                                    &state,
                                                    ct,
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

                    "preview-instrument-patch" => {
                        let Some(session) = instrument_edit_session.as_mut() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No instrument edit session is active".to_string(),
                            ));
                            continue;
                        };
                        let status = extract_string_from_payload(&payload, "status")
                            .unwrap_or_else(|| "invalid".to_string());
                        if status == "agentic-submit" {
                            let Some(path) = extract_string_from_payload(&payload, "path") else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing patch path".to_string(),
                                ));
                                continue;
                            };
                            let Some(bubble_id) =
                                extract_string_from_payload(&payload, "bubble-id")
                            else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing bubble id".to_string(),
                                ));
                                continue;
                            };
                            let generation = extract_usize_from_payload(&payload, "generation")
                                .unwrap_or(0) as u64;
                            let prompt =
                                extract_string_from_payload(&payload, "prompt").unwrap_or_default();
                            let macro_name = extract_string_from_payload(&payload, "macro-name")
                                .unwrap_or_else(|| "agentic-macro".to_string());
                            let target = extract_string_from_payload(&payload, "target")
                                .unwrap_or_else(|| "create-macro".to_string());
                            let intent = match extract_string_from_payload(&payload, "intent")
                                .as_deref()
                            {
                                Some("effect") => {
                                    eseqlisp::widget_render::patcher::PatcherIntent::Effect
                                }
                                _ => eseqlisp::widget_render::patcher::PatcherIntent::Instrument,
                            };
                            let task_key = format!("{path}::{bubble_id}");
                            eprintln!(
                                "[agentic-bubble] host submit key={} generation={} intent={:?} macro={} prompt={:?}",
                                task_key, generation, intent, macro_name, prompt
                            );
                            let (tx, rx) = std::sync::mpsc::channel();
                            let follow_up = if target == "edit-macro" {
                                let existing_macro_name =
                                    extract_string_from_payload(&payload, "existing-macro-name")
                                        .unwrap_or_else(|| macro_name.clone());
                                let params =
                                    extract_string_from_payload(&payload, "existing-macro-params")
                                        .unwrap_or_default()
                                        .split_whitespace()
                                        .map(str::to_string)
                                        .collect::<Vec<_>>();
                                let source =
                                    extract_string_from_payload(&payload, "existing-macro-source")
                                        .unwrap_or_default();
                                Some(sequencer::agent::agentic_bubble::AgenticBubbleFollowUp {
                                    macro_name: existing_macro_name,
                                    params,
                                    source,
                                })
                            } else {
                                None
                            };
                            let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                                prompt,
                                suggested_macro_name: macro_name.clone(),
                                follow_up,
                            };
                            std::thread::spawn(move || {
                                let result =
                                    sequencer::agent::agentic_bubble::generate_agentic_bubble_macro(
                                        request,
                                    );
                                let _ = tx.send(result);
                            });
                            pending_agentic_bubbles.insert(
                                task_key,
                                PendingAgenticBubble {
                                    path: PathBuf::from(path),
                                    intent,
                                    bubble_id,
                                    generation,
                                    receiver: rx,
                                },
                            );
                            editor.handle_host_event(HostEvent::Status(
                                "Agentic bubble working...".to_string(),
                            ));
                            continue;
                        }
                        if status == "layout" {
                            if let Some(layout) = extract_string_from_payload(&payload, "layout") {
                                session.last_valid_layout = Some(layout);
                                if let Some(pending) = pending_instrument_preview.as_mut() {
                                    pending.layout = session.last_valid_layout.clone();
                                }
                            }
                            continue;
                        }
                        if status != "valid" {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_instrument_preview = None;
                            let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                                .unwrap_or_else(|| "Patch writeback failed".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-error", Value::String(diagnostic));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let Some(source) = extract_string_from_payload(&payload, "source") else {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_instrument_preview = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Patch preview did not include emitted source".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        };

                        let layout = extract_string_from_payload(&payload, "layout");
                        session.preview_generation = session.preview_generation.wrapping_add(1);
                        session.visible_revision_valid = false;
                        let generation = session.preview_generation;
                        let sample_rate = app.graph.sample_rate;
                        let asset_base = session.path.parent().map(|parent| parent.to_path_buf());
                        let compile_source = source.clone();
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result =
                                sequencer::lisp_effect::compile_and_load_instrument_with_asset_base(
                                    &compile_source,
                                    sample_rate,
                                    asset_base.as_deref(),
                                );
                            let _ = tx.send(result);
                        });
                        pending_instrument_preview = Some(PendingInstrumentPreview {
                            generation,
                            source,
                            layout,
                            receiver: rx,
                        });
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Preview compiling...".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }

                    "preview-effect-patch" => {
                        let Some(session) = effect_edit_session.as_mut() else {
                            editor.handle_host_event(HostEvent::Status(
                                "No effect edit session is active".to_string(),
                            ));
                            continue;
                        };
                        let status = extract_string_from_payload(&payload, "status")
                            .unwrap_or_else(|| "invalid".to_string());
                        if status == "agentic-submit" {
                            let Some(path) = extract_string_from_payload(&payload, "path") else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing patch path".to_string(),
                                ));
                                continue;
                            };
                            let Some(bubble_id) =
                                extract_string_from_payload(&payload, "bubble-id")
                            else {
                                editor.handle_host_event(HostEvent::Status(
                                    "Agentic bubble request missing bubble id".to_string(),
                                ));
                                continue;
                            };
                            let generation = extract_usize_from_payload(&payload, "generation")
                                .unwrap_or(0) as u64;
                            let prompt =
                                extract_string_from_payload(&payload, "prompt").unwrap_or_default();
                            let macro_name = extract_string_from_payload(&payload, "macro-name")
                                .unwrap_or_else(|| "agentic-macro".to_string());
                            let target = extract_string_from_payload(&payload, "target")
                                .unwrap_or_else(|| "create-macro".to_string());
                            let task_key = format!("{path}::{bubble_id}");
                            let (tx, rx) = std::sync::mpsc::channel();
                            let follow_up = if target == "edit-macro" {
                                let existing_macro_name =
                                    extract_string_from_payload(&payload, "existing-macro-name")
                                        .unwrap_or_else(|| macro_name.clone());
                                let params =
                                    extract_string_from_payload(&payload, "existing-macro-params")
                                        .unwrap_or_default()
                                        .split_whitespace()
                                        .map(str::to_string)
                                        .collect::<Vec<_>>();
                                let source =
                                    extract_string_from_payload(&payload, "existing-macro-source")
                                        .unwrap_or_default();
                                Some(sequencer::agent::agentic_bubble::AgenticBubbleFollowUp {
                                    macro_name: existing_macro_name,
                                    params,
                                    source,
                                })
                            } else {
                                None
                            };
                            let request = sequencer::agent::agentic_bubble::AgenticBubbleRequest {
                                prompt,
                                suggested_macro_name: macro_name,
                                follow_up,
                            };
                            std::thread::spawn(move || {
                                let result =
                                    sequencer::agent::agentic_bubble::generate_agentic_bubble_macro(
                                        request,
                                    );
                                let _ = tx.send(result);
                            });
                            pending_agentic_bubbles.insert(
                                task_key,
                                PendingAgenticBubble {
                                    path: PathBuf::from(path),
                                    intent: eseqlisp::widget_render::patcher::PatcherIntent::Effect,
                                    bubble_id,
                                    generation,
                                    receiver: rx,
                                },
                            );
                            editor.handle_host_event(HostEvent::Status(
                                "Agentic bubble working...".to_string(),
                            ));
                            continue;
                        }
                        if status == "layout" {
                            if let Some(layout) = extract_string_from_payload(&payload, "layout") {
                                session.last_valid_layout = Some(layout);
                                if let Some(pending) = pending_effect_preview.as_mut() {
                                    pending.layout = session.last_valid_layout.clone();
                                }
                            }
                            continue;
                        }
                        if status != "valid" {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_effect_preview = None;
                            let diagnostic = extract_string_from_payload(&payload, "diagnostic")
                                .unwrap_or_else(|| "Patch writeback failed".to_string());
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-error", Value::String(diagnostic));
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        let Some(source) = extract_string_from_payload(&payload, "source") else {
                            session.preview_generation = session.preview_generation.wrapping_add(1);
                            session.visible_revision_valid = false;
                            pending_effect_preview = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Patch preview did not include emitted source".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        };

                        let layout = extract_string_from_payload(&payload, "layout");
                        session.preview_generation = session.preview_generation.wrapping_add(1);
                        session.visible_revision_valid = false;
                        let generation = session.preview_generation;
                        let sample_rate = app.graph.sample_rate;
                        let asset_base = session.path.parent().map(|parent| parent.to_path_buf());
                        let compile_source = source.clone();
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let result = sequencer::lisp_effect::compile_and_load_with_asset_base(
                                &compile_source,
                                sample_rate,
                                asset_base.as_deref(),
                            );
                            let _ = tx.send(result);
                        });
                        pending_effect_preview = Some(PendingEffectPreview {
                            generation,
                            source,
                            layout,
                            receiver: rx,
                        });
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Preview compiling...".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }

                    "toggle-instrument-patcher-source" => {
                        let (buffer_name, path, last_valid_source) =
                            if let Some(session) = instrument_edit_session.as_ref() {
                                (
                                    session.buffer_name.clone(),
                                    session.path.clone(),
                                    session.last_valid_source.clone(),
                                )
                            } else if let Some(session) = effect_edit_session.as_ref() {
                                (
                                    session.buffer_name.clone(),
                                    session.path.clone(),
                                    session.last_valid_source.clone(),
                                )
                            } else {
                                editor.handle_host_event(HostEvent::Status(
                                    "No patch edit session is active".to_string(),
                                ));
                                continue;
                            };
                        if !path.exists() {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Patch source no longer exists: {}",
                                path.display()
                            )));
                            continue;
                        }
                        let source_buffer_name =
                            eseqlisp::widget_render::patcher::emitted_source_buffer_name(
                                &path.to_string_lossy(),
                            );
                        let layout_source =
                            if editor_has_visible_buffer(&editor, &source_buffer_name) {
                                show_instrument_patcher_layout_source(&buffer_name)
                            } else {
                                let source_buffer_name = match editor
                                    .upsert_patcher_emitted_source_buffer(
                                        &buffer_name,
                                        &path,
                                        &last_valid_source,
                                    ) {
                                    Ok(name) => name,
                                    Err(error) => {
                                        editor.handle_host_event(HostEvent::Error(error));
                                        continue;
                                    }
                                };
                                show_instrument_patcher_source_layout_source(
                                    &buffer_name,
                                    &source_buffer_name,
                                )
                            };
                        match editor.runtime_mut().eval_str(&layout_source) {
                            Ok(_) => editor.refresh_runtime_side_effects(),
                            Err(error) => editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch source layout: {error:?}"
                            ))),
                        }
                    }

                    "enter-new-effect-editor" => {
                        if editor_mode.is_some()
                            || instrument_edit_session.is_some()
                            || effect_edit_session.is_some()
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Close the current editor before creating a new effect"
                            )));
                            continue;
                        }
                        if app.tracks.is_empty() {
                            editor.handle_host_event(HostEvent::Error(
                                "No current track is available for a new effect".to_string(),
                            ));
                            continue;
                        }
                        let track = current_track.load(Ordering::Relaxed);
                        app.ui.cursor_track = track;
                        let Some(slot) = app.next_free_custom_slot() else {
                            editor.handle_host_event(HostEvent::Error(
                                "No free effect slots available".to_string(),
                            ));
                            continue;
                        };
                        let temp_dir = match create_new_effect_draft_dir() {
                            Ok(dir) => dir,
                            Err(error) => {
                                editor.handle_host_event(HostEvent::Error(error));
                                continue;
                            }
                        };
                        let file_path = temp_dir.join("dsp.lisp");
                        if let Err(error) =
                            std::fs::write(&file_path, sequencer::lisp_effect::EFFECT_TEMPLATE)
                        {
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to write starter effect: {error}"
                            )));
                            continue;
                        }
                        match sequencer::lisp_effect::compile_and_load_with_asset_base(
                            sequencer::lisp_effect::EFFECT_TEMPLATE,
                            app.graph.sample_rate,
                            file_path.parent(),
                        )
                        .and_then(|result| {
                            app.apply_compiled_effect_to_slot_sync(
                                result,
                                NEW_EFFECT_DRAFT_NAME,
                                slot,
                                track,
                            )
                        }) {
                            Ok(()) => {}
                            Err(error) => {
                                let _ = std::fs::remove_dir_all(&temp_dir);
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to create draft effect: {error}"
                                )));
                                continue;
                            }
                        }

                        let buf_name = "*effect-patcher:new-effect*".to_string();
                        editor.remove_buffer_by_name(&buf_name);
                        editor.create_scratch_buffer(&buf_name, "", BufferMode::ESeqLisp);
                        let patcher_source = effect_patcher_buffer_source(&buf_name, &file_path);
                        if let Err(error) = editor.runtime_mut().eval_str(&patcher_source) {
                            let _ = app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to build patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        reset_effect_patcher_state(&file_path);
                        let layout_source = show_instrument_patcher_layout_source(&buf_name);
                        if let Err(error) = editor.runtime_mut().eval_str(&layout_source) {
                            let _ = app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot);
                            let _ = std::fs::remove_dir_all(&temp_dir);
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to show patch editor: {error:?}"
                            )));
                            editor.remove_buffer_by_name(&buf_name);
                            continue;
                        }
                        editor_buffer_name = Some(buf_name.clone());
                        editor_mode = Some("new-effect".to_string());
                        effect_edit_session = Some(EffectEditSession::begin_create_draft(
                            NEW_EFFECT_DRAFT_NAME.to_string(),
                            file_path,
                            buf_name.clone(),
                            EffectEditTarget::Track { track, slot },
                            sequencer::lisp_effect::EFFECT_TEMPLATE.to_string(),
                            temp_dir,
                        ));
                        let rt = editor.runtime_mut();
                        let _ = rt.eval_str("(set! sbrowser-editor-name \"\")");
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(true));
                        rt.set_reactive(
                            "SEQ",
                            "editor-mode",
                            Value::String("new-effect".to_string()),
                        );
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "editor-buffer-name",
                            Value::String(buf_name.clone()),
                        );
                        rt.set_reactive(
                            "SEQ",
                            "effects",
                            build_effects_value(
                                &state,
                                track,
                                &app.graph.effect_descriptors,
                                &selected_steps,
                            ),
                        );
                        rt.run_reactive_cycle();
                        if let Err(error) = rt.eval_str("(sbrowser-refresh-buffer)") {
                            let _ = app
                                .graph_controller()
                                .delete_custom_effect_slot(track, slot);
                            if let Some(EffectEditSession {
                                mode: EffectEditMode::CreateDraft { temp_dir },
                                ..
                            }) = effect_edit_session.take()
                            {
                                let _ = std::fs::remove_dir_all(temp_dir);
                            }
                            editor.remove_buffer_by_name(&buf_name);
                            editor_buffer_name = None;
                            editor_mode = None;
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
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to refresh effect editor sidebar: {error:?}"
                            )));
                            continue;
                        }
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*samples*");
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Created draft effect in slot {}",
                            slot + 1
                        )));
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
                                    let Some(session) = effect_edit_session.as_ref() else {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "No draft effect session is active".to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    };
                                    if !matches!(&session.mode, EffectEditMode::CreateDraft { .. })
                                    {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Current editor session is not a draft effect"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    if !session.visible_revision_valid {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(
                                                "Cannot finalize: the current patch has errors"
                                                    .to_string(),
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let final_slug =
                                        sequencer::agent::actions::normalize_patch_name(
                                            &effect_name,
                                            "new-effect",
                                        );
                                    let final_name = format!("{final_slug}/");
                                    let (final_dir, legacy_file) =
                                        finalized_effect_storage_paths(&final_slug);
                                    if final_dir.exists() || legacy_file.exists() {
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Effect '{final_slug}' already exists"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }

                                    let source = session.last_valid_source.clone();
                                    if let Err(e) =
                                        sequencer::lisp_effect::save_effect(&final_name, &source)
                                    {
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
                                    let final_dsp =
                                        sequencer::lisp_effect::effect_source_path(&final_name);
                                    if let Some(layout) = session.last_valid_layout.as_deref() {
                                        if let Err(e) =
                                            write_patcher_layout_sidecar(&final_dsp, layout)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save layout: {e}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    } else if let EffectEditMode::CreateDraft { temp_dir } =
                                        &session.mode
                                    {
                                        let source_dsp = temp_dir.join("dsp.lisp");
                                        if let Err(e) =
                                            copy_patcher_layout_sidecar(&source_dsp, &final_dsp)
                                        {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(format!(
                                                    "Failed to save layout: {e}"
                                                )),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            continue;
                                        }
                                    }
                                    let (track, slot) = match session.target {
                                        EffectEditTarget::Track { track, slot } => (track, slot),
                                        EffectEditTarget::Bus { .. } => {
                                            let _ = std::fs::remove_dir_all(&final_dir);
                                            editor.handle_host_event(HostEvent::Error(
                                                "Draft effects can only target track effect slots"
                                                    .to_string(),
                                            ));
                                            continue;
                                        }
                                    };
                                    if let Err(error) =
                                        app.load_saved_effect_to_slot_sync(track, slot, &final_name)
                                    {
                                        let _ = std::fs::remove_dir_all(&final_dir);
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(format!(
                                                "Failed to load finalized effect: {error}"
                                            )),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        continue;
                                    }
                                    let session =
                                        effect_edit_session.take().expect("session exists");
                                    if let EffectEditMode::CreateDraft { temp_dir } = session.mode {
                                        let _ = std::fs::remove_dir_all(temp_dir);
                                    }
                                    reset_effect_patcher_state(&session.path);
                                    if let Err(error) = editor
                                        .runtime_mut()
                                        .eval_str(restore_instrument_patcher_layout_source())
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to restore main editor layout: {error:?}"
                                        )));
                                        continue;
                                    }
                                    editor.refresh_runtime_side_effects();
                                    editor.remove_buffer_by_name(&session.buffer_name);
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
                                        "available-builtin-effects",
                                        build_available_builtin_effects(),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "available-effects",
                                        build_available_effects(),
                                    );
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                                    ui_epoch.fetch_add(1, Ordering::Relaxed);
                                    editor.handle_host_event(HostEvent::Status(format!(
                                        "Finalized effect '{}' in slot {}",
                                        display_instrument_name(&final_name),
                                        slot + 1
                                    )));
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
                                    let bus_idx =
                                        map.get("bus").and_then(|cell| match &*cell.borrow() {
                                            Value::Number(n) => Some(*n as usize),
                                            _ => None,
                                        });
                                    let file_path =
                                        sequencer::lisp_effect::effect_source_path(&effect_name);
                                    if !file_path.exists() {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Effect file not found: {}",
                                            file_path.display()
                                        )));
                                        continue;
                                    }
                                    let target = match (bus_idx, slot_idx) {
                                        (Some(bus), Some(slot)) => {
                                            EffectEditTarget::Bus { bus, slot }
                                        }
                                        (None, Some(slot)) => EffectEditTarget::Track {
                                            track: current_track.load(Ordering::Relaxed),
                                            slot,
                                        },
                                        _ => {
                                            editor.handle_host_event(HostEvent::Error(
                                                "Effect edit command did not include a target slot"
                                                    .to_string(),
                                            ));
                                            continue;
                                        }
                                    };
                                    let persisted_source = match std::fs::read_to_string(&file_path)
                                    {
                                        Ok(source) => source,
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to read '{}': {error}",
                                                file_path.display()
                                            )));
                                            continue;
                                        }
                                    };
                                    let buf_name = format!("*effect-patcher:{effect_name}*");
                                    editor.remove_buffer_by_name(&buf_name);
                                    editor.create_scratch_buffer(
                                        &buf_name,
                                        "",
                                        BufferMode::ESeqLisp,
                                    );
                                    let patcher_source =
                                        effect_patcher_buffer_source(&buf_name, &file_path);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&patcher_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to build patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    reset_effect_patcher_state(&file_path);
                                    let layout_source =
                                        show_instrument_patcher_layout_source(&buf_name);
                                    if let Err(error) =
                                        editor.runtime_mut().eval_str(&layout_source)
                                    {
                                        editor.handle_host_event(HostEvent::Error(format!(
                                            "Failed to show patch editor: {error:?}"
                                        )));
                                        editor.remove_buffer_by_name(&buf_name);
                                        continue;
                                    }
                                    editor_buffer_name = Some(buf_name.clone());
                                    editor_mode = Some("edit-effect".to_string());
                                    effect_edit_session =
                                        Some(EffectEditSession::begin_edit_existing(
                                            effect_name.clone(),
                                            file_path,
                                            buf_name.clone(),
                                            target,
                                            persisted_source,
                                        ));
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
                        let Some(session) = effect_edit_session.as_ref() else {
                            editor.handle_host_event(HostEvent::Error(
                                "No effect being edited".to_string(),
                            ));
                            continue;
                        };
                        if !session.visible_revision_valid {
                            let rt = editor.runtime_mut();
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(
                                    "Cannot save: the current patch has errors".to_string(),
                                ),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            continue;
                        }
                        if let Err(e) = std::fs::write(&session.path, &session.last_valid_source) {
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
                        if let Some(layout) = session.last_valid_layout.as_deref() {
                            if let Err(e) = write_patcher_layout_sidecar(&session.path, layout) {
                                let rt = editor.runtime_mut();
                                rt.set_reactive(
                                    "SEQ",
                                    "editor-error",
                                    Value::String(format!("Failed to save layout: {e}")),
                                );
                                rt.run_reactive_cycle();
                                editor.refresh_runtime_side_effects();
                                continue;
                            }
                        }
                        let session = effect_edit_session.take().expect("session exists");
                        reset_effect_patcher_state(&session.path);
                        if let Err(error) = editor
                            .runtime_mut()
                            .eval_str(restore_instrument_patcher_layout_source())
                        {
                            editor.handle_host_event(HostEvent::Error(format!(
                                "Failed to restore main editor layout: {error:?}"
                            )));
                            effect_edit_session = Some(session);
                            continue;
                        }
                        editor.refresh_runtime_side_effects();
                        editor.remove_buffer_by_name(&session.buffer_name);
                        editor_buffer_name = None;
                        editor_mode = None;

                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
                        match session.target {
                            EffectEditTarget::Track { track, .. } => {
                                rt.set_reactive(
                                    "SEQ",
                                    "effects",
                                    build_effects_value(
                                        &state,
                                        track,
                                        &app.graph.effect_descriptors,
                                        &selected_steps,
                                    ),
                                );
                            }
                            EffectEditTarget::Bus { .. } => {
                                *bus_state.lock().unwrap() = app.buses.clone();
                                sync_bus_mixer_state(rt, &app);
                                rt.set_reactive(
                                    "SEQ",
                                    "bus-effects",
                                    build_bus_effects_value_for_selection(
                                        &app,
                                        Some(&selected_steps),
                                    ),
                                );
                            }
                        }
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*fx*");
                        fx_epoch.fetch_add(1, Ordering::Relaxed);
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status(format!(
                            "Saved effect '{}'",
                            session.name
                        )));
                    }

                    "cancel-editor" => {
                        if pending_instrument_cancel_restore.is_some()
                            || pending_effect_cancel_restore.is_some()
                        {
                            continue;
                        }
                        let cancelled_patcher =
                            instrument_edit_session.is_some() || effect_edit_session.is_some();
                        if let Some(session) = instrument_edit_session.take() {
                            pending_instrument_preview = None;
                            reset_instrument_patcher_state(&session.path);
                            match session.mode.clone() {
                                InstrumentEditMode::EditExisting { persisted_source } => {
                                    let source = persisted_source.clone();
                                    let sample_rate = app.graph.sample_rate;
                                    let asset_base =
                                        session.path.parent().map(|parent| parent.to_path_buf());
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    std::thread::spawn(move || {
                                        let result = sequencer::lisp_effect::compile_and_load_instrument_with_asset_base(
                                            &source,
                                            sample_rate,
                                            asset_base.as_deref(),
                                        );
                                        let _ = tx.send(result);
                                    });
                                    pending_instrument_cancel_restore =
                                        Some(PendingInstrumentCancelRestore {
                                            session,
                                            persisted_source,
                                            receiver: rx,
                                        });
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.mark_needs_redraw();
                                    continue;
                                }
                                InstrumentEditMode::CreateDraft {
                                    temp_dir,
                                    draft_track,
                                    original_track,
                                } => {
                                    let delete_result = if app.tracks.len() > 1 {
                                        app.graph_controller().delete_track(draft_track)
                                    } else {
                                        app.graph_controller().clear_track_in_place(draft_track)
                                    };
                                    match delete_result {
                                        Ok(_) => {
                                            let restored_track = if app.tracks.is_empty() {
                                                0
                                            } else {
                                                original_track.min(app.tracks.len() - 1)
                                            };
                                            current_track.store(restored_track, Ordering::Relaxed);
                                            app.ui.cursor_track = restored_track;
                                            {
                                                let mut pan_ids = track_pan_ids.lock().unwrap();
                                                *pan_ids = app
                                                    .graph
                                                    .track_node_ids
                                                    .iter()
                                                    .map(|ids| ids.pan_id)
                                                    .collect();
                                                push_solo_mutes(lg_raw, &state, &pan_ids);
                                            }
                                            *record_armed.lock().unwrap() =
                                                app.graph.record_armed.clone();
                                            let rt = editor.runtime_mut();
                                            sync_track_topology_state(
                                                rt,
                                                &app,
                                                &state,
                                                &mut track_names,
                                                restored_track,
                                                &selected_steps,
                                                &piano_roll_selection,
                                                &accumulator_names,
                                                &record_armed,
                                                &cached_track_peak_levels,
                                            );
                                            rt.clear_subtree_effects_for_named_target(
                                                "*sequencer*",
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            refresh_visible_track_topology_layouts(&mut editor);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(error) => {
                                            editor.handle_host_event(HostEvent::Error(format!(
                                                "Failed to remove draft instrument track: {error}"
                                            )));
                                        }
                                    }
                                    let _ = std::fs::remove_dir_all(temp_dir);
                                }
                            }
                        }
                        if let Some(session) = effect_edit_session.take() {
                            pending_effect_preview = None;
                            reset_effect_patcher_state(&session.path);
                            match session.mode.clone() {
                                EffectEditMode::EditExisting { persisted_source } => {
                                    let source = persisted_source.clone();
                                    let sample_rate = app.graph.sample_rate;
                                    let asset_base =
                                        session.path.parent().map(|parent| parent.to_path_buf());
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    std::thread::spawn(move || {
                                        let result =
                                            sequencer::lisp_effect::compile_and_load_with_asset_base(
                                                &source,
                                                sample_rate,
                                                asset_base.as_deref(),
                                            );
                                        let _ = tx.send(result);
                                    });
                                    pending_effect_cancel_restore =
                                        Some(PendingEffectCancelRestore {
                                            session,
                                            receiver: rx,
                                        });
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(true));
                                    rt.set_reactive(
                                        "SEQ",
                                        "editor-error",
                                        Value::String(String::new()),
                                    );
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                    editor.mark_needs_redraw();
                                    continue;
                                }
                                EffectEditMode::CreateDraft { temp_dir } => {
                                    if let EffectEditTarget::Track { track, slot } = session.target
                                    {
                                        match app
                                            .graph_controller()
                                            .delete_custom_effect_slot(track, slot)
                                        {
                                            Ok(()) => {
                                                let rt = editor.runtime_mut();
                                                rt.set_reactive(
                                                    "SEQ",
                                                    "effects",
                                                    build_effects_value(
                                                        &state,
                                                        track,
                                                        &app.graph.effect_descriptors,
                                                        &selected_steps,
                                                    ),
                                                );
                                                rt.run_reactive_cycle();
                                                editor.refresh_runtime_side_effects();
                                                editor.refresh_visible_layouts_for_buffer_named(
                                                    "*fx*",
                                                );
                                                fx_epoch.fetch_add(1, Ordering::Relaxed);
                                                ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            }
                                            Err(error) => {
                                                editor.handle_host_event(HostEvent::Error(
                                                    format!(
                                                    "Failed to remove draft effect slot: {error}"
                                                ),
                                                ));
                                            }
                                        }
                                    }
                                    let _ = std::fs::remove_dir_all(temp_dir);
                                }
                            }
                        }
                        if let Some(buf_name) = editor_buffer_name.take() {
                            if cancelled_patcher {
                                if let Err(error) = editor
                                    .runtime_mut()
                                    .eval_str(restore_instrument_patcher_layout_source())
                                {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Failed to restore main editor layout: {error:?}"
                                    )));
                                }
                                editor.refresh_runtime_side_effects();
                            } else {
                                editor.swap_buffer_in_tile_showing(&buf_name, "*metal*");
                            }
                            editor.remove_buffer_by_name(&buf_name);
                        }

                        editor_mode = None;
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "editor-instrument-run-mode",
                            Value::String("instrument".to_string()),
                        );
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

        let mut project_load_still_pending = false;
        if app.has_pending_project_load() {
            let was_pending = true;
            match app.advance_pending_project_load() {
                Ok(()) => {
                    if app.has_pending_project_load() {
                        project_load_still_pending = true;
                    } else if was_pending {
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
                        sync_shared_track_collapsed(&track_collapsed, &app);
                        let restored_track = if app.tracks.is_empty() {
                            0
                        } else {
                            app.ui.cursor_track.min(app.tracks.len() - 1)
                        };
                        current_track.store(restored_track, Ordering::Relaxed);
                        app.ui.cursor_track = restored_track;
                        {
                            let mut pan_ids = track_pan_ids.lock().unwrap();
                            *pan_ids = app
                                .graph
                                .track_node_ids
                                .iter()
                                .map(|ids| ids.pan_id)
                                .collect();
                            push_solo_mutes(lg_raw, &state, &pan_ids);
                        }
                        *bus_node_ids.lock().unwrap() = app.graph.bus_node_ids.clone();
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
                        cached_peak_l_level = meter_display_level(f32::from_bits(
                            state.transport.peak_l.load(Ordering::Relaxed),
                        ));
                        cached_peak_r_level = meter_display_level(f32::from_bits(
                            state.transport.peak_r.load(Ordering::Relaxed),
                        ));
                        cached_track_peak_levels =
                            read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                        cached_bus_peak_levels =
                            read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                        (cached_modulator_phases, cached_modulator_levels) =
                            read_modulator_display_values(app.graph.lg, &app);
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
                            "master-recording",
                            Value::Bool(master_recording.load(Ordering::Acquire)),
                        );
                        sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                        sync_modulator_phase_fields(rt, &cached_modulator_phases);
                        sync_modulator_level_fields(rt, &cached_modulator_levels);
                        rt.set_reactive(
                            "SEQ",
                            "num-tracks",
                            Value::Number(track_names.len() as f64),
                        );
                        set_current_track_reactive(rt, app.tracks.len(), ct);
                        rt.set_reactive("SEQ", "track-ids", build_track_ids(&app));
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
                            rt.set_reactive("SEQ", "delays", Value::List(vec![]));
                            sync_track_mixer_empty_state(rt);
                            rt.set_reactive("SEQ", "effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "midi-effects", Value::List(vec![]));
                            rt.set_reactive("SEQ", "instrument-panel", Value::List(vec![]));
                            rt.set_reactive("SEQ", "step-has-plocks", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-num-steps", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-duration-spans", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-playheads", Value::List(vec![]));
                            rt.set_reactive("SEQ", "track-step-has-plocks", Value::List(vec![]));
                        } else {
                            sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
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
                            sync_track_mixer_state(rt, &app, &state);
                            sync_track_peak_fields(rt, &cached_track_peak_levels);
                            sync_bus_peak_fields(rt, &cached_bus_peak_levels);
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
                                "midi-effects",
                                build_midi_effects_value(&state, ct, &selected_steps),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "instrument-panel",
                                build_instrument_panel_value(&app, ct, &selected_steps),
                            );
                            *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                            let selected_neural_snapshot =
                                selected_neural_neurons.lock().unwrap().clone();
                            sync_track_params_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                ct,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            sync_fx_param_binding_fields_with_neural_selection(
                                rt,
                                &app,
                                &state,
                                ct,
                                &selected_steps,
                                Some(&selected_neural_snapshot),
                            );
                            rt.set_reactive(
                                "SEQ",
                                "step-has-plocks",
                                build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                            );
                            sync_sidebar_browser(rt, &app, ct);
                        }

                        rt.clear_subtree_effects_for_named_target("*sequencer*");
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                        if let Err(error) =
                            evaluate_project_scratch_on_ui_runtime(&mut editor, &app)
                        {
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Scratch UI eval error: {error}"
                            )));
                        }
                        ui_invalidations.clear();
                        expanded_step_projection.clear();

                        prev_current_track = ct;
                        prev_playhead = playhead;
                        prev_transport_playhead = transport_playhead;
                        prev_bpm = bpm;
                        prev_playing = playing;
                        prev_pattern_epoch = epoch;
                        prev_cpu_load_bits = cached_cpu_load_bits;
                        prev_peak_l_level = cached_peak_l_level;
                        prev_peak_r_level = cached_peak_r_level;
                        prev_master_recording = master_recording.load(Ordering::Acquire);
                        prev_track_peak_levels = cached_track_peak_levels.clone();
                        prev_modulator_phases = cached_modulator_phases.clone();
                        prev_modulator_levels = cached_modulator_levels.clone();
                        prev_bus_playheads = bus_playhead_snapshot(&app);
                        prev_track_playheads = track_playheads_snapshot(&state, &app);
                        prev_track_button_states = track_button_state_snapshot(&state);
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

        if let Some(completed_cancel_restore) =
            pending_instrument_cancel_restore
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Instrument restore compile thread crashed".to_string()))
                    }
                })
        {
            let pending = pending_instrument_cancel_restore
                .take()
                .expect("completed cancel restore must have pending state");
            let session = pending.session;
            match completed_cancel_restore {
                Ok(result) => match app.apply_compiled_instrument_engine(
                    session.engine_id,
                    &session.name,
                    &pending.persisted_source,
                    result,
                ) {
                    Ok(()) => {
                        if let Some(buf_name) = editor_buffer_name.take() {
                            if let Err(error) = editor
                                .runtime_mut()
                                .eval_str(restore_instrument_patcher_layout_source())
                            {
                                editor.handle_host_event(HostEvent::Error(format!(
                                    "Failed to restore main editor layout: {error:?}"
                                )));
                            }
                            editor.refresh_runtime_side_effects();
                            editor.remove_buffer_by_name(&buf_name);
                        }
                        editor_mode = None;
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                        rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                        rt.set_reactive("SEQ", "editor-buffer-name", Value::String(String::new()));
                        rt.set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(
                                &app,
                                current_track.load(Ordering::Relaxed),
                                &selected_steps,
                            ),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                        editor.handle_host_event(HostEvent::Status("Editor cancelled".to_string()));
                        editor.mark_needs_redraw();
                    }
                    Err(error) => {
                        instrument_edit_session = Some(session);
                        let rt = editor.runtime_mut();
                        rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String(format!("Failed to restore instrument: {error}")),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                        editor.mark_needs_redraw();
                    }
                },
                Err(error) => {
                    instrument_edit_session = Some(session);
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(format!("Failed to restore instrument: {error}")),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.mark_needs_redraw();
                }
            }
        }

        if let Some(completed_cancel_restore) =
            pending_effect_cancel_restore.as_ref().and_then(|pending| {
                match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("Effect restore compile thread crashed".to_string()))
                    }
                }
            })
        {
            let pending = pending_effect_cancel_restore
                .take()
                .expect("completed effect cancel restore must have pending state");
            let session = pending.session;
            match completed_cancel_restore {
                Ok(result) => {
                    match apply_compiled_effect_edit_session(
                        &mut app,
                        &session,
                        &session.name,
                        result,
                    ) {
                        Ok(()) => {
                            if let Some(buf_name) = editor_buffer_name.take() {
                                if let Err(error) = editor
                                    .runtime_mut()
                                    .eval_str(restore_instrument_patcher_layout_source())
                                {
                                    editor.handle_host_event(HostEvent::Error(format!(
                                        "Failed to restore main editor layout: {error:?}"
                                    )));
                                }
                                editor.refresh_runtime_side_effects();
                                editor.remove_buffer_by_name(&buf_name);
                            }
                            editor_mode = None;
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-active", Value::Bool(false));
                            rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                            rt.set_reactive("SEQ", "editor-mode", Value::String(String::new()));
                            rt.set_reactive("SEQ", "editor-error", Value::String(String::new()));
                            rt.set_reactive(
                                "SEQ",
                                "editor-buffer-name",
                                Value::String(String::new()),
                            );
                            match session.target {
                                EffectEditTarget::Track { track, .. } => {
                                    rt.set_reactive(
                                        "SEQ",
                                        "effects",
                                        build_effects_value(
                                            &state,
                                            track,
                                            &app.graph.effect_descriptors,
                                            &selected_steps,
                                        ),
                                    );
                                }
                                EffectEditTarget::Bus { .. } => {
                                    *bus_state.lock().unwrap() = app.buses.clone();
                                    sync_bus_mixer_state(rt, &app);
                                    rt.set_reactive(
                                        "SEQ",
                                        "bus-effects",
                                        build_bus_effects_value_for_selection(
                                            &app,
                                            Some(&selected_steps),
                                        ),
                                    );
                                }
                            }
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                            editor.handle_host_event(HostEvent::Status(
                                "Editor cancelled".to_string(),
                            ));
                            editor.mark_needs_redraw();
                        }
                        Err(error) => {
                            effect_edit_session = Some(session);
                            let rt = editor.runtime_mut();
                            rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                            rt.set_reactive(
                                "SEQ",
                                "editor-error",
                                Value::String(format!("Failed to restore effect: {error}")),
                            );
                            rt.run_reactive_cycle();
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                        }
                    }
                }
                Err(error) => {
                    effect_edit_session = Some(session);
                    let rt = editor.runtime_mut();
                    rt.set_reactive("SEQ", "editor-canceling", Value::Bool(false));
                    rt.set_reactive(
                        "SEQ",
                        "editor-error",
                        Value::String(format!("Failed to restore effect: {error}")),
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    editor.mark_needs_redraw();
                }
            }
        }

        if let Some(completed_preview) = pending_instrument_preview.as_ref().and_then(|pending| {
            match pending.receiver.try_recv() {
                Ok(result) => Some(Ok((
                    pending.generation,
                    pending.source.clone(),
                    pending.layout.clone(),
                    result,
                ))),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
            }
        }) {
            let _ = pending_instrument_preview.take();
            match completed_preview {
                Ok((generation, source, layout, compile_result)) => {
                    if let Some(session) = instrument_edit_session.as_mut() {
                        if session.preview_generation == generation {
                            match compile_result {
                                Ok(result) => match app.apply_compiled_instrument_engine(
                                    session.engine_id,
                                    &session.name,
                                    &source,
                                    result,
                                ) {
                                    Ok(()) => {
                                        session.last_valid_source = source;
                                        session.last_valid_layout = layout;
                                        session.visible_revision_valid = true;
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(String::new()),
                                        );
                                        rt.set_reactive(
                                            "SEQ",
                                            "instrument-panel",
                                            build_instrument_panel_value(
                                                &app,
                                                current_track.load(Ordering::Relaxed),
                                                &selected_steps,
                                            ),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                        ui_epoch.fetch_add(1, Ordering::Relaxed);
                                        editor.handle_host_event(HostEvent::Status(format!(
                                            "Previewed instrument '{}'",
                                            session.name
                                        )));
                                    }
                                    Err(error) => {
                                        session.visible_revision_valid = false;
                                        let rt = editor.runtime_mut();
                                        rt.set_reactive(
                                            "SEQ",
                                            "editor-error",
                                            Value::String(error),
                                        );
                                        rt.run_reactive_cycle();
                                        editor.refresh_runtime_side_effects();
                                    }
                                },
                                Err(error) => {
                                    session.visible_revision_valid = false;
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }
                }
                Err(()) => {
                    if let Some(session) = instrument_edit_session.as_mut() {
                        session.visible_revision_valid = false;
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Instrument preview compile thread crashed".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                }
            }
        }

        if let Some(completed_preview) =
            pending_effect_preview
                .as_ref()
                .and_then(|pending| match pending.receiver.try_recv() {
                    Ok(result) => Some(Ok((
                        pending.generation,
                        pending.source.clone(),
                        pending.layout.clone(),
                        result,
                    ))),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                })
        {
            let _ = pending_effect_preview.take();
            match completed_preview {
                Ok((generation, source, layout, compile_result)) => {
                    if let Some(session) = effect_edit_session.as_mut() {
                        if session.preview_generation == generation {
                            match compile_result {
                                Ok(result) => {
                                    let name = session.name.clone();
                                    match apply_compiled_effect_edit_session(
                                        &mut app, session, &name, result,
                                    ) {
                                        Ok(()) => {
                                            session.last_valid_source = source;
                                            session.last_valid_layout = layout;
                                            session.visible_revision_valid = true;
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(String::new()),
                                            );
                                            match session.target {
                                                EffectEditTarget::Track { track, .. } => {
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "effects",
                                                        build_effects_value(
                                                            &state,
                                                            track,
                                                            &app.graph.effect_descriptors,
                                                            &selected_steps,
                                                        ),
                                                    );
                                                }
                                                EffectEditTarget::Bus { .. } => {
                                                    *bus_state.lock().unwrap() = app.buses.clone();
                                                    sync_bus_mixer_state(rt, &app);
                                                    rt.set_reactive(
                                                        "SEQ",
                                                        "bus-effects",
                                                        build_bus_effects_value_for_selection(
                                                            &app,
                                                            Some(&selected_steps),
                                                        ),
                                                    );
                                                }
                                            }
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                            editor.refresh_visible_layouts_for_buffer_named("*fx*");
                                            fx_epoch.fetch_add(1, Ordering::Relaxed);
                                            ui_epoch.fetch_add(1, Ordering::Relaxed);
                                            editor.handle_host_event(HostEvent::Status(format!(
                                                "Previewed effect '{}'",
                                                session.name
                                            )));
                                        }
                                        Err(error) => {
                                            session.visible_revision_valid = false;
                                            let rt = editor.runtime_mut();
                                            rt.set_reactive(
                                                "SEQ",
                                                "editor-error",
                                                Value::String(error),
                                            );
                                            rt.run_reactive_cycle();
                                            editor.refresh_runtime_side_effects();
                                        }
                                    }
                                }
                                Err(error) => {
                                    session.visible_revision_valid = false;
                                    let rt = editor.runtime_mut();
                                    rt.set_reactive("SEQ", "editor-error", Value::String(error));
                                    rt.run_reactive_cycle();
                                    editor.refresh_runtime_side_effects();
                                }
                            }
                        }
                    }
                }
                Err(()) => {
                    if let Some(session) = effect_edit_session.as_mut() {
                        session.visible_revision_valid = false;
                        let rt = editor.runtime_mut();
                        rt.set_reactive(
                            "SEQ",
                            "editor-error",
                            Value::String("Effect preview compile thread crashed".to_string()),
                        );
                        rt.run_reactive_cycle();
                        editor.refresh_runtime_side_effects();
                    }
                }
            }
        }
        let mut completed_agentic = Vec::new();
        for (key, pending) in &pending_agentic_bubbles {
            match pending.receiver.try_recv() {
                Ok(result) => completed_agentic.push((key.clone(), result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => completed_agentic
                    .push((key.clone(), Err("request worker disconnected".to_string()))),
            }
        }
        for (key, result) in completed_agentic {
            let Some(pending) = pending_agentic_bubbles.remove(&key) else {
                continue;
            };
            match result {
                Ok(output) => match output {
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::Macro {
                        macro_name,
                        source,
                    } => match eseqlisp::widget_render::patcher::resolve_agentic_bubble(
                        &pending.path,
                        pending.intent,
                        &pending.bubble_id,
                        pending.generation,
                        &macro_name,
                        &source,
                    ) {
                        Ok(()) => {
                            eprintln!(
                                "[agentic-bubble] host materialized path={} bubble={} generation={} macro={}",
                                pending.path.display(),
                                pending.bubble_id,
                                pending.generation,
                                macro_name
                            );
                            editor.refresh_runtime_side_effects();
                            editor.mark_needs_redraw();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Generated macro '{}'",
                                macro_name
                            )));
                        }
                        Err(error) => {
                            eprintln!(
                                "[agentic-bubble] host materialize failed path={} bubble={} generation={} error={}",
                                pending.path.display(),
                                pending.bubble_id,
                                pending.generation,
                                error
                            );
                            eseqlisp::widget_render::patcher::fail_agentic_bubble(
                                &pending.path,
                                &pending.bubble_id,
                                pending.generation,
                                "materialize failed",
                                error.clone(),
                            );
                            editor.mark_needs_redraw();
                            editor.handle_host_event(HostEvent::Status(format!(
                                "Agentic bubble failed: {error}"
                            )));
                        }
                    },
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::MacroEdit { source } => {
                        let macro_name = extract_macro_name_from_defmacro(&source)
                            .unwrap_or_else(|| "macro".to_string());
                        match eseqlisp::widget_render::patcher::resolve_agentic_bubble_macro_edit(
                            &pending.path,
                            pending.intent,
                            &pending.bubble_id,
                            pending.generation,
                            &macro_name,
                            &source,
                        ) {
                            Ok(()) => {
                                editor.refresh_runtime_side_effects();
                                editor.mark_needs_redraw();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Updated macro '{}'",
                                    macro_name
                                )));
                            }
                            Err(error) => {
                                eseqlisp::widget_render::patcher::fail_agentic_bubble(
                                    &pending.path,
                                    &pending.bubble_id,
                                    pending.generation,
                                    "materialize failed",
                                    error.clone(),
                                );
                                editor.mark_needs_redraw();
                                editor.handle_host_event(HostEvent::Status(format!(
                                    "Agentic bubble failed: {error}"
                                )));
                            }
                        }
                    }
                    sequencer::agent::agentic_bubble::AgenticBubbleOutput::Answer { text } => {
                        eseqlisp::widget_render::patcher::resolve_agentic_bubble_answer(
                            &pending.path,
                            &pending.bubble_id,
                            pending.generation,
                            text,
                        );
                        editor.mark_needs_redraw();
                        editor.handle_host_event(HostEvent::Status(
                            "Agentic bubble answered".to_string(),
                        ));
                    }
                },
                Err(error) => {
                    eprintln!(
                        "[agentic-bubble] host generation failed path={} bubble={} generation={} error={}",
                        pending.path.display(),
                        pending.bubble_id,
                        pending.generation,
                        error
                    );
                    eseqlisp::widget_render::patcher::fail_agentic_bubble(
                        &pending.path,
                        &pending.bubble_id,
                        pending.generation,
                        "generation failed",
                        error.clone(),
                    );
                    editor.mark_needs_redraw();
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Agentic bubble failed: {error}"
                    )));
                }
            }
        }
        if project_load_still_pending {
            continue;
        }

        poll_pending_compile_status(
            &mut app,
            &mut editor,
            &state,
            &current_track,
            &selected_steps,
            &fx_epoch,
            &ui_epoch,
        );

        // 2. Sync reactive state AFTER events
        let ct = current_track_for_app(&mut app, &current_track).unwrap_or(0);
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
            let bus_playheads = bus_playhead_snapshot(&app);
            let epoch = state.transport.pattern_epoch.load(Ordering::Relaxed);
            let metal_visible = editor_has_visible_buffer(&editor, "*metal*");
            let mixer_visible = editor_has_visible_buffer(&editor, "*mixer*");
            let sequencer_visible = editor_has_visible_buffer(&editor, "*sequencer*");
            let fx_visible = editor_has_visible_buffer(&editor, "*fx*");
            let transport_visible = editor_has_visible_buffer(&editor, "*transport*");
            let master_meter_visible = transport_visible || mixer_visible;
            let current_track_playhead_visible = editor_has_visible_buffer(&editor, "*metal*")
                || editor_has_visible_buffer(&editor, "*piano-roll*");
            let current_track_playhead_changed = playhead != prev_playhead;
            if last_meter_poll_at.elapsed() >= METER_POLL_INTERVAL {
                cached_peak_l_level = meter_display_level(f32::from_bits(
                    state.transport.peak_l.load(Ordering::Relaxed),
                ));
                cached_peak_r_level = meter_display_level(f32::from_bits(
                    state.transport.peak_r.load(Ordering::Relaxed),
                ));
                cached_track_peak_levels =
                    read_track_peak_levels(app.graph.lg, &track_pan_ids.lock().unwrap());
                cached_bus_peak_levels =
                    read_bus_peak_levels(app.graph.lg, &app.graph.bus_node_ids);
                (cached_modulator_phases, cached_modulator_levels) =
                    read_modulator_display_values(app.graph.lg, &app);
                last_meter_poll_at = Instant::now();
            }
            let mut needs_reactive_cycle = false;
            let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
            if selected_neural_snapshot != prev_selected_neural_neurons {
                needs_reactive_cycle |= sync_selected_neural_neuron_bindings(
                    editor.runtime_mut(),
                    &state,
                    &selected_neural_snapshot,
                );
                needs_reactive_cycle |= sync_fx_param_binding_fields_with_neural_selection(
                    editor.runtime_mut(),
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                needs_reactive_cycle |= sync_track_plocks_for_neural_selection(
                    editor.runtime_mut(),
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    &selected_neural_snapshot,
                );
                prev_selected_neural_neurons = selected_neural_snapshot.clone();
            }
            // Track switch — rebuild everything
            if ct != prev_current_track && !app.tracks.is_empty() {
                editor.reset_widget_scroll_for_buffer_named("*metal*");
                editor.reset_widget_scroll_for_buffer_named("*fx*");
                let cleared_step_selection = {
                    let mut selection = selected_steps.lock().unwrap();
                    let had_selection = !selection.is_empty();
                    selection.clear();
                    had_selection
                };
                let cleared_piano_selection = {
                    let mut selection = piano_roll_selection.lock().unwrap();
                    let had_selection = !selection.is_empty();
                    selection.clear();
                    had_selection
                };
                if cleared_step_selection || cleared_piano_selection {
                    fx_epoch.fetch_add(1, Ordering::Relaxed);
                }
                let _ = editor.runtime_mut().eval_str("(set! selected-bus -1)");
                reset_sampler_waveform_view(&mut editor);
                let rt = editor.runtime_mut();
                sync_shared_track_collapsed(&track_collapsed, &app);
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                set_current_track_reactive(rt, app.tracks.len(), ct);
                if current_track_playhead_visible {
                    sync_playhead_fields(
                        rt,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                }
                if transport_visible {
                    rt.set_reactive(
                        "SEQ",
                        "transport-playhead",
                        Value::Number(transport_playhead as f64),
                    );
                }
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_step_param_lists(rt, &state, ct);
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                sync_track_peak_fields(rt, &cached_track_peak_levels);
                sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                sync_modulator_phase_fields(rt, &cached_modulator_phases);
                sync_modulator_level_fields(rt, &cached_modulator_levels);
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "midi-effects",
                    build_midi_effects_value(&state, ct, &selected_steps),
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    build_instrument_panel_value(&app, ct, &selected_steps),
                );
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
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
                needs_reactive_cycle = true;
            }

            if playing != prev_playing {
                let rt = editor.runtime_mut();
                rt.set_reactive("SEQ", "playing", Value::Bool(playing));
                if sequencer_visible {
                    if playing {
                        sync_all_track_playhead_fields(rt, &state, &app);
                    } else {
                        clear_all_track_playhead_fields(rt, &app);
                    }
                }
                prev_playing = playing;
                needs_reactive_cycle = true;
                if fx_visible && !app.tracks.is_empty() {
                    let rt = editor.runtime_mut();
                    sync_track_params_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                    sync_fx_param_binding_fields_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                }
            }
            if bpm != prev_bpm {
                app.push_all_delay_bpm();
                editor
                    .runtime_mut()
                    .set_reactive("SEQ", "bpm", Value::Number(bpm as f64));
                prev_bpm = bpm;
                needs_reactive_cycle = true;
            }
            if transport_visible && cpu_load_bits != prev_cpu_load_bits {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "cpu-load-pct",
                        Value::Number(f32::from_bits(cpu_load_bits) as f64),
                    )
                    .effects_dirty;
                prev_cpu_load_bits = cpu_load_bits;
            }
            if !transport_visible && cpu_load_bits != prev_cpu_load_bits {
                prev_cpu_load_bits = cpu_load_bits;
            }
            let master_rec_on = master_recording.load(Ordering::Acquire);
            app.ui.master_recording = master_rec_on;
            if transport_visible && master_rec_on != prev_master_recording {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on))
                    .effects_dirty;
                prev_master_recording = master_rec_on;
            }
            if !transport_visible && master_rec_on != prev_master_recording {
                prev_master_recording = master_rec_on;
            }
            if master_meter_visible && cached_peak_l_level != prev_peak_l_level {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-peak-l", Value::Number(cached_peak_l_level))
                    .effects_dirty;
                prev_peak_l_level = cached_peak_l_level;
            }
            if !master_meter_visible && cached_peak_l_level != prev_peak_l_level {
                prev_peak_l_level = cached_peak_l_level;
            }
            if master_meter_visible && cached_peak_r_level != prev_peak_r_level {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive("SEQ", "master-peak-r", Value::Number(cached_peak_r_level))
                    .effects_dirty;
                prev_peak_r_level = cached_peak_r_level;
            }
            if !master_meter_visible && cached_peak_r_level != prev_peak_r_level {
                prev_peak_r_level = cached_peak_r_level;
            }
            if cached_track_peak_levels != prev_track_peak_levels {
                if mixer_visible {
                    needs_reactive_cycle |= sync_track_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_track_peak_levels,
                        &cached_track_peak_levels,
                    );
                }
                prev_track_peak_levels = cached_track_peak_levels.clone();
            }
            if cached_bus_peak_levels != prev_bus_peak_levels {
                if mixer_visible {
                    needs_reactive_cycle |= sync_bus_peak_field_delta(
                        editor.runtime_mut(),
                        &prev_bus_peak_levels,
                        &cached_bus_peak_levels,
                    );
                }
                prev_bus_peak_levels = cached_bus_peak_levels.clone();
            }
            if last_neural_visualization_poll_at.elapsed() >= NEURAL_VISUALIZATION_POLL_INTERVAL {
                last_neural_visualization_poll_at = Instant::now();
                needs_reactive_cycle |=
                    sync_neural_visualization_fields(editor.runtime_mut(), &state);
            }
            if cached_modulator_phases != prev_modulator_phases {
                if fx_visible {
                    needs_reactive_cycle |= sync_modulator_phase_field_delta(
                        editor.runtime_mut(),
                        &prev_modulator_phases,
                        &cached_modulator_phases,
                    );
                }
                prev_modulator_phases = cached_modulator_phases.clone();
            }
            if cached_modulator_levels != prev_modulator_levels {
                if fx_visible {
                    needs_reactive_cycle |= sync_modulator_level_field_delta(
                        editor.runtime_mut(),
                        &prev_modulator_levels,
                        &cached_modulator_levels,
                    );
                }
                prev_modulator_levels = cached_modulator_levels.clone();
            }
            if bus_playheads != prev_bus_playheads {
                if metal_visible {
                    editor.runtime_mut().set_reactive(
                        "SEQ",
                        "bus-playheads",
                        build_bus_playheads_value(&app),
                    );
                    needs_reactive_cycle = true;
                }
                prev_bus_playheads = bus_playheads;
            }
            if sequencer_visible {
                let previous_track_playheads = prev_track_playheads.clone();
                if sync_track_playhead_field_delta(
                    editor.runtime_mut(),
                    &state,
                    &app,
                    &mut prev_track_playheads,
                ) {
                    needs_reactive_cycle = true;
                }
                if previous_track_playheads != prev_track_playheads {
                    let auto_follow_now = auto_follow_enabled(&auto_follow_override_until);
                    let selection_empty = selected_steps.lock().unwrap().is_empty();
                    let selected = selected_steps.lock().unwrap();
                    let rt = editor.runtime_mut();
                    for mut viewport in expanded_step_projection.all_viewports() {
                        if viewport.track >= app.tracks.len() {
                            continue;
                        }
                        let active_step = track_active_playhead_step(&state, viewport.track);
                        let active_page = active_step / PAGE_SIZE;
                        if playing && auto_follow_now && selection_empty {
                            if viewport.page != active_page {
                                viewport.page = active_page;
                                viewport.cursor_step = active_step;
                                expanded_step_projection.set_viewport(viewport);
                                needs_reactive_cycle |= sync_expanded_step_viewport(
                                    rt, &state, &app, &selected, ct, viewport,
                                );
                                continue;
                            }
                        }
                        needs_reactive_cycle |=
                            sync_expanded_step_viewport_playhead(rt, &state, viewport);
                    }
                }
            } else {
                prev_track_playheads = track_playheads_snapshot(&state, &app);
            }
            if current_track_playhead_visible
                && (!prev_current_track_playhead_visible || playhead != prev_playhead)
                && !app.tracks.is_empty()
            {
                if prev_current_track_playhead_visible {
                    needs_reactive_cycle |= sync_playhead_field_delta(
                        editor.runtime_mut(),
                        prev_playhead as usize,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                } else {
                    needs_reactive_cycle |= sync_playhead_fields(
                        editor.runtime_mut(),
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                }
                prev_playhead = playhead;
            }
            if !current_track_playhead_visible && prev_playhead != playhead {
                prev_playhead = playhead;
            }
            if fx_visible && current_track_playhead_changed && !app.tracks.is_empty() {
                let rt = editor.runtime_mut();
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                needs_reactive_cycle = true;
            }
            prev_current_track_playhead_visible = current_track_playhead_visible;
            let mut profile_pattern_reactive_cycle = false;
            let mut refresh_visible_sequencer_after_cycle = false;
            let mut refresh_visible_mixer_after_cycle = false;
            let typed_invalidations = ui_invalidations.drain();
            if apply_ui_invalidations(
                typed_invalidations,
                UiInvalidationApplyCtx {
                    app: &mut app,
                    editor: &mut editor,
                    state: &state,
                    track_collapsed: &track_collapsed,
                    bus_state: &bus_state,
                    current_track_idx: ct,
                    selected_steps: &selected_steps,
                    selected_neural_neurons: &selected_neural_snapshot,
                    piano_roll_selection: &piano_roll_selection,
                    accumulator_names: &accumulator_names,
                    cached_track_peak_levels: &cached_track_peak_levels,
                    cached_bus_peak_levels: &cached_bus_peak_levels,
                    record_armed: &record_armed,
                    active_delete_target: &active_delete_target,
                    active_delete_target_version: &active_delete_target_version,
                    expanded_step_projection: &expanded_step_projection,
                    fx_visible,
                    sequencer_visible,
                    mixer_visible,
                },
            ) {
                needs_reactive_cycle = true;
            }
            if epoch != prev_pattern_epoch && !app.tracks.is_empty() {
                let profile_switch = pattern_switch_profile_enabled();
                let profile_total_started = Instant::now();
                let sync_names_pattern_elapsed;
                let mut sync_playhead_elapsed = Duration::ZERO;
                let sync_current_steps_elapsed;
                let sync_sequencer_elapsed;
                let sync_piano_elapsed;
                let sync_step_params_elapsed;
                let sync_mixer_elapsed;
                let sync_track_params_elapsed;
                let sync_fx_bindings_elapsed;
                let sync_plocks_sidebar_elapsed;
                let old_pattern_epoch = prev_pattern_epoch;
                let rt = editor.runtime_mut();
                let started = Instant::now();
                sync_shared_track_collapsed(&track_collapsed, &app);
                sync_track_name_state(rt, &mut track_names, &app);
                sync_pattern_state(rt, &state);
                let selected_neural_snapshot = selected_neural_neurons.lock().unwrap().clone();
                sync_selected_neural_neuron_bindings(rt, &state, &selected_neural_snapshot);
                sync_names_pattern_elapsed = started.elapsed();
                if current_track_playhead_visible {
                    let started = Instant::now();
                    sync_playhead_fields(
                        rt,
                        playhead as usize,
                        state.pattern.track_params[ct].get_num_steps(),
                    );
                    sync_playhead_elapsed = started.elapsed();
                }
                let started = Instant::now();
                rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                sync_current_steps_elapsed = started.elapsed();
                let started = Instant::now();
                sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                sync_sequencer_elapsed = started.elapsed();
                let started = Instant::now();
                sync_piano_roll_state(rt, &state, ct, &piano_roll_selection);
                sync_piano_elapsed = started.elapsed();
                let started = Instant::now();
                sync_step_param_lists(rt, &state, ct);
                sync_step_params_elapsed = started.elapsed();
                let started = Instant::now();
                sync_track_mixer_state(rt, &app, &state);
                sync_bus_mixer_state(rt, &app);
                if mixer_visible {
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                }
                sync_mixer_elapsed = started.elapsed();
                *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                let started = Instant::now();
                sync_track_params_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_track_params_elapsed = started.elapsed();
                let started = Instant::now();
                sync_fx_param_binding_fields_with_neural_selection(
                    rt,
                    &app,
                    &state,
                    ct,
                    &selected_steps,
                    Some(&selected_neural_snapshot),
                );
                sync_fx_bindings_elapsed = started.elapsed();
                prev_selected_neural_neurons = selected_neural_snapshot;
                let started = Instant::now();
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    build_step_has_plocks(&state, ct, &app.graph.effect_descriptors),
                );
                sync_sidebar_browser(rt, &app, ct);
                sync_plocks_sidebar_elapsed = started.elapsed();
                if profile_switch {
                    eprintln!(
                        "[pattern-switch-profile][epoch-sync] total={:.2}ms epoch {}->{} names_pattern={:.2}ms playhead={:.2}ms current_steps={:.2}ms sequencer_bindings={:.2}ms piano={:.2}ms step_params={:.2}ms mixer={:.2}ms track_params={:.2}ms fx_bindings={:.2}ms plocks_sidebar={:.2}ms",
                        duration_ms(profile_total_started.elapsed()),
                        old_pattern_epoch,
                        epoch,
                        duration_ms(sync_names_pattern_elapsed),
                        duration_ms(sync_playhead_elapsed),
                        duration_ms(sync_current_steps_elapsed),
                        duration_ms(sync_sequencer_elapsed),
                        duration_ms(sync_piano_elapsed),
                        duration_ms(sync_step_params_elapsed),
                        duration_ms(sync_mixer_elapsed),
                        duration_ms(sync_track_params_elapsed),
                        duration_ms(sync_fx_bindings_elapsed),
                        duration_ms(sync_plocks_sidebar_elapsed),
                    );
                }
                prev_pattern_epoch = epoch;
                prev_track_button_states = track_button_state_snapshot(&state);
                needs_reactive_cycle = true;
                refresh_visible_mixer_after_cycle |= mixer_visible;
                profile_pattern_reactive_cycle = profile_switch;
            }
            let ui_ep = ui_epoch.load(Ordering::Relaxed);
            if ui_ep != prev_ui_epoch {
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] ui_epoch {}->{} visible metal={} mixer={} sequencer={} fx={} ct={}",
                        prev_ui_epoch,
                        ui_ep,
                        metal_visible,
                        mixer_visible,
                        sequencer_visible,
                        fx_visible,
                        ct
                    );
                }
                pull_shared_bus_state(&mut app, &bus_state);
                let track_button_states = track_button_state_snapshot(&state);
                let track_buttons_changed = track_button_states != prev_track_button_states;
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] track_buttons_changed={} prev_buttons={} next_buttons={}",
                        track_buttons_changed,
                        prev_track_button_states.len(),
                        track_button_states.len()
                    );
                }
                let rt = editor.runtime_mut();
                if app.tracks.is_empty() {
                    sync_track_topology_state(
                        rt,
                        &app,
                        &state,
                        &mut track_names,
                        ct,
                        &selected_steps,
                        &piano_roll_selection,
                        &accumulator_names,
                        &record_armed,
                        &cached_track_peak_levels,
                    );
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                } else {
                    sync_shared_track_collapsed(&track_collapsed, &app);
                    sync_track_name_state(rt, &mut track_names, &app);
                    rt.set_reactive("SEQ", "steps", build_steps_value(&state, ct));
                    sync_step_param_lists(rt, &state, ct);
                    if metal_visible || sequencer_visible {
                        sync_all_track_sequencer_state(rt, &state, &app, ct, &selected_steps);
                    }
                    if sequencer_visible {
                        let _ = sync_all_expanded_step_viewports(
                            rt,
                            &state,
                            &app,
                            &selected_steps,
                            ct,
                            &expanded_step_projection,
                        );
                    }
                    sync_track_mixer_state(rt, &app, &state);
                    sync_bus_mixer_state(rt, &app);
                    sync_track_peak_fields(rt, &cached_track_peak_levels);
                    sync_bus_peak_fields(rt, &cached_bus_peak_levels);
                    *accumulator_names.lock().unwrap() = build_accumulator_names(&app);
                    sync_track_params_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
                    sync_fx_param_binding_fields_with_neural_selection(
                        rt,
                        &app,
                        &state,
                        ct,
                        &selected_steps,
                        Some(&selected_neural_snapshot),
                    );
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
                }
                // Sync recording state
                let rec_on = recording.load(Ordering::Relaxed);
                let master_rec_on = master_recording.load(Ordering::Acquire);
                rt.set_reactive("SEQ", "recording", Value::Bool(rec_on));
                rt.set_reactive("SEQ", "master-recording", Value::Bool(master_rec_on));
                rt.set_reactive(
                    "SEQ",
                    "delete-target-version",
                    Value::Number(active_delete_target_version.load(Ordering::Relaxed) as f64),
                );
                sync_mixer_delete_target_binding_fields(
                    rt,
                    app.tracks.len(),
                    &state,
                    active_delete_target.lock().unwrap().as_ref(),
                );
                let armed = record_armed.lock().unwrap();
                let record_armed_changed = armed.len() != app.graph.record_armed.len()
                    || armed
                        .iter()
                        .enumerate()
                        .any(|(i, armed)| app.graph.record_armed.get(i) != Some(armed));
                rt.set_reactive("SEQ", "record-armed", build_record_armed_value(&armed));
                // Sync to app for TUI recording logic
                app.ui.recording = rec_on;
                app.ui.master_recording = master_rec_on;
                prev_master_recording = master_rec_on;
                for (i, a) in armed.iter().enumerate() {
                    if i < app.graph.record_armed.len() {
                        app.graph.record_armed[i] = *a;
                    }
                }
                refresh_visible_sequencer_after_cycle = sequencer_visible;
                refresh_visible_mixer_after_cycle |=
                    mixer_visible && (record_armed_changed || track_buttons_changed);
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    eprintln!(
                        "[ui-trace][metal_seq] refresh_after_cycle sequencer={} mixer={} record_armed_changed={} track_buttons_changed={}",
                        refresh_visible_sequencer_after_cycle,
                        refresh_visible_mixer_after_cycle,
                        record_armed_changed,
                        track_buttons_changed
                    );
                }
                prev_track_button_states = track_button_states;
                prev_ui_epoch = ui_ep;
                needs_reactive_cycle = true;
            }
            let fx_ep = fx_epoch.load(Ordering::Relaxed);
            if fx_visible && fx_ep != prev_fx_epoch {
                let rt = editor.runtime_mut();
                rt.set_reactive(
                    "SEQ",
                    "effects",
                    if app.tracks.is_empty() {
                        Value::List(vec![])
                    } else {
                        build_effects_value(
                            &state,
                            ct,
                            &app.graph.effect_descriptors,
                            &selected_steps,
                        )
                    },
                );
                rt.set_reactive(
                    "SEQ",
                    "midi-effects",
                    if app.tracks.is_empty() {
                        Value::List(vec![])
                    } else {
                        build_midi_effects_value(&state, ct, &selected_steps)
                    },
                );
                rt.set_reactive(
                    "SEQ",
                    "instrument-panel",
                    if app.tracks.is_empty() {
                        Value::List(vec![])
                    } else {
                        build_instrument_panel_value(&app, ct, &selected_steps)
                    },
                );
                rt.set_reactive(
                    "SEQ",
                    "step-has-plocks",
                    if app.tracks.is_empty() {
                        Value::List(vec![])
                    } else {
                        build_step_has_plocks(&state, ct, &app.graph.effect_descriptors)
                    },
                );
                rt.set_reactive(
                    "SEQ",
                    "bus-effects",
                    build_bus_effects_value_for_selection(&app, Some(&selected_steps)),
                );
                prev_fx_epoch = fx_ep;
                needs_reactive_cycle = true;
            }
            if transport_visible && transport_playhead != prev_transport_playhead {
                needs_reactive_cycle |= editor
                    .runtime_mut()
                    .set_reactive(
                        "SEQ",
                        "transport-playhead",
                        Value::Number(transport_playhead as f64),
                    )
                    .effects_dirty;
                prev_transport_playhead = transport_playhead;
            }
            if !transport_visible && transport_playhead != prev_transport_playhead {
                prev_transport_playhead = transport_playhead;
            }
            {
                let ct = current_track.load(Ordering::Relaxed);
                let analysis_key = if app.is_sampler_track(ct) {
                    let buffer_id = app.graph.track_buffer_ids.get(ct).copied().unwrap_or(-1);
                    let entry = app.sample_analysis.cache().get(buffer_id);
                    let (status, bpm_bits, onset_count) = match entry.as_deref() {
                        Some(sequencer::analysis::AnalysisEntry::Pending) => (1, 0, 0),
                        Some(sequencer::analysis::AnalysisEntry::Ready(result)) => {
                            (2, result.bpm.to_bits(), result.onsets_frames.len())
                        }
                        Some(sequencer::analysis::AnalysisEntry::Failed(_)) => (3, 0, 0),
                        None => (0, 0, 0),
                    };
                    Some((ct, buffer_id, status, bpm_bits, onset_count))
                } else {
                    None
                };
                if analysis_key != prev_sampler_analysis_key {
                    if let Some((ct, _, _, _, _)) = analysis_key {
                        app.publish_sampler_analysis_runtime(ct);
                        editor.runtime_mut().set_reactive(
                            "SEQ",
                            "instrument-panel",
                            build_instrument_panel_value(&app, ct, &selected_steps),
                        );
                        needs_reactive_cycle = true;
                    }
                    prev_sampler_analysis_key = analysis_key;
                }
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
                let profile_cycle = profile_pattern_reactive_cycle;
                let cycle_total_started = Instant::now();
                let started = Instant::now();
                editor.runtime_mut().run_reactive_cycle();
                let reactive_elapsed = started.elapsed();
                let started = Instant::now();
                editor.refresh_runtime_side_effects();
                let side_effects_elapsed = started.elapsed();
                let mut refresh_seq_elapsed = Duration::ZERO;
                let mut refresh_mixer_elapsed = Duration::ZERO;
                if refresh_visible_sequencer_after_cycle {
                    let started = Instant::now();
                    editor.refresh_visible_layouts_for_buffer_named("*sequencer*");
                    refresh_seq_elapsed = started.elapsed();
                }
                if refresh_visible_mixer_after_cycle {
                    let started = Instant::now();
                    editor.refresh_visible_layouts_for_buffer_named("*mixer*");
                    refresh_mixer_elapsed = started.elapsed();
                }
                editor.mark_needs_redraw();
                if profile_cycle {
                    eprintln!(
                        "[pattern-switch-profile][reactive-cycle] total={:.2}ms reactive={:.2}ms side_effects={:.2}ms refresh_seq={:.2}ms refresh_mixer={:.2}ms refresh_seq_flag={} refresh_mixer_flag={}",
                        duration_ms(cycle_total_started.elapsed()),
                        duration_ms(reactive_elapsed),
                        duration_ms(side_effects_elapsed),
                        duration_ms(refresh_seq_elapsed),
                        duration_ms(refresh_mixer_elapsed),
                        refresh_visible_sequencer_after_cycle,
                        refresh_visible_mixer_after_cycle,
                    );
                }
            }
        }
        ui_loop_stats.note_sync(reactive_sync_started.elapsed());

        // Keep selection animation live only during playback; when paused, edits/events
        // still request redraws explicitly, but idle should stay cheap.
        if playing_now && !selected_steps.lock().unwrap().is_empty() {
            editor.mark_needs_redraw();
        }

        stub_animation_cache.update_size(viewport_size);

        // Render
        if last_render_at.elapsed() >= frame_interval {
            if stub_animation_active && !editor.needs_redraw() && !sdf_animation_active {
                if let Some(tiled_frame) = stub_animation_cache.frame() {
                    let render_started = Instant::now();
                    backend
                        .render_tiled(tiled_frame)
                        .map_err(|_| "render failed")?;
                    ui_loop_stats.note_frame(Duration::ZERO, render_started.elapsed());
                    last_render_at = Instant::now();
                    continue;
                }
            }
        }

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
            if backend.agent_instrument_stub_animation_visible() {
                stub_animation_cache.store(viewport_size, tiled_frame);
            } else {
                stub_animation_cache.reset();
            }
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
