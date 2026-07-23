use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::agent::actions::{
    AgentAppAction, AgentInstrumentParamSchema, AgentInstrumentPresetDraft,
    AgentInstrumentPresetSchema, AgentSessionContext,
};
use crate::agent::audition::{audition_feedback, audition_loaded_effect};
use crate::agent::dsp_validate::validate_effect_dsp_source;
use crate::agent::network::{AgentTurnError, AgentTurnResult};
use crate::agent::protocol::{AgentToolRuntime, ToolCallOutcome};
use crate::agent::providers::{AgentMessage, AgentMessageRole, AgentProviderState};
use crate::agent::store::{AgentKind, ConversationStore, EffectDraft};
use crate::agent::ui_validate::validate_effect_ui_source;
use crate::analysis::{AnalysisJob, AnalysisService};
use crate::audiograph::LiveGraphPtr;
use crate::effects::{EffectDescriptor, EffectSlotSnapshot, ParamKind, ParamScaling};
use crate::lisp_host::{
    dylib_cache::DylibCacheManager, DGenManifest, DylibLease, LoadedDGenLib, ScratchControlRuntime,
};
use crate::quantized_launch::{DuePatternLaunch, PatternLaunchTarget};
use crate::recorder::{MasterRecorder, RecordingTake};
use crate::sequencer::{
    BusGateSequence, BusId, BusPatternSnapshot, CustomInstrumentRunMode, InstrumentType,
    KeyboardTrigger, RackRouting, RackTrackSnapshot, SequencerState, StepParam, StepSnapshot,
    DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_BANK_START, DRUM_RACK_LAST_PAD_NOTE,
    DRUM_RACK_PAD_BANK_STRIDE, DRUM_RACK_PAD_COUNT, MAX_STEPS, STEPS_PER_PAGE,
};
use crate::track_color::TrackColor;

mod browser;
pub mod command;
pub mod edit;
pub mod history;
mod effect_params;
mod effects;
mod fx_chain;
mod graph;
mod hooks;
mod params;
mod projects;
mod synth;

pub use browser::BrowserNode;
#[allow(unused_imports)]
pub use command::{apply_command, AppCommand};
pub use edit::try_apply_command;

const BAR_HEIGHT: usize = 8;
const COL_WIDTH: u16 = 3;

fn param_unit(param: &crate::effects::ParamDescriptor) -> Option<String> {
    match &param.kind {
        ParamKind::Continuous { unit } => unit.clone(),
        _ => None,
    }
}

fn param_enum_labels(param: &crate::effects::ParamDescriptor) -> Vec<String> {
    match &param.kind {
        ParamKind::Enum { labels } => labels.clone(),
        _ => Vec::new(),
    }
}

fn param_scaling(param: &crate::effects::ParamDescriptor) -> String {
    match param.scaling {
        ParamScaling::Linear => "linear".to_string(),
        ParamScaling::Exponential => "exponential".to_string(),
    }
}

fn validate_runtime_preset_value(
    preset_name: &str,
    param_name: &str,
    value: f32,
    param_desc: &crate::effects::ParamDescriptor,
) -> Result<(), String> {
    if value < param_desc.min || value > param_desc.max {
        return Err(format!(
            "Preset '{}' param '{}'={} is outside the allowed range [{}, {}].",
            preset_name, param_name, value, param_desc.min, param_desc.max
        ));
    }
    if let ParamKind::Enum { labels } = &param_desc.kind {
        let rounded = value.round();
        if (value - rounded).abs() > 0.0001 {
            return Err(format!(
                "Preset '{}' param '{}' must be an integer enum index between 0 and {}.",
                preset_name,
                param_name,
                labels.len().saturating_sub(1)
            ));
        }
    }
    Ok(())
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EffectTab {
    Slot(usize),
    Synth,
    Mod,
    Sources,
    Reverb,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum SidebarTab {
    Tools,
    Agent,
    Sounds,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(super) enum EffectPaneEntry {
    Tab(EffectTab),
    PlusButton,
}

#[derive(Clone, Debug)]
pub enum PatternBtn {
    PrevPage,
    Pattern(usize),
    NextPage,
    Clone,
    Delete,
}

#[derive(Clone, Debug)]
pub struct HeldKeyboardNote {
    pub key: char,
    pub transpose: f32,
    pub step_at_press: usize,
    pub press_time: Instant,
    pub tracks: Vec<usize>,
}

// Track param cursor indices
const TP_GATE: usize = 0;
const TP_ATTACK: usize = 1;
const TP_RELEASE: usize = 2;
const TP_SWING: usize = 3;
const TP_SWING_RESOLUTION: usize = 4;
const TP_STEPS: usize = 5;
const TP_VOLUME: usize = 6;
const TP_PAN: usize = 7;
const TP_TIMEBASE: usize = 8;
const TP_SEND: usize = 9;
const TP_MASTER: usize = 10;
const TP_POLY: usize = 11;
const TP_MAX_POLY: usize = 12;
const TP_FTS: usize = 13;
const TP_LAST: usize = TP_FTS;

// Accumulator tab cursor indices
const AC_FN: usize = 0;
const AC_LIMIT: usize = 1;
const AC_MODE: usize = 2;
const AC_LAST: usize = AC_MODE;

enum PendingEditor {
    Effect {
        slot_idx: usize,
        name: Option<String>,
    },
    Instrument {
        name: Option<String>,
    },
    Scratch,
}

enum CompileTarget {
    Effect {
        name: String,
        slot_idx: usize,
        track: crate::sequencer::TrackId,
        expected_node_id: u32,
    },
    Instrument {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HookUnit {
    Step,
    Beat,
    Bar,
}

#[derive(Clone, Debug)]
pub(super) enum HookCallback {
    Source(String),
    Global(String),
}

#[derive(Clone, Debug)]
struct SequencerHook {
    id: u64,
    unit: HookUnit,
    interval: u64,
    track: usize,
    callback: HookCallback,
}

#[derive(Clone, Debug)]
struct PendingHookInvocation {
    hook_id: u64,
    track: usize,
    step_16th: u64,
    code: String,
}

struct PendingCompile {
    receiver: std::sync::mpsc::Receiver<Result<crate::lisp_host::CompileResult, String>>,
    target: CompileTarget,
    tick: usize,
}

struct PendingAgentRequest {
    receiver: Receiver<Result<AgentTurnResult, AgentTurnError>>,
    started_at: Instant,
}

enum PendingProjectLoadPhase {
    ClearExisting,
    AddTrack(usize),
    AddEffect { track_idx: usize, offset: usize },
    BuildPattern(usize),
    Finalize,
}

struct PendingProjectLoad {
    name: String,
    tick: usize,
    project: crate::project::ProjectFile,
    built_patterns: Vec<crate::sequencer::PatternSnapshot>,
    built_bus_patterns: Vec<Vec<BusPatternSnapshot>>,
    fallback_samples: usize,
    phase: PendingProjectLoadPhase,
}

#[derive(Clone)]
pub struct EngineDescriptor {
    pub name: String,
    pub source: String,
    pub manifest: DGenManifest,
    pub lib_index: usize,
    pub shared_runtime: bool,
}

#[derive(Default)]
pub struct EngineRegistry {
    pub engines: Vec<EngineDescriptor>,
    instrument_descriptors: Vec<EffectDescriptor>,
}

impl EngineRegistry {
    pub fn find_by_name_and_source(&self, name: &str, source: &str) -> Option<usize> {
        self.engines
            .iter()
            .position(|entry| entry.shared_runtime && entry.name == name && entry.source == source)
    }

    pub fn get(&self, engine_id: usize) -> Option<&EngineDescriptor> {
        self.engines.get(engine_id)
    }

    pub fn get_instrument_descriptor(&self, engine_id: usize) -> Option<&EffectDescriptor> {
        self.instrument_descriptors.get(engine_id)
    }

    pub fn replace_at(&mut self, engine_id: usize, entry: EngineDescriptor) {
        if engine_id < self.engines.len() {
            let descriptor =
                crate::lisp_host::instrument_descriptor_from_manifest(&entry.name, &entry.manifest);
            self.engines[engine_id] = entry;
            self.instrument_descriptors[engine_id] = descriptor;
        }
    }

    pub fn upsert(&mut self, entry: EngineDescriptor) -> usize {
        let descriptor =
            crate::lisp_host::instrument_descriptor_from_manifest(&entry.name, &entry.manifest);
        if !entry.shared_runtime {
            self.engines.push(entry);
            self.instrument_descriptors.push(descriptor);
            return self.engines.len() - 1;
        }
        if let Some(existing_idx) = self.find_by_name_and_source(&entry.name, &entry.source) {
            self.engines[existing_idx] = entry;
            self.instrument_descriptors[existing_idx] = descriptor;
            existing_idx
        } else {
            self.engines.push(entry);
            self.instrument_descriptors.push(descriptor);
            self.engines.len() - 1
        }
    }
}

#[cfg(test)]
mod engine_registry_tests {
    use super::{EngineDescriptor, EngineRegistry};
    use crate::lisp_host::DGenManifest;

    fn manifest() -> DGenManifest {
        DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            version: 2,
            process_abi: "dgen-c-v2-host-sample-rate".to_string(),
            total_memory_slots: 0,
            params: Vec::new(),
            groups: Vec::new(),
            envelopes: Vec::new(),
            inputs: Vec::new(),
            modulators: Vec::new(),
            mod_outputs: Vec::new(),
            mod_destinations: Vec::new(),
            n_inputs: 0,
            n_outputs: 1,
            tensors: Vec::new(),
            tensor_init_data: Vec::new(),
            voice_cell_id: None,
        }
    }

    #[test]
    fn duplicate_instrument_sources_reuse_runtime_engine_id() {
        let mut registry = EngineRegistry::default();
        let first = registry.upsert(EngineDescriptor {
            name: "bass/".to_string(),
            source: "(out 0 1 @name audio)".to_string(),
            manifest: manifest(),
            lib_index: 0,
            shared_runtime: true,
        });
        let second = registry.upsert(EngineDescriptor {
            name: "bass/".to_string(),
            source: "(out 0 1 @name audio)".to_string(),
            manifest: manifest(),
            lib_index: 1,
            shared_runtime: true,
        });

        assert_eq!(first, second);
        assert_eq!(registry.engines.len(), 1);
        assert_eq!(registry.engines[first].lib_index, 1);
    }

    #[test]
    fn replacing_an_engine_refreshes_its_cached_instrument_descriptor() {
        let mut registry = EngineRegistry::default();
        let engine_id = registry.upsert(EngineDescriptor {
            name: "old/".to_string(),
            source: "old source".to_string(),
            manifest: manifest(),
            lib_index: 0,
            shared_runtime: false,
        });
        assert_eq!(
            registry
                .get_instrument_descriptor(engine_id)
                .map(|descriptor| descriptor.name.as_str()),
            Some("old/")
        );

        registry.replace_at(
            engine_id,
            EngineDescriptor {
                name: "new/".to_string(),
                source: "new source".to_string(),
                manifest: manifest(),
                lib_index: 1,
                shared_runtime: false,
            },
        );

        assert_eq!(
            registry
                .get_instrument_descriptor(engine_id)
                .map(|descriptor| descriptor.name.as_str()),
            Some("new/")
        );
    }

    #[test]
    fn dedicated_instrument_runtime_does_not_shadow_shared_cache() {
        let mut registry = EngineRegistry::default();
        let shared = registry.upsert(EngineDescriptor {
            name: "free/".to_string(),
            source: "(out 0 1 @name audio)".to_string(),
            manifest: manifest(),
            lib_index: 0,
            shared_runtime: true,
        });
        let dedicated = registry.upsert(EngineDescriptor {
            name: "free/".to_string(),
            source: "(out 0 1 @name audio)".to_string(),
            manifest: manifest(),
            lib_index: 0,
            shared_runtime: false,
        });

        assert_ne!(shared, dedicated);
        assert_eq!(
            registry.find_by_name_and_source("free/", "(out 0 1 @name audio)"),
            Some(shared)
        );
        assert!(!registry.engines[dedicated].shared_runtime);
    }
}

pub struct EngineNodeIds {
    pub synth_ids: Vec<i32>,
    pub synth_inputs: usize,
    pub synth_outputs: usize,
    pub audio_output_channels: Vec<usize>,
    pub mod_output_channels: Vec<usize>,
    pub gatepitch_ids: Vec<i32>,
    pub modulator_ids: Vec<i32>,
    pub route_gain_ids: Vec<Vec<[i32; 2]>>,
    pub ext_route_gain_ids: Vec<Vec<[i32; crate::sequencer::EXT_MOD_INPUT_COUNT]>>,
}

#[derive(Clone, Copy)]
pub enum ParamMouseDragTarget {
    CirklonStepParam { step: usize },
    TrackParam { row_idx: usize },
    TrackListVolume,
    AccumParam { row_idx: usize },
    SynthParam { row_idx: usize },
    ModParam { row_idx: usize },
    SourceParam { row_idx: usize },
    EffectParam { slot_idx: usize, param_idx: usize },
    ReverbParam { param_idx: usize },
}

#[derive(Clone, Copy)]
pub struct ParamMouseDrag {
    pub track: usize,
    pub target: ParamMouseDragTarget,
    pub start_col: u16,
    pub start_display_value: f32,
}

pub struct EditorState {
    pending_editor: Option<PendingEditor>,
    pending_compile: Option<PendingCompile>,
    pending_project_load: Option<PendingProjectLoad>,
    pub dylib_cache: DylibCacheManager,
    lisp_libs: Vec<LoadedDGenLib>,
    effect_chain_leases: fx_chain::FxChainLeaseStore,
    pub instrument_libs: Vec<LoadedDGenLib>,
    instrument_lib_leases: Vec<Option<DylibLease>>,
    pub picker_cursor: usize,
    pub picker_filter: String,
    pub picker_items: Vec<String>,
    pub status_message: Option<(String, Instant)>,
    pub engine_registry: EngineRegistry,
    pub scratch_buffer: String,
    pub scratch_cursor: (usize, usize),
    pub scratch_runtime: Option<ScratchControlRuntime>,
    hooks: Vec<SequencerHook>,
    pending_hook_invocations: VecDeque<PendingHookInvocation>,
    next_hook_id: u64,
    next_hook_callback_id: u64,
    last_hook_step_16th: Option<u64>,
}

pub struct BrowserState {
    pub tree: Vec<BrowserNode>,
    pub cursor: usize,
    pub filter: String,
    pub scroll_offset: usize,
}

pub struct PresetBrowserState {
    pub cursor: usize,
    pub filter: String,
    pub scroll_offset: usize,
}

pub struct AgentTranscriptEntry {
    pub role: String,
    pub text: String,
}

pub struct AgentPanelState {
    pub kind: AgentKind,
    pub provider_state: AgentProviderState,
    pub transcript: Vec<AgentTranscriptEntry>,
    pub conversation: Vec<AgentMessage>,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub scroll_offset: usize,
    pub auto_retry_budget: usize,
    pub model_dropdown_open: bool,
    pub model_dropdown_cursor: usize,
    pub current_effect_artifact: Option<EffectDraft>,
    pending_request: Option<PendingAgentRequest>,
    pub load_error: Option<String>,
}

pub struct GraphState {
    pub lg: LiveGraphPtr,
    pub track_node_ids: Vec<TrackNodeIds>,
    pub sample_rate: u32,
    pub bus_l_id: i32,
    pub bus_r_id: i32,
    pub bus_node_ids: Vec<BusNodeIds>,
    pub bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    pub bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
    pub reverb_bus_id: i32,
    pub reverb_node_id: i32,
    pub track_buffer_ids: Vec<i32>,
    pub track_sample_rates: Vec<u32>,
    pub track_voice_lids: Vec<Vec<u64>>,
    pub track_instrument_types: Vec<InstrumentType>,
    pub track_instrument_run_modes: Vec<CustomInstrumentRunMode>,
    pub track_engine_ids: Vec<Option<usize>>,
    pub track_synth_node_ids: Vec<Vec<i32>>,
    pub track_gatepitch_node_ids: Vec<Vec<i32>>,
    pub engine_node_ids: Vec<Option<EngineNodeIds>>,
    pub effect_descriptors: Vec<Vec<EffectDescriptor>>,
    pub instrument_descriptors: Vec<EffectDescriptor>,
    pub record_armed: Vec<bool>,
    pub keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
    /// Cross-track mod routes currently connected in the audiograph, stored as
    /// (source mod_out node id, dest mod_in_clip node id). Owned exclusively by
    /// GraphController::sync_current_pattern_mod_routes so scene switches can
    /// diff instead of disconnecting every possible track pair.
    pub applied_mod_routes: Vec<(i32, i32)>,
    pub deferred_rack_teardowns: Vec<graph::DeferredRackTeardown>,
}

impl GraphState {
    pub fn track_exposes_mod_output(&self, track: usize) -> bool {
        match self.track_instrument_types.get(track).copied() {
            Some(InstrumentType::Modulator) => true,
            Some(InstrumentType::Custom) => self
                .track_engine_ids
                .get(track)
                .and_then(|engine_id| *engine_id)
                .and_then(|engine_id| self.engine_node_ids.get(engine_id))
                .and_then(|engine| engine.as_ref())
                .map(|engine| !engine.mod_output_channels.is_empty())
                .unwrap_or(false),
            Some(InstrumentType::Sampler) | Some(InstrumentType::Rack) | None => false,
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum InputMode {
    Normal,
    ValueEntry,
    Dropdown,
    PatternSelect,
    PresetNameEntry,
    ProjectNameEntry,
    WavExportNameEntry,
    EffectPicker,
    InstrumentPicker,
    ProjectPicker,
    StepInsert,
    StepSelect,
    StepArm,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SidebarMode {
    InstrumentPicker,
    AddTrack,
    Audition,
    Presets,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum PresetPromptKind {
    SaveNew,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Region {
    Cirklon,
    Sidebar,
    Params,
}

impl Region {
    fn next(self) -> Region {
        match self {
            Region::Cirklon => Region::Sidebar,
            Region::Sidebar => Region::Params,
            Region::Params => Region::Cirklon,
        }
    }

    fn prev(self) -> Region {
        match self {
            Region::Cirklon => Region::Params,
            Region::Sidebar => Region::Cirklon,
            Region::Params => Region::Sidebar,
        }
    }
}

/// Per-track node IDs needed for graph rewiring.
#[derive(Clone)]
#[allow(dead_code)]
pub struct TrackNodeIds {
    pub sampler_ids: Vec<i32>, // up to MAX_VOICES
    pub sampler_gatepitch_ids: Vec<i32>,
    pub sampler_modulator_ids: Vec<i32>,
    pub voice_sum_id: i32,
    pub voice_sum_r_id: i32,
    pub pan_id: i32,
    pub filter_id: i32,
    pub delay_id: i32,
    pub send_id: i32,
    pub mod_out_id: i32,
    pub mod_in_clip_ids: [i32; crate::sequencer::EXT_MOD_INPUT_COUNT],
    pub mod_env_id: i32,
    pub bus_send_ids: Vec<BusSendNodeIds>,
    pub rack_slots: Vec<RackSlotNodeIds>,
    pub rack_signature: Option<graph::RackTopologySignature>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct RackSlotNodeIds {
    pub sampler_pool_id: Option<usize>,
    pub engine_id: Option<usize>,
    pub sampler_voice_lids: Vec<u64>,
    pub sampler_ids: Vec<i32>,
    pub sampler_gatepitch_ids: Vec<i32>,
    pub sampler_modulator_ids: Vec<i32>,
    pub slot_sum_l_id: i32,
    pub slot_sum_r_id: i32,
    pub slot_pan_id: i32,
}

#[derive(Clone)]
pub struct BusSendNodeIds {
    pub destination: BusId,
    pub left_id: i32,
    pub right_id: i32,
}

/// Audio bus node IDs, passed to App::new to reduce parameter count.
pub struct AudioBuses {
    pub bus_l_id: i32,
    pub bus_r_id: i32,
    pub default_bus_nodes: Vec<BusNodeIds>,
    pub bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    pub bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
    pub reverb_bus_id: i32,
    pub reverb_node_id: i32,
}

#[derive(Clone)]
pub struct BusNodeIds {
    pub id: BusId,
    pub left_id: i32,
    pub right_id: i32,
    pub merge_id: i32,
    pub gate_id: i32,
    pub volume_id: i32,
    pub mod_in_clip_ids: [i32; crate::sequencer::EXT_MOD_INPUT_COUNT],
}

#[derive(Clone)]
pub struct BusGateRuntimeState {
    pub id: BusId,
    pub gate_id: i32,
    pub sequence: BusGateSequence,
    pub effect_slots: Vec<EffectSlotSnapshot>,
}

#[derive(Clone)]
pub struct BusChannelState {
    pub id: BusId,
    pub name: String,
    pub volume: f32,
    pub mute: bool,
    pub solo: bool,
    pub gate_sequence: BusGateSequence,
    pub effect_descriptors: Vec<EffectDescriptor>,
    pub effect_slots: Vec<EffectSlotSnapshot>,
    pub custom_effect_names: Vec<Option<String>>,
}

impl BusChannelState {
    pub fn new(id: BusId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            volume: crate::mixer_volume::default_fader(),
            mute: false,
            solo: false,
            gate_sequence: BusGateSequence::default(),
            effect_descriptors: Self::default_effect_descriptors(),
            effect_slots: Self::default_effect_slots(),
            custom_effect_names: vec![None; crate::lisp_host::MAX_CUSTOM_FX],
        }
    }

    pub fn default_effect_descriptors() -> Vec<EffectDescriptor> {
        (0..crate::lisp_host::MAX_CUSTOM_FX)
            .map(|_| EffectDescriptor::empty_custom_slot())
            .collect()
    }

    pub fn default_effect_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_host::MAX_CUSTOM_FX)
            .map(|_| EffectSlotSnapshot::new_empty())
            .collect()
    }

    pub fn default_buses() -> Vec<Self> {
        vec![
            Self::new(BusId::MIX, "Mix"),
            Self::new(BusId::DEFAULT_A, "Bus A"),
            Self::new(BusId::DEFAULT_B, "Bus B"),
        ]
    }
}

fn restored_bus_effect_default(
    descriptor: Option<&EffectDescriptor>,
    param_idx: usize,
    value: f32,
) -> f32 {
    descriptor
        .and_then(|descriptor| descriptor.params.get(param_idx))
        .map(|param| {
            if value.is_finite() {
                param.clamp(value)
            } else {
                param.default
            }
        })
        .unwrap_or(value)
}

#[cfg(test)]
mod bus_effect_default_tests {
    use super::restored_bus_effect_default;
    use crate::effects::EffectDescriptor;

    #[test]
    fn restored_bus_effect_defaults_enforce_descriptor_ranges() {
        let ott = EffectDescriptor::builtin_ott();
        let low_above_threshold = ott
            .params
            .iter()
            .position(|param| param.name == "low above thr")
            .unwrap();

        assert_eq!(
            restored_bus_effect_default(Some(&ott), low_above_threshold, 100.0),
            0.0
        );
        assert_eq!(
            restored_bus_effect_default(Some(&ott), low_above_threshold, f32::NAN),
            ott.params[low_above_threshold].default
        );
    }
}

pub struct UiState {
    pub cursor_step: usize,
    pub cursor_track: usize,
    pub active_param: StepParam,
    pub input_mode: InputMode,
    pub value_buffer: String,
    pub selection_anchor: Option<usize>,
    pub track_selection_anchor: Option<usize>,
    pub track_drag_anchor: Option<usize>,
    pub step_drag_anchor: Option<usize>,
    pub visual_steps: HashSet<usize>,
    pub should_quit: bool,
    pub focused_region: Region,
    pub sidebar_tab: SidebarTab,
    pub sidebar_mode: SidebarMode,
    pub params_column: usize,
    pub tools_cursor: usize,
    pub tools_scroll_offset: usize,
    pub effect_tab: EffectTab,
    pub effect_tab_cursor: usize,
    pub effect_param_cursor: usize,
    pub effect_scroll_offset: usize,
    pub dropdown_open: bool,
    pub dropdown_cursor: usize,
    pub track_param_dropdown: bool,
    pub last_step_click: Option<(usize, Instant)>,
    pub last_x_press: Option<Instant>,
    pub pattern_clone_pending: bool,
    pub pattern_page: usize,
    pub pattern_btn_layout: Vec<(u16, u16, PatternBtn)>,
    pub page_btn_layout: Vec<(u16, u16, usize)>,
    pub bpm_entry: bool,
    pub reverb_param_cursor: usize,
    pub reverb_size: f32,
    pub reverb_brightness: f32,
    pub reverb_replace: f32,
    pub recording: bool,
    pub keyboard_octave: i32,
    pub held_notes: Vec<HeldKeyboardNote>,
    pub piano_notes: Vec<(i32, Instant)>,
    pub piano_last_step: usize,
    pub piano_last_track: usize,
    pub piano_lo: i32,
    pub follow_override_until: Option<Instant>,
    pub instrument_picker_cursor: usize,
    pub instrument_param_cursor: usize,
    pub synth_scroll_offset: usize,
    pub mod_param_cursor: usize,
    pub mod_scroll_offset: usize,
    pub source_param_cursor: usize,
    pub source_scroll_offset: usize,
    pub preset_prompt_kind: PresetPromptKind,
    pub param_mouse_drag: Option<ParamMouseDrag>,
    /// Step clipboard: source track plus (relative offset from anchor, snapshot) pairs.
    pub step_clipboard: Option<(usize, Vec<(usize, StepSnapshot)>)>,
    pub master_recording: bool,
    /// Whether the sidebar search/filter bar has keyboard focus.
    /// When false, char keys are not consumed by the filter so armed tracks can play.
    pub sidebar_search_focused: bool,
}

pub struct App {
    pub state: Arc<SequencerState>,
    pub track_registry: crate::sequencer::TrackRegistry,
    pub device_registry: DeviceIdentityRegistry,
    pub history: history::UndoManager<history::EditPatch>,
    pub macro_engine: crate::macro_engine::MacroEngine,
    scene_macro_runtime: HashMap<crate::macro_engine::MacroId, SceneMacroRuntime>,
    pub tracks: Vec<String>,
    pub track_colors: Vec<TrackColor>,
    pub track_collapsed: Vec<bool>,
    pub buses: Vec<BusChannelState>,
    pub groups: Vec<crate::project::ProjectTrackGroup>,
    pub sampler_paths: Vec<Option<PathBuf>>,
    pub rack_selected_slots: Vec<usize>,
    pub rack_pad_bank_starts: Vec<i32>,
    pub sample_path_registry: HashMap<String, PathBuf>,
    pub sample_buffer_path_registry: HashMap<i32, PathBuf>,
    pub current_project_name: Option<String>,
    pub ui: UiState,
    pub editor: EditorState,
    pub browser: BrowserState,
    pub preset_browser: PresetBrowserState,
    pub agent_panel: AgentPanelState,
    pub agent_store: ConversationStore,
    pub graph: GraphState,
    pub master_recorder: Arc<MasterRecorder>,
    pub sample_analysis: AnalysisService,
    pub pending_recording_take: Option<RecordingTake>,
    recording_history: Option<RecordingHistoryTransaction>,
}

struct RecordingHistoryTransaction {
    before: crate::sequencer::ProjectScenes,
    changed: bool,
}

#[derive(Default)]
pub struct DeviceIdentityRegistry {
    next_id: u64,
    audio_effects: HashMap<(crate::sequencer::TrackId, usize), crate::sequencer::EffectInstanceId>,
    audio_effect_locations:
        HashMap<crate::sequencer::EffectInstanceId, (crate::sequencer::TrackId, usize)>,
    bus_audio_effects:
        HashMap<(crate::sequencer::BusId, usize), crate::sequencer::EffectInstanceId>,
    bus_audio_effect_locations:
        HashMap<crate::sequencer::EffectInstanceId, (crate::sequencer::BusId, usize)>,
    rack_audio_effects:
        HashMap<(crate::sequencer::RackSlotId, usize), crate::sequencer::EffectInstanceId>,
    rack_audio_effect_locations:
        HashMap<crate::sequencer::EffectInstanceId, (crate::sequencer::RackSlotId, usize)>,
    midi_effects: HashMap<(crate::sequencer::TrackId, usize), crate::sequencer::MidiFxInstanceId>,
    midi_effect_locations:
        HashMap<crate::sequencer::MidiFxInstanceId, (crate::sequencer::TrackId, usize)>,
    rack_slots: HashMap<(crate::sequencer::TrackId, usize), crate::sequencer::RackSlotId>,
    rack_slot_locations:
        HashMap<crate::sequencer::RackSlotId, (crate::sequencer::TrackId, usize)>,
}

impl DeviceIdentityRegistry {
    fn observe(&mut self, id: u64) {
        self.next_id = self.next_id.max(id);
    }
    fn allocate(&mut self) -> u64 {
        self.next_id = self.next_id.checked_add(1).expect("device instance id exhausted");
        self.next_id
    }

    pub(crate) fn allocate_effect_instance(&mut self) -> crate::sequencer::EffectInstanceId {
        crate::sequencer::EffectInstanceId(self.allocate())
    }

    pub(crate) fn audio_effect(
        &mut self,
        track: crate::sequencer::TrackId,
        slot: usize,
    ) -> crate::sequencer::EffectInstanceId {
        if let Some(id) = self.audio_effects.get(&(track, slot)).copied() {
            return id;
        }
        let id = crate::sequencer::EffectInstanceId(self.allocate());
        self.audio_effects.insert((track, slot), id);
        self.audio_effect_locations.insert(id, (track, slot));
        id
    }

    pub(crate) fn audio_effect_location(
        &self,
        id: crate::sequencer::EffectInstanceId,
    ) -> Option<(crate::sequencer::TrackId, usize)> {
        self.audio_effect_locations.get(&id).copied()
    }

    pub(crate) fn audio_effect_chain(
        &mut self,
        track: crate::sequencer::TrackId,
        slots: impl IntoIterator<Item = usize>,
    ) -> Vec<crate::sequencer::EffectInstanceId> {
        slots
            .into_iter()
            .map(|slot| self.audio_effect(track, slot))
            .collect()
    }

    pub(crate) fn bind_audio_effect_chain(
        &mut self,
        track: crate::sequencer::TrackId,
        first_slot: usize,
        instances: &[crate::sequencer::EffectInstanceId],
    ) -> Result<(), String> {
        for id in instances {
            self.observe(id.0);
        }
        let end_slot = first_slot
            .checked_add(crate::lisp_host::MAX_CUSTOM_FX)
            .ok_or_else(|| "audio-effect chain range overflow".to_string())?;
        let retained = self
            .audio_effects
            .iter()
            .filter(|((owner, slot), _)| {
                *owner != track || *slot < first_slot || *slot >= end_slot
            })
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        if instances.iter().enumerate().any(|(index, id)| instances[..index].contains(id)) {
            return Err("audio-effect chain contains a duplicate stable identity".to_string());
        }
        for id in instances {
            if self.bus_audio_effect_locations.contains_key(id)
                || self.rack_audio_effect_locations.contains_key(id)
            {
                return Err(format!(
                    "effect instance {} is already bound to a bus device",
                    id.0
                ));
            }
            if let Some((owner, slot)) = self.audio_effect_locations.get(id).copied() {
                if owner != track || slot < first_slot || slot >= end_slot {
                    return Err(format!(
                        "audio-effect instance {} is already bound to another device",
                        id.0
                    ));
                }
            }
        }
        self.audio_effects.clear();
        self.audio_effect_locations.clear();
        for (location, id) in retained {
            self.audio_effects.insert(location, id);
            self.audio_effect_locations.insert(id, location);
        }
        for (offset, id) in instances.iter().copied().enumerate() {
            let location = (track, first_slot + offset);
            self.audio_effects.insert(location, id);
            self.audio_effect_locations.insert(id, location);
        }
        Ok(())
    }

    pub(crate) fn bus_audio_effect(
        &mut self,
        bus: crate::sequencer::BusId,
        slot: usize,
    ) -> crate::sequencer::EffectInstanceId {
        if let Some(id) = self.bus_audio_effects.get(&(bus, slot)).copied() {
            return id;
        }
        let id = crate::sequencer::EffectInstanceId(self.allocate());
        self.bus_audio_effects.insert((bus, slot), id);
        self.bus_audio_effect_locations.insert(id, (bus, slot));
        id
    }

    pub(crate) fn bus_audio_effect_location(
        &self,
        id: crate::sequencer::EffectInstanceId,
    ) -> Option<(crate::sequencer::BusId, usize)> {
        self.bus_audio_effect_locations.get(&id).copied()
    }

    pub(crate) fn bind_bus_audio_effect_chain(
        &mut self,
        bus: crate::sequencer::BusId,
        instances: &[crate::sequencer::EffectInstanceId],
    ) -> Result<(), String> {
        for id in instances {
            self.observe(id.0);
        }
        if instances.iter().enumerate().any(|(index, id)| instances[..index].contains(id)) {
            return Err("bus effect chain contains a duplicate stable identity".to_string());
        }
        for id in instances {
            if self.audio_effect_locations.contains_key(id)
                || self.rack_audio_effect_locations.contains_key(id)
            {
                return Err(format!(
                    "effect instance {} is already bound to a track device",
                    id.0
                ));
            }
            if let Some((owner, _)) = self.bus_audio_effect_locations.get(id).copied() {
                if owner != bus {
                    return Err(format!(
                        "effect instance {} is already bound to another bus",
                        id.0
                    ));
                }
            }
        }
        let retained = self
            .bus_audio_effects
            .iter()
            .filter(|((owner, _), _)| *owner != bus)
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        self.bus_audio_effects.clear();
        self.bus_audio_effect_locations.clear();
        for (location, id) in retained {
            self.bus_audio_effects.insert(location, id);
            self.bus_audio_effect_locations.insert(id, location);
        }
        for (slot, id) in instances.iter().copied().enumerate() {
            self.bus_audio_effects.insert((bus, slot), id);
            self.bus_audio_effect_locations.insert(id, (bus, slot));
        }
        Ok(())
    }

    pub(crate) fn rack_audio_effect(
        &mut self,
        rack_slot: crate::sequencer::RackSlotId,
        slot: usize,
    ) -> crate::sequencer::EffectInstanceId {
        if let Some(id) = self.rack_audio_effects.get(&(rack_slot, slot)).copied() {
            return id;
        }
        let id = crate::sequencer::EffectInstanceId(self.allocate());
        self.rack_audio_effects.insert((rack_slot, slot), id);
        self.rack_audio_effect_locations.insert(id, (rack_slot, slot));
        id
    }

    pub(crate) fn rack_audio_effect_location(
        &self,
        id: crate::sequencer::EffectInstanceId,
    ) -> Option<(crate::sequencer::RackSlotId, usize)> {
        self.rack_audio_effect_locations.get(&id).copied()
    }

    pub(crate) fn bind_rack_audio_effect_chain(
        &mut self,
        rack_slot: crate::sequencer::RackSlotId,
        instances: &[crate::sequencer::EffectInstanceId],
    ) -> Result<(), String> {
        for id in instances {
            self.observe(id.0);
        }
        if instances.iter().enumerate().any(|(index, id)| instances[..index].contains(id)) {
            return Err("rack-slot effect chain contains a duplicate stable identity".to_string());
        }
        for id in instances {
            if self.audio_effect_locations.contains_key(id)
                || self.bus_audio_effect_locations.contains_key(id)
            {
                return Err(format!(
                    "effect instance {} is already bound to another device domain",
                    id.0
                ));
            }
            if let Some((owner, _)) = self.rack_audio_effect_locations.get(id).copied() {
                if owner != rack_slot {
                    return Err(format!(
                        "effect instance {} is already bound to another rack slot",
                        id.0
                    ));
                }
            }
        }
        let retained = self.rack_audio_effects.iter()
            .filter(|((owner, _), _)| *owner != rack_slot)
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        self.rack_audio_effects.clear();
        self.rack_audio_effect_locations.clear();
        for (location, id) in retained {
            self.rack_audio_effects.insert(location, id);
            self.rack_audio_effect_locations.insert(id, location);
        }
        for (slot, id) in instances.iter().copied().enumerate() {
            self.rack_audio_effects.insert((rack_slot, slot), id);
            self.rack_audio_effect_locations.insert(id, (rack_slot, slot));
        }
        Ok(())
    }

    pub(crate) fn midi_effect(
        &mut self,
        track: crate::sequencer::TrackId,
        slot: usize,
    ) -> crate::sequencer::MidiFxInstanceId {
        if let Some(id) = self.midi_effects.get(&(track, slot)).copied() {
            return id;
        }
        let id = crate::sequencer::MidiFxInstanceId(self.allocate());
        self.midi_effects.insert((track, slot), id);
        self.midi_effect_locations.insert(id, (track, slot));
        id
    }

    pub(crate) fn midi_effect_location(
        &self,
        id: crate::sequencer::MidiFxInstanceId,
    ) -> Option<(crate::sequencer::TrackId, usize)> {
        self.midi_effect_locations.get(&id).copied()
    }

    pub(crate) fn midi_effect_chain(
        &mut self,
        track: crate::sequencer::TrackId,
        slot_count: usize,
    ) -> Vec<crate::sequencer::MidiFxInstanceId> {
        (0..slot_count)
            .map(|slot| self.midi_effect(track, slot))
            .collect()
    }

    pub(crate) fn bind_midi_effect_chain(
        &mut self,
        track: crate::sequencer::TrackId,
        instances: &[crate::sequencer::MidiFxInstanceId],
    ) -> Result<(), String> {
        for id in instances {
            self.observe(id.0);
        }
        if instances.iter().enumerate().any(|(index, id)| instances[..index].contains(id)) {
            return Err("MIDI-FX chain contains a duplicate stable identity".to_string());
        }
        let retained = self
            .midi_effects
            .iter()
            .filter(|((owner, _), _)| *owner != track)
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        for id in instances {
            if self
                .midi_effect_locations
                .get(id)
                .is_some_and(|(owner, _)| *owner != track)
            {
                return Err(format!(
                    "MIDI-FX instance {} is already bound to another track",
                    id.0
                ));
            }
        }
        self.midi_effects.clear();
        self.midi_effect_locations.clear();
        for (location, id) in retained {
            self.midi_effects.insert(location, id);
            self.midi_effect_locations.insert(id, location);
        }
        for (slot, id) in instances.iter().copied().enumerate() {
            let location = (track, slot);
            self.midi_effects.insert(location, id);
            self.midi_effect_locations.insert(id, location);
        }
        Ok(())
    }

    pub(crate) fn insert_midi_effect_identity(
        &mut self,
        track: crate::sequencer::TrackId,
        slot: usize,
        old_len: usize,
    ) -> Result<crate::sequencer::MidiFxInstanceId, String> {
        let mut instances = self.midi_effect_chain(track, old_len);
        let id = crate::sequencer::MidiFxInstanceId(self.allocate());
        instances.insert(slot.min(instances.len()), id);
        self.bind_midi_effect_chain(track, &instances)?;
        Ok(id)
    }

    pub(crate) fn remove_midi_effect_identity(
        &mut self,
        track: crate::sequencer::TrackId,
        slot: usize,
        old_len: usize,
    ) -> Result<crate::sequencer::MidiFxInstanceId, String> {
        let mut instances = self.midi_effect_chain(track, old_len);
        if slot >= instances.len() {
            return Err("MIDI-FX identity removal is out of range".to_string());
        }
        let removed = instances.remove(slot);
        self.bind_midi_effect_chain(track, &instances)?;
        Ok(removed)
    }

    pub(crate) fn move_midi_effect_identity(
        &mut self,
        track: crate::sequencer::TrackId,
        source: usize,
        target: usize,
        len: usize,
    ) -> Result<(), String> {
        let mut instances = self.midi_effect_chain(track, len);
        if source >= instances.len() || target >= instances.len() {
            return Err("MIDI-FX identity move is out of range".to_string());
        }
        let id = instances.remove(source);
        instances.insert(target, id);
        self.bind_midi_effect_chain(track, &instances)
    }

    pub(crate) fn rack_slot(
        &mut self,
        track: crate::sequencer::TrackId,
        slot: usize,
    ) -> crate::sequencer::RackSlotId {
        if let Some(id) = self.rack_slots.get(&(track, slot)).copied() {
            return id;
        }
        let id = crate::sequencer::RackSlotId(self.allocate());
        self.rack_slots.insert((track, slot), id);
        self.rack_slot_locations.insert(id, (track, slot));
        id
    }

    pub(crate) fn rack_slot_location(
        &self,
        id: crate::sequencer::RackSlotId,
    ) -> Option<(crate::sequencer::TrackId, usize)> {
        self.rack_slot_locations.get(&id).copied()
    }

    pub(crate) fn bind_rack_slot(
        &mut self,
        track: crate::sequencer::TrackId,
        slot: usize,
        id: crate::sequencer::RackSlotId,
    ) -> Result<(), String> {
        self.observe(id.0);
        if let Some((owner, owner_slot)) = self.rack_slot_locations.get(&id).copied() {
            if owner != track || owner_slot != slot {
                return Err(format!("rack-slot instance {} is already bound", id.0));
            }
        }
        if let Some(previous) = self.rack_slots.insert((track, slot), id) {
            self.rack_slot_locations.remove(&previous);
        }
        self.rack_slot_locations.insert(id, (track, slot));
        Ok(())
    }

    pub(crate) fn clear_rack_track(&mut self, track: crate::sequencer::TrackId) {
        let removed = self.rack_slots.iter()
            .filter(|((owner, _), _)| *owner == track)
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        for (location, rack_slot) in removed {
            self.rack_slots.remove(&location);
            self.rack_slot_locations.remove(&rack_slot);
            let effects = self.rack_audio_effects.iter()
                .filter(|((owner, _), _)| *owner == rack_slot)
                .map(|(effect_location, id)| (*effect_location, *id))
                .collect::<Vec<_>>();
            for (effect_location, effect_id) in effects {
                self.rack_audio_effects.remove(&effect_location);
                self.rack_audio_effect_locations.remove(&effect_id);
            }
        }
    }

    pub(crate) fn clear_track(&mut self, track: crate::sequencer::TrackId) {
        let audio_effects = self.audio_effects.iter()
            .filter(|((owner, _), _)| *owner == track)
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        for (location, id) in audio_effects {
            self.audio_effects.remove(&location);
            self.audio_effect_locations.remove(&id);
        }
        let midi_effects = self.midi_effects.iter()
            .filter(|((owner, _), _)| *owner == track)
            .map(|(location, id)| (*location, *id))
            .collect::<Vec<_>>();
        for (location, id) in midi_effects {
            self.midi_effects.remove(&location);
            self.midi_effect_locations.remove(&id);
        }
        self.clear_rack_track(track);
    }

    pub(crate) fn clear(&mut self) {
        self.audio_effects.clear();
        self.audio_effect_locations.clear();
        self.bus_audio_effects.clear();
        self.bus_audio_effect_locations.clear();
        self.rack_audio_effects.clear();
        self.rack_audio_effect_locations.clear();
        self.midi_effects.clear();
        self.midi_effect_locations.clear();
        self.rack_slots.clear();
        self.rack_slot_locations.clear();
    }
}

#[cfg(test)]
mod device_identity_registry_tests {
    use super::DeviceIdentityRegistry;
    use crate::effects::BUILTIN_SLOT_COUNT;
    use crate::sequencer::{BusId, EffectInstanceId, TrackId};

    #[test]
    fn persisted_device_identities_advance_future_allocation() {
        let mut registry = DeviceIdentityRegistry::default();
        registry.bind_audio_effect_chain(
            TrackId(1),
            BUILTIN_SLOT_COUNT,
            &[EffectInstanceId(100)],
        ).unwrap();

        let allocated = registry.bus_audio_effect(BusId::DEFAULT_A, 0);

        assert_eq!(allocated, EffectInstanceId(101));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternLaunchOutcome {
    pub token: Option<crate::quantized_launch::QuantizedLaunchToken>,
    pub target: PatternLaunchTarget,
    /// Non-transactional graph repair failures are reported without hiding
    /// the fact that project state was successfully launched.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
struct SceneMacroRuntime {
    origin_scene: usize,
    target_token: Option<crate::quantized_launch::QuantizedLaunchToken>,
    return_token: Option<crate::quantized_launch::QuantizedLaunchToken>,
    target_applied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternLaunchError {
    SceneOutOfRange { scene: usize },
    EmptyTrackMask,
    TrackOutOfRange { track: usize },
    MissingSceneCell { scene: usize, track: usize },
}

impl App {
    pub fn apply_pattern_launch(
        &mut self,
        target: &PatternLaunchTarget,
    ) -> Result<PatternLaunchOutcome, PatternLaunchError> {
        let num_tracks = self.tracks.len();
        let scene = match target {
            PatternLaunchTarget::Scene { scene }
            | PatternLaunchTarget::SceneTracks { scene, .. } => *scene,
        };
        if scene >= self.state.scene_count() {
            return Err(PatternLaunchError::SceneOutOfRange { scene });
        }

        let sample_ids = match target {
            PatternLaunchTarget::Scene { scene } => {
                if *scene != self.state.current_scene_index() {
                    self.switch_bus_pattern(*scene);
                }
                self.state
                    .launch_scene(
                        *scene,
                        num_tracks,
                        &self.graph.track_buffer_ids,
                        &self.graph.track_sample_rates,
                        &self.tracks,
                        &self.graph.track_instrument_types,
                    )
                    .ok_or(PatternLaunchError::SceneOutOfRange { scene: *scene })?
            }
            PatternLaunchTarget::SceneTracks { scene, tracks } => {
                if tracks.is_empty() {
                    return Err(PatternLaunchError::EmptyTrackMask);
                }
                if let Some(track) = tracks.iter().copied().find(|track| *track >= num_tracks) {
                    return Err(PatternLaunchError::TrackOutOfRange { track });
                }
                if let Some(track) = tracks
                    .iter()
                    .copied()
                    .find(|track| self.state.scene_track_pattern_id(*scene, *track).is_none())
                {
                    return Err(PatternLaunchError::MissingSceneCell {
                        scene: *scene,
                        track,
                    });
                }
                if !self.state.launch_scene_tracks(
                    *scene,
                    tracks,
                    num_tracks,
                    &self.graph.track_buffer_ids,
                    &self.graph.track_sample_rates,
                    &self.tracks,
                    &self.graph.track_instrument_types,
                ) {
                    return Err(PatternLaunchError::MissingSceneCell {
                        scene: *scene,
                        track: tracks[0],
                    });
                }
                self.state.effective_pattern_sample_ids(num_tracks)
            }
        };

        self.graph_controller().apply_sample_ids(&sample_ids);
        let mut warnings = Vec::new();
        if let Err(error) = self
            .graph_controller()
            .sync_track_instrument_run_modes_from_live_state()
        {
            warnings.push(error);
        }
        if matches!(target, PatternLaunchTarget::Scene { .. }) {
            self.graph_controller().sync_current_pattern_mod_routes();
        }
        self.push_all_restored_defaults();
        Ok(PatternLaunchOutcome {
            token: None,
            target: target.clone(),
            warnings,
        })
    }

    pub fn apply_manual_pattern_launch(
        &mut self,
        target: &PatternLaunchTarget,
    ) -> Result<PatternLaunchOutcome, PatternLaunchError> {
        let _ = self.state.quantized_launches().cancel_all();
        self.scene_macro_runtime.clear();
        let mut touched = self.macro_engine.release_all_scene_macros();
        touched.extend(self.macro_engine.end_scene_push());
        self.send_macro_targets(touched);
        self.apply_pattern_launch(target)
    }

    pub fn drain_due_pattern_launches(
        &mut self,
    ) -> Vec<Result<PatternLaunchOutcome, PatternLaunchError>> {
        let due = self.state.quantized_launches().drain_valid_due();
        due.into_iter()
            .map(|DuePatternLaunch { token, target }| {
                self.apply_pattern_launch(&target).map(|mut outcome| {
                    self.note_scene_macro_launch_applied(token);
                    outcome.token = Some(token);
                    outcome
                })
            })
            .collect()
    }

    fn note_scene_macro_launch_applied(
        &mut self,
        token: crate::quantized_launch::QuantizedLaunchToken,
    ) {
        let mut completed_return = None;
        for (id, runtime) in &mut self.scene_macro_runtime {
            if runtime.target_token == Some(token) {
                runtime.target_token = None;
                runtime.target_applied = true;
            }
            if runtime.return_token == Some(token) {
                completed_return = Some(*id);
            }
        }
        if let Some(id) = completed_return {
            self.scene_macro_runtime.remove(&id);
            let touched = self.macro_engine.release(id);
            self.send_macro_targets(touched);
        }
    }

    pub fn handle_scene_deleted(&mut self, deleted: usize) {
        let _ = self.state.quantized_launches().cancel_all();
        self.scene_macro_runtime.clear();
        let mut touched = self.macro_engine.release_all_scene_macros();
        touched.extend(self.macro_engine.end_scene_push());
        self.macro_engine
            .remap_scene_targets_after_delete(deleted, self.state.scene_count());
        self.send_macro_targets(touched);
    }

    pub fn next_track_color(&self) -> TrackColor {
        TrackColor::next_for_existing(&self.track_colors)
    }

    pub fn push_next_track_color(&mut self) {
        let color = self.next_track_color();
        self.track_colors.push(color);
    }

    pub fn push_default_track_collapsed(&mut self) {
        self.track_collapsed.push(false);
    }

    pub fn set_track_color(&mut self, track: usize, color: TrackColor) {
        self.normalize_track_colors();
        if let Some(slot) = self.track_colors.get_mut(track) {
            *slot = color.clamped();
        }
    }

    pub fn normalize_track_colors(&mut self) {
        self.track_colors.truncate(self.tracks.len());
        while self.track_colors.len() < self.tracks.len() {
            self.push_next_track_color();
        }
    }

    pub fn normalize_track_collapsed(&mut self) {
        self.track_collapsed.truncate(self.tracks.len());
        while self.track_collapsed.len() < self.tracks.len() {
            self.push_default_track_collapsed();
        }
    }

    pub fn set_track_collapsed(&mut self, track: usize, collapsed: bool) {
        self.normalize_track_collapsed();
        if let Some(slot) = self.track_collapsed.get_mut(track) {
            *slot = collapsed;
        }
    }

    pub fn replace_track_collapsed(&mut self, collapsed: Vec<bool>) {
        self.track_collapsed = collapsed;
        self.normalize_track_collapsed();
    }

    pub fn normalize_rack_selected_slots(&mut self) {
        self.rack_selected_slots.truncate(self.tracks.len());
        while self.rack_selected_slots.len() < self.tracks.len() {
            self.rack_selected_slots.push(0);
        }
        self.rack_pad_bank_starts.truncate(self.tracks.len());
        while self.rack_pad_bank_starts.len() < self.tracks.len() {
            self.rack_pad_bank_starts.push(DRUM_RACK_FIRST_PAD_NOTE);
        }
    }

    pub fn rack_selected_slot(&self, track: usize, slot_count: usize) -> usize {
        if slot_count == 0 {
            return 0;
        }
        self.rack_selected_slots
            .get(track)
            .copied()
            .unwrap_or(0)
            .min(slot_count - 1)
    }

    pub fn set_rack_selected_slot(&mut self, track: usize, slot_idx: usize) {
        self.normalize_rack_selected_slots();
        if let Some(selected) = self.rack_selected_slots.get_mut(track) {
            *selected = slot_idx;
        }
    }

    /// Select a physical rack slot while preserving each rack routing mode's
    /// selection identity. Broadcast racks select by slot index; drum racks
    /// select by the slot's pad note so sparse pad assignments stay correct.
    pub fn select_rack_slot(
        &mut self,
        track: usize,
        rack: &RackTrackSnapshot,
        slot_idx: usize,
    ) -> bool {
        let Some(slot) = rack.slots.get(slot_idx) else {
            return false;
        };
        match rack.routing {
            RackRouting::Broadcast => self.set_rack_selected_slot(track, slot_idx),
            RackRouting::ByPitch => {
                let Some(pad_note) = slot.pad_note else {
                    return false;
                };
                self.set_rack_selected_pad_note(track, pad_note);
            }
        }
        true
    }

    pub fn rack_selected_pad_note(&self, track: usize) -> i32 {
        let raw = self.rack_selected_slots.get(track).copied().unwrap_or(0) as i32;
        raw.clamp(DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE)
    }

    fn drum_rack_bank_start_for_pad_note(pad_note: i32) -> i32 {
        let clamped = pad_note.clamp(DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE);
        let relative = clamped - DRUM_RACK_FIRST_PAD_NOTE;
        let start = DRUM_RACK_FIRST_PAD_NOTE
            + (relative / DRUM_RACK_PAD_BANK_STRIDE) * DRUM_RACK_PAD_BANK_STRIDE;
        start.clamp(DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_BANK_START)
    }

    pub fn rack_pad_bank_start(&self, track: usize) -> i32 {
        self.rack_pad_bank_starts
            .get(track)
            .copied()
            .unwrap_or(DRUM_RACK_FIRST_PAD_NOTE)
            .clamp(DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_BANK_START)
    }

    pub fn set_rack_pad_bank_start(&mut self, track: usize, bank_start: i32) {
        self.normalize_rack_selected_slots();
        let bank_start = bank_start.clamp(DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_BANK_START);
        if let Some(selected_bank) = self.rack_pad_bank_starts.get_mut(track) {
            *selected_bank = bank_start;
        }
        let selected_pad = self.rack_selected_pad_note(track);
        let bank_end = bank_start + DRUM_RACK_PAD_COUNT as i32 - 1;
        if selected_pad < bank_start || selected_pad > bank_end {
            self.set_rack_selected_pad_note(track, bank_start);
        }
    }

    pub fn set_rack_selected_pad_note(&mut self, track: usize, pad_note: i32) {
        self.normalize_rack_selected_slots();
        let pad_note = pad_note.clamp(DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE);
        if let Some(selected) = self.rack_selected_slots.get_mut(track) {
            *selected = pad_note as usize;
        }
        let bank_start = self.rack_pad_bank_start(track);
        let bank_end = bank_start + DRUM_RACK_PAD_COUNT as i32 - 1;
        if pad_note < bank_start || pad_note > bank_end {
            let new_bank_start = Self::drum_rack_bank_start_for_pad_note(pad_note);
            if let Some(selected_bank) = self.rack_pad_bank_starts.get_mut(track) {
                *selected_bank = new_bank_start;
            }
        }
    }

    pub fn selected_rack_slot_index_for_rack(
        &self,
        track: usize,
        rack: &RackTrackSnapshot,
    ) -> Option<usize> {
        match rack.routing {
            RackRouting::Broadcast => {
                (!rack.slots.is_empty()).then(|| self.rack_selected_slot(track, rack.slots.len()))
            }
            RackRouting::ByPitch => {
                let selected_pad = self.rack_selected_pad_note(track);
                rack.slots
                    .iter()
                    .position(|slot| slot.pad_note == Some(selected_pad))
            }
        }
    }

    pub fn submit_sample_analysis(&self, loaded: &crate::instruments::sampler::LoadedSample) {
        self.sample_analysis.submit(AnalysisJob {
            buffer_id: loaded.buffer_id,
            samples: Arc::new(loaded.mono_samples.clone()),
            sample_rate: loaded.sample_rate,
        });
    }

    pub fn reset_sampler_bpm_for_analysis(&self, track: usize) {
        if let Some(slot) = self.state.pattern.instrument_slots.get(track) {
            slot.defaults.set(11, 120.0);
        }
    }

    pub fn publish_sampler_analysis_runtime(&self, track: usize) {
        use std::sync::atomic::Ordering;

        let Some(&buffer_id) = self.graph.track_buffer_ids.get(track) else {
            return;
        };
        let runtime = &self.state.runtime;
        runtime.sampler_analysis_buffer_ids[track].store(buffer_id as u32, Ordering::Release);
        match self.sample_analysis.cache().get(buffer_id) {
            Some(entry) => match entry.as_ref() {
                crate::analysis::AnalysisEntry::Pending => {
                    runtime.sampler_analysis_status[track].store(1, Ordering::Release);
                    runtime.sampler_onset_ptr_lo[track].store(0, Ordering::Release);
                    runtime.sampler_onset_ptr_hi[track].store(0, Ordering::Release);
                }
                crate::analysis::AnalysisEntry::Ready(result) => {
                    runtime.sampler_analysis_bpm[track]
                        .store(result.bpm.to_bits(), Ordering::Release);
                    if let Some(slot) = self.state.pattern.instrument_slots.get(track) {
                        if (slot.defaults.get(11) - 120.0).abs() < 0.001 && result.bpm > 0.0 {
                            slot.defaults.set(11, result.bpm.clamp(20.0, 400.0));
                        }
                    }
                    if let Some(table) = self.sample_analysis.cache().table(buffer_id) {
                        let (lo, hi) = crate::analysis::pack_ptr(Arc::as_ptr(&table));
                        runtime.sampler_onset_ptr_lo[track].store(lo.to_bits(), Ordering::Release);
                        runtime.sampler_onset_ptr_hi[track].store(hi.to_bits(), Ordering::Release);
                    }
                    runtime.sampler_analysis_status[track].store(2, Ordering::Release);
                }
                crate::analysis::AnalysisEntry::Failed(_) => {
                    runtime.sampler_analysis_status[track].store(3, Ordering::Release);
                    runtime.sampler_onset_ptr_lo[track].store(0, Ordering::Release);
                    runtime.sampler_onset_ptr_hi[track].store(0, Ordering::Release);
                }
            },
            None => {
                runtime.sampler_analysis_status[track].store(0, Ordering::Release);
                runtime.sampler_onset_ptr_lo[track].store(0, Ordering::Release);
                runtime.sampler_onset_ptr_hi[track].store(0, Ordering::Release);
            }
        }
    }

    pub fn capture_bus_pattern_snapshot(&self) -> Vec<BusPatternSnapshot> {
        self.buses
            .iter()
            .map(|bus| BusPatternSnapshot {
                id: bus.id,
                gate_sequence: bus.gate_sequence.clone(),
                effect_plocks: bus
                    .effect_slots
                    .iter()
                    .map(|slot| slot.plocks.clone())
                    .collect(),
                effect_defaults: bus
                    .effect_slots
                    .iter()
                    .map(|slot| slot.defaults.clone())
                    .collect(),
            })
            .collect()
    }

    pub fn restore_bus_pattern_snapshot(&mut self, snapshot: &[BusPatternSnapshot]) {
        for bus in &mut self.buses {
            let Some(saved) = snapshot.iter().find(|saved| saved.id == bus.id) else {
                bus.gate_sequence = BusGateSequence::default();
                for slot in &mut bus.effect_slots {
                    slot.plocks = (0..MAX_STEPS)
                        .map(|_| vec![None; slot.num_params as usize])
                        .collect();
                }
                continue;
            };
            bus.gate_sequence = saved.gate_sequence.clone();
            let descriptors = &bus.effect_descriptors;
            for (slot_idx, slot) in bus.effect_slots.iter_mut().enumerate() {
                // Recall per-scene base parameter values. Legacy snapshots (and
                // slots missing from the saved set) carry no defaults, so the
                // slot's current values are left untouched.
                if let Some(saved_defaults) = saved.effect_defaults.get(slot_idx) {
                    for (param_idx, value) in saved_defaults.iter().copied().enumerate() {
                        if param_idx < slot.defaults.len() {
                            let restored = restored_bus_effect_default(
                                descriptors.get(slot_idx),
                                param_idx,
                                value,
                            );
                            slot.defaults[param_idx] = restored;
                        }
                    }
                }
                let Some(saved_plocks) = saved.effect_plocks.get(slot_idx) else {
                    slot.plocks = (0..MAX_STEPS)
                        .map(|_| vec![None; slot.num_params as usize])
                        .collect();
                    continue;
                };
                let param_count = slot.num_params as usize;
                slot.plocks = (0..MAX_STEPS)
                    .map(|step| {
                        let mut values = vec![None; param_count];
                        if let Some(saved_step) = saved_plocks.get(step) {
                            for (param_idx, value) in saved_step.iter().copied().enumerate() {
                                if param_idx < values.len() {
                                    values[param_idx] = value.map(|value| {
                                        restored_bus_effect_default(
                                            descriptors.get(slot_idx),
                                            param_idx,
                                            value,
                                        )
                                    });
                                }
                            }
                        }
                        values
                    })
                    .collect();
            }
        }
        self.publish_bus_gate_runtime();
        // Push the recalled base values to the live audio nodes so the scene's
        // effect settings take immediately (not just on the next gate step).
        for bus_idx in 0..self.buses.len() {
            let slot_count = self.buses[bus_idx].effect_slots.len();
            for slot_idx in 0..slot_count {
                self.push_bus_effect_slot_defaults(bus_idx, slot_idx);
            }
        }
    }

    pub fn save_current_bus_pattern(&self) {
        self.state
            .save_current_bus_pattern_snapshot(self.capture_bus_pattern_snapshot());
    }

    pub fn ensure_bus_pattern_bank_len(&mut self, len: usize) {
        let default_snapshot = self.capture_bus_pattern_snapshot();
        self.state
            .ensure_bus_pattern_repository_len(len, &default_snapshot);
    }

    pub fn switch_bus_pattern(&mut self, new_idx: usize) {
        self.save_current_bus_pattern();
        let default_snapshot = self.capture_bus_pattern_snapshot();
        let snapshot = self
            .state
            .bus_pattern_snapshot_or_default(new_idx, &default_snapshot);
        self.restore_bus_pattern_snapshot(&snapshot);
    }

    pub fn clone_bus_pattern_from_to(&mut self, source_idx: usize, new_idx: usize) {
        self.save_current_bus_pattern();
        let default_snapshot = self.capture_bus_pattern_snapshot();
        let source = self
            .state
            .clone_bus_pattern_snapshot(source_idx, new_idx, &default_snapshot);
        self.restore_bus_pattern_snapshot(&source);
    }

    pub fn delete_bus_pattern_at(&mut self, deleted_idx: usize, new_idx: usize) {
        let default_snapshot = self.capture_bus_pattern_snapshot();
        let snapshot =
            self.state
                .delete_bus_pattern_snapshot(deleted_idx, new_idx, &default_snapshot);
        self.restore_bus_pattern_snapshot(&snapshot);
    }

    pub fn publish_bus_gate_runtime(&self) {
        let runtime = self
            .buses
            .iter()
            .filter_map(|bus| {
                let nodes = self
                    .graph
                    .bus_node_ids
                    .iter()
                    .find(|nodes| nodes.id == bus.id)?;
                let mut effect_slots = bus.effect_slots.clone();
                for (slot_idx, slot) in effect_slots.iter_mut().enumerate() {
                    let count = (slot.num_params as usize).min(slot.defaults.len());
                    for param_idx in 0..count {
                        let raw_idx = slot
                            .param_node_indices
                            .get(param_idx)
                            .copied()
                            .unwrap_or(param_idx as u32);
                        let param_id = crate::neural::ParamNodeId::from_slot_param(
                            slot.node_id,
                            slot.modulator_node_id,
                            raw_idx,
                        );
                        let key = crate::macro_engine::MacroParamKey::for_bus_effect(
                            bus.id, slot_idx, param_idx, param_id,
                        );
                        slot.defaults[param_idx] = self
                            .macro_engine
                            .effective_value(&key, slot.defaults[param_idx]);
                    }
                }
                Some(BusGateRuntimeState {
                    id: bus.id,
                    gate_id: nodes.gate_id,
                    sequence: bus.gate_sequence.clone(),
                    effect_slots,
                })
            })
            .collect();
        *self.graph.bus_gate_runtime.lock().unwrap() = runtime;

        let mut playheads = self.graph.bus_gate_playheads.lock().unwrap();
        let next_playheads = self
            .buses
            .iter()
            .map(|bus| {
                let step = playheads
                    .iter()
                    .find(|(id, _)| *id == bus.id)
                    .map(|(_, step)| *step)
                    .unwrap_or(0);
                (bus.id, step)
            })
            .collect();
        *playheads = next_playheads;
    }

    pub fn new(
        state: Arc<SequencerState>,
        lg: LiveGraphPtr,
        sample_rate: u32,
        buses: AudioBuses,
        master_recorder: Arc<MasterRecorder>,
        keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
    ) -> Self {
        let has_tracks = state.active_track_count() > 0;
        let focused_region = if has_tracks {
            Region::Cirklon
        } else {
            Region::Sidebar
        };
        let sidebar_mode = if has_tracks {
            SidebarMode::Audition
        } else {
            SidebarMode::InstrumentPicker
        };
        let sidebar_tab = if has_tracks {
            SidebarTab::Tools
        } else {
            SidebarTab::Sounds
        };
        let provider_state = AgentProviderState::from_env();
        let load_error = match AgentToolRuntime::load_default() {
            Ok(_) => None,
            Err(error) => Some(error),
        };
        let browser_tree = BrowserNode::scan_root("samples");

        let mut app = Self {
            state,
            track_registry: crate::sequencer::TrackRegistry::default(),
            device_registry: DeviceIdentityRegistry::default(),
            history: history::UndoManager::default(),
            macro_engine: crate::macro_engine::MacroEngine::default(),
            scene_macro_runtime: HashMap::new(),
            tracks: Vec::new(),
            track_colors: Vec::new(),
            track_collapsed: Vec::new(),
            buses: BusChannelState::default_buses(),
            groups: Vec::new(),
            sampler_paths: Vec::new(),
            rack_selected_slots: Vec::new(),
            rack_pad_bank_starts: Vec::new(),
            sample_path_registry: HashMap::new(),
            sample_buffer_path_registry: HashMap::new(),
            current_project_name: None,
            ui: UiState {
                cursor_step: 0,
                cursor_track: 0,
                active_param: StepParam::Velocity,
                input_mode: InputMode::Normal,
                value_buffer: String::new(),
                selection_anchor: None,
                track_selection_anchor: None,
                track_drag_anchor: None,
                step_drag_anchor: None,
                visual_steps: HashSet::new(),
                should_quit: false,
                focused_region,
                sidebar_tab,
                sidebar_mode,
                params_column: 1,
                tools_cursor: 0,
                tools_scroll_offset: 0,
                effect_tab: EffectTab::Slot(0),
                effect_tab_cursor: 0,
                effect_param_cursor: 0,
                effect_scroll_offset: 0,
                dropdown_open: false,
                dropdown_cursor: 0,
                track_param_dropdown: false,
                last_step_click: None,
                last_x_press: None,
                pattern_clone_pending: false,
                pattern_page: 0,
                pattern_btn_layout: Vec::new(),
                page_btn_layout: Vec::new(),
                bpm_entry: false,
                reverb_param_cursor: 0,
                reverb_size: 0.2,
                reverb_brightness: 0.8,
                reverb_replace: 0.3,
                recording: false,
                keyboard_octave: 0,
                held_notes: Vec::new(),
                piano_notes: Vec::new(),
                piano_last_step: usize::MAX,
                piano_last_track: usize::MAX,
                piano_lo: -12,
                follow_override_until: None,
                instrument_picker_cursor: 0,
                instrument_param_cursor: 0,
                synth_scroll_offset: 0,
                mod_param_cursor: 0,
                mod_scroll_offset: 0,
                source_param_cursor: 0,
                source_scroll_offset: 0,
                preset_prompt_kind: PresetPromptKind::SaveNew,
                param_mouse_drag: None,
                step_clipboard: None,
                master_recording: false,
                sidebar_search_focused: false,
            },
            editor: EditorState {
                pending_editor: None,
                pending_compile: None,
                pending_project_load: None,
                dylib_cache: DylibCacheManager::workspace_default(),
                lisp_libs: Vec::new(),
                effect_chain_leases: fx_chain::FxChainLeaseStore::default(),
                instrument_libs: Vec::new(),
                instrument_lib_leases: Vec::new(),
                picker_cursor: 0,
                picker_filter: String::new(),
                picker_items: Vec::new(),
                status_message: None,
                engine_registry: EngineRegistry::default(),
                scratch_buffer: String::new(),
                scratch_cursor: (0, 0),
                scratch_runtime: None,
                hooks: Vec::new(),
                pending_hook_invocations: VecDeque::new(),
                next_hook_id: 1,
                next_hook_callback_id: 1,
                last_hook_step_16th: None,
            },
            browser: BrowserState {
                tree: browser_tree,
                cursor: 0,
                filter: String::new(),
                scroll_offset: 0,
            },
            preset_browser: PresetBrowserState {
                cursor: 0,
                filter: String::new(),
                scroll_offset: 0,
            },
            agent_panel: AgentPanelState {
                kind: AgentKind::General,
                provider_state,
                transcript: Vec::new(),
                conversation: Vec::new(),
                input_buffer: String::new(),
                input_cursor: 0,
                scroll_offset: 0,
                auto_retry_budget: 0,
                model_dropdown_open: false,
                model_dropdown_cursor: 0,
                current_effect_artifact: None,
                pending_request: None,
                load_error,
            },
            agent_store: ConversationStore::new(sample_rate),
            master_recorder,
            sample_analysis: AnalysisService::new(),
            pending_recording_take: None,
            recording_history: None,
            graph: GraphState {
                lg,
                track_node_ids: Vec::new(),
                sample_rate,
                bus_l_id: buses.bus_l_id,
                bus_r_id: buses.bus_r_id,
                bus_node_ids: buses.default_bus_nodes,
                bus_gate_runtime: buses.bus_gate_runtime,
                bus_gate_playheads: buses.bus_gate_playheads,
                reverb_bus_id: buses.reverb_bus_id,
                reverb_node_id: buses.reverb_node_id,
                track_buffer_ids: Vec::new(),
                track_sample_rates: Vec::new(),
                track_voice_lids: Vec::new(),
                track_instrument_types: Vec::new(),
                track_instrument_run_modes: Vec::new(),
                track_engine_ids: Vec::new(),
                track_synth_node_ids: Vec::new(),
                track_gatepitch_node_ids: Vec::new(),
                engine_node_ids: Vec::new(),
                effect_descriptors: Vec::new(),
                instrument_descriptors: Vec::new(),
                record_armed: Vec::new(),
                keyboard_tx,
                applied_mod_routes: Vec::new(),
                deferred_rack_teardowns: Vec::new(),
            },
        };
        app.ensure_bus_pattern_bank_len(1);
        app.publish_bus_gate_runtime();
        app
    }

    fn selected_range(&self) -> (usize, usize) {
        match self.ui.selection_anchor {
            Some(anchor) => {
                let lo = anchor.min(self.ui.cursor_step);
                let hi = anchor.max(self.ui.cursor_step);
                (lo, hi)
            }
            None => (self.ui.cursor_step, self.ui.cursor_step),
        }
    }

    fn has_selection(&self) -> bool {
        self.ui.selection_anchor.is_some() || !self.ui.visual_steps.is_empty()
    }

    fn clear_step_selection(&mut self) {
        self.ui.selection_anchor = None;
        self.ui.visual_steps.clear();
    }

    fn track_selected_range(&self) -> (usize, usize) {
        match self.ui.track_selection_anchor {
            Some(anchor) => {
                let lo = anchor.min(self.ui.cursor_track);
                let hi = anchor.max(self.ui.cursor_track);
                (lo, hi)
            }
            None => (self.ui.cursor_track, self.ui.cursor_track),
        }
    }

    fn has_track_selection(&self) -> bool {
        self.ui.track_selection_anchor.is_some()
    }

    fn selected_tracks(&self) -> Vec<usize> {
        let (lo, hi) = self.track_selected_range();
        (lo..=hi).collect()
    }

    pub(super) fn effective_sidebar_mode(&self) -> SidebarMode {
        match self.ui.sidebar_mode {
            SidebarMode::InstrumentPicker | SidebarMode::AddTrack => self.ui.sidebar_mode,
            _ => {
                if !self.tracks.is_empty() && !self.is_sampler_track(self.ui.cursor_track) {
                    SidebarMode::Presets
                } else {
                    SidebarMode::Audition
                }
            }
        }
    }

    pub(super) fn focus_sidebar_sounds(&mut self) {
        self.ui.sidebar_tab = SidebarTab::Sounds;
        self.ui.focused_region = Region::Sidebar;
        self.ui.sidebar_search_focused = true;
    }

    pub(super) fn selected_agent_provider(
        &self,
    ) -> Option<&crate::agent::providers::ProviderAvailability> {
        self.agent_panel
            .provider_state
            .providers
            .iter()
            .find(|entry| entry.provider == self.agent_panel.provider_state.selected_provider)
    }

    pub(super) fn agent_model_options(
        &self,
    ) -> Vec<(crate::agent::providers::AgentProviderKind, String)> {
        let mut flattened = Vec::new();
        for provider in &self.agent_panel.provider_state.providers {
            for model in &provider.available_models {
                flattened.push((provider.provider, model.id.clone()));
            }
        }
        flattened
    }

    pub(super) fn selected_agent_model_index(&self) -> Option<usize> {
        let state = &self.agent_panel.provider_state;
        self.agent_model_options()
            .iter()
            .position(|(provider, model)| {
                *provider == state.selected_provider
                    && state
                        .providers
                        .iter()
                        .find(|entry| entry.provider == *provider)
                        .map(|entry| entry.selected_model == *model)
                        .unwrap_or(false)
            })
    }

    pub(super) fn select_agent_model_index(&mut self, index: usize) {
        let flattened = self.agent_model_options();
        let Some((provider_kind, model_id)) = flattened.get(index) else {
            return;
        };
        let state = &mut self.agent_panel.provider_state;
        state.selected_provider = *provider_kind;
        if let Some(provider) = state
            .providers
            .iter_mut()
            .find(|entry| entry.provider == *provider_kind)
        {
            provider.selected_model = model_id.clone();
        }
        self.agent_panel.model_dropdown_cursor = index;
    }

    pub(super) fn submit_agent_prompt(&mut self) -> Result<(), String> {
        if self.agent_panel.pending_request.is_some() {
            return Err("Agent request already in flight.".to_string());
        }
        let prompt = self.agent_panel.input_buffer.trim().to_string();
        if prompt.is_empty() {
            return Err("Agent prompt is empty.".to_string());
        }

        if prompt == "/new" {
            self.clear_agent_session();
            return Ok(());
        }

        self.agent_panel.transcript.push(AgentTranscriptEntry {
            role: "user".to_string(),
            text: prompt.clone(),
        });
        self.agent_panel.scroll_offset = 0;
        self.agent_panel.conversation.push(AgentMessage {
            role: AgentMessageRole::User,
            content: prompt.clone(),
            tool_name: None,
            reasoning_content: None,
        });
        self.agent_panel.input_buffer.clear();
        self.agent_panel.input_cursor = 0;
        self.agent_panel.auto_retry_budget = 1;

        self.start_agent_request()
    }

    fn current_agent_system_prompt(&self) -> String {
        match self.agent_panel.kind {
            AgentKind::General => include_str!("../agent/prompts/general.md"),
            AgentKind::Instrument => include_str!("../agent/prompts/instrument.md"),
            AgentKind::Effect => include_str!("../agent/prompts/effect.md"),
        }
        .to_string()
    }

    fn current_agent_session_context(&self) -> AgentSessionContext {
        let current_track_index = (!self.tracks.is_empty()).then_some(self.ui.cursor_track);
        let current_track_name = self.tracks.get(self.ui.cursor_track).cloned();
        let current_instrument_name = current_track_index.and_then(|track| {
            if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Custom) {
                return None;
            }
            self.graph
                .track_engine_ids
                .get(track)
                .and_then(|engine_id| *engine_id)
                .and_then(|engine_id| self.editor.engine_registry.get(engine_id))
                .map(|engine| engine.name.clone())
                .or_else(|| current_track_name.clone())
        });
        let current_instrument_source = current_instrument_name
            .as_deref()
            .and_then(|name| crate::lisp_host::load_instrument_source(name).ok());
        let current_effect_slot = self
            .selected_effect_slot()
            .filter(|slot| !self.tracks.is_empty() && *slot >= crate::effects::BUILTIN_SLOT_COUNT);
        let current_effect_name = current_effect_slot.and_then(|slot| {
            self.graph
                .effect_descriptors
                .get(self.ui.cursor_track)
                .and_then(|descs| descs.get(slot))
                .map(|desc| desc.name.clone())
        });
        let current_effect_source = current_effect_name
            .as_deref()
            .and_then(|name| crate::lisp_host::load_effect_source(name).ok());
        let current_effect_ui_source = current_effect_name
            .as_deref()
            .and_then(|name| crate::lisp_host::load_effect_ui_source(name).ok());
        let current_instrument_preset_schema = self.current_agent_instrument_preset_schema(
            current_track_index,
            current_instrument_name.as_deref(),
        );
        AgentSessionContext {
            has_tracks: !self.tracks.is_empty(),
            current_track_name,
            current_track_index,
            can_apply_effect_to_current_track: self.next_free_custom_slot().is_some(),
            current_effect_name,
            current_effect_source: current_effect_source.clone(),
            current_effect_ui_source,
            current_effect_slot,
            can_update_current_effect: current_effect_source.is_some(),
            can_update_current_instrument: current_instrument_source.is_some(),
            current_instrument_name,
            current_instrument_source,
            current_instrument_preset_schema,
        }
    }

    fn start_agent_request(&mut self) -> Result<(), String> {
        if self.agent_panel.pending_request.is_some() {
            return Err("Agent request already in flight.".to_string());
        }

        let selected = self
            .selected_agent_provider()
            .ok_or_else(|| "No agent model selected.".to_string())?;
        let provider = selected.provider;
        let model = selected.selected_model.clone();
        let system_prompt = self.current_agent_system_prompt();
        let kind = self.agent_panel.kind;
        let conversation = self.agent_panel.conversation.clone();
        let session_context = self.current_agent_session_context();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = match crate::agent::network::AgentNetworkClient::load_default() {
                Ok(client) => client.execute_turn_with_progress_for_kind(
                    provider,
                    &model,
                    &system_prompt,
                    &conversation,
                    session_context,
                    kind,
                    None,
                ),
                Err(error) => Err(AgentTurnError {
                    message: error,
                    tool_outcomes: Vec::new(),
                }),
            };
            let _ = tx.send(result);
        });
        self.agent_panel.pending_request = Some(PendingAgentRequest {
            receiver: rx,
            started_at: Instant::now(),
        });
        Ok(())
    }

    pub(super) fn poll_agent_request(&mut self) {
        let Some(pending) = self.agent_panel.pending_request.as_ref() else {
            return;
        };
        match pending.receiver.try_recv() {
            Ok(Ok(result)) => {
                self.agent_panel.pending_request = None;
                let tool_count = result.tool_outcomes.len();
                let history_checkpoint = self.history.clone();
                let history_checkpoint_len = self.history.undo_len();
                let routed_intent = result
                    .pending_actions
                    .iter()
                    .any(|action| matches!(action, AgentAppAction::SetAgentIntent { .. }));
                let action_results = result
                    .pending_actions
                    .into_iter()
                    .map(|action| self.apply_agent_action(action))
                    .collect::<Vec<_>>();
                let mut action_errors = action_results
                    .iter()
                    .filter_map(|result| result.as_ref().err().cloned())
                    .collect::<Vec<_>>();
                if action_errors.is_empty() {
                    edit::squash_history_since(
                        self,
                        history_checkpoint_len,
                        "Agent authoring edit",
                    );
                } else if let Err(error) = edit::rollback_history_to(self, history_checkpoint) {
                    action_errors.push(format!(
                        "Agent edit rollback failed: {error:?}"
                    ));
                }
                self.record_agent_tool_outcomes(&result.tool_outcomes, Some(&action_results));
                if !result.text.trim().is_empty() {
                    self.agent_panel.transcript.push(AgentTranscriptEntry {
                        role: "assistant".to_string(),
                        text: result.text.clone(),
                    });
                    self.agent_panel.scroll_offset = 0;
                    self.agent_panel.conversation.push(AgentMessage {
                        role: AgentMessageRole::Assistant,
                        content: result.text,
                        tool_name: None,
                        reasoning_content: result.reasoning_content,
                    });
                }
                self.editor.status_message = Some((
                    format!(
                        "Agent response received{}",
                        if tool_count > 0 {
                            format!(" ({tool_count} tools)")
                        } else {
                            String::new()
                        }
                    ),
                    Instant::now(),
                ));
                for action_result in action_results {
                    let (role, text) = match action_result {
                        Ok(message) => ("system".to_string(), message),
                        Err(error) => ("error".to_string(), error),
                    };
                    self.agent_panel.conversation.push(AgentMessage {
                        role: AgentMessageRole::System,
                        content: text.clone(),
                        tool_name: None,
                        reasoning_content: None,
                    });
                    self.agent_panel
                        .transcript
                        .push(AgentTranscriptEntry { role, text });
                }
                self.agent_panel.scroll_offset = 0;

                if !action_errors.is_empty() && self.agent_panel.auto_retry_budget > 0 {
                    self.agent_panel.auto_retry_budget -= 1;
                    let repair_message = format!(
                        "Applying your last generated change failed with these errors:\n{}\nRevise the code and try again using the appropriate tool. Do not claim success unless the tool actually succeeds.",
                        action_errors.join("\n")
                    );
                    self.agent_panel.conversation.push(AgentMessage {
                        role: AgentMessageRole::System,
                        content: repair_message.clone(),
                        tool_name: None,
                        reasoning_content: None,
                    });
                    self.agent_panel.transcript.push(AgentTranscriptEntry {
                        role: "system".to_string(),
                        text: repair_message,
                    });
                    if let Err(error) = self.start_agent_request() {
                        self.editor.status_message =
                            Some((format!("Agent retry failed: {error}"), Instant::now()));
                    }
                } else if routed_intent && action_errors.is_empty() {
                    if let Err(error) = self.start_agent_request() {
                        self.agent_panel.transcript.push(AgentTranscriptEntry {
                            role: "system".to_string(),
                            text: format!("Failed to continue after routing: {error}"),
                        });
                        self.editor.status_message =
                            Some((format!("Agent routing failed: {error}"), Instant::now()));
                    }
                }
            }
            Ok(Err(error)) => {
                self.agent_panel.pending_request = None;
                self.record_agent_tool_outcomes(&error.tool_outcomes, None);
                self.agent_panel.transcript.push(AgentTranscriptEntry {
                    role: "error".to_string(),
                    text: error.message.clone(),
                });
                self.agent_panel.scroll_offset = 0;
                self.editor.status_message =
                    Some((format!("Agent error: {}", error.message), Instant::now()));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.agent_panel.pending_request = None;
                self.editor.status_message =
                    Some(("Agent worker crashed".to_string(), Instant::now()));
            }
        }
    }

    fn record_agent_tool_outcomes(
        &mut self,
        tool_outcomes: &[ToolCallOutcome],
        action_results: Option<&[Result<String, String>]>,
    ) {
        let mut action_result_idx = 0usize;
        for outcome in tool_outcomes {
            let action_count = outcome.pending_actions.len();
            let mut tool_ok = outcome.ok;
            let mut details = vec![outcome.summary.clone()];

            if !outcome.content.trim().is_empty()
                && outcome.content.trim() != outcome.summary.trim()
            {
                details.push(outcome.content.clone());
            }

            if let Some(action_results) = action_results.filter(|_| action_count > 0) {
                let end = action_result_idx + action_count;
                let related_results = &action_results[action_result_idx..end];
                action_result_idx = end;
                tool_ok = related_results.iter().all(|result| result.is_ok());
                for result in related_results {
                    match result {
                        Ok(message) => details.push(message.clone()),
                        Err(error) => details.push(error.clone()),
                    }
                }
            }

            let tool_text = format!(
                "{} [{}]\n{}",
                outcome.name,
                if tool_ok { "ok" } else { "error" },
                details.join("\n\n")
            );
            self.agent_panel.conversation.push(AgentMessage {
                role: AgentMessageRole::Tool,
                content: tool_text.clone(),
                tool_name: Some(outcome.name.clone()),
                reasoning_content: None,
            });
            self.agent_panel.transcript.push(AgentTranscriptEntry {
                role: "tool".to_string(),
                text: tool_text,
            });
        }
    }

    pub(super) fn scroll_agent_transcript(&mut self, delta: isize) {
        if delta > 0 {
            self.agent_panel.scroll_offset = self
                .agent_panel
                .scroll_offset
                .saturating_add(delta as usize);
        } else if delta < 0 {
            self.agent_panel.scroll_offset = self
                .agent_panel
                .scroll_offset
                .saturating_sub((-delta) as usize);
        }
    }

    pub(super) fn cancel_agent_request(&mut self) {
        if self.agent_panel.pending_request.take().is_some() {
            self.agent_panel.auto_retry_budget = 0;
            self.agent_panel.transcript.push(AgentTranscriptEntry {
                role: "system".to_string(),
                text: "Interrupted agent request.".to_string(),
            });
            self.agent_panel.scroll_offset = 0;
            self.editor.status_message =
                Some(("Agent request interrupted".to_string(), Instant::now()));
        }
    }

    fn clear_agent_session(&mut self) {
        self.agent_panel.pending_request = None;
        self.agent_panel.transcript.clear();
        self.agent_panel.conversation.clear();
        self.agent_panel.input_buffer.clear();
        self.agent_panel.input_cursor = 0;
        self.agent_panel.scroll_offset = 0;
        self.agent_panel.auto_retry_budget = 0;
        self.agent_panel.kind = AgentKind::General;
        self.agent_panel.model_dropdown_open = false;
        self.agent_panel.current_effect_artifact = None;
        self.editor.status_message = Some(("Cleared agent session".to_string(), Instant::now()));
    }

    fn create_agent_effect_artifact(
        &mut self,
        name: String,
        dsp_source: String,
        ui_source: String,
    ) -> Result<String, String> {
        self.validate_agent_effect_artifact(&name, &dsp_source, &ui_source)?;
        self.agent_panel.current_effect_artifact = Some(EffectDraft {
            name: name.clone(),
            dsp_source,
            ui_source,
        });
        Ok(format!("Created validated draft effect artifact '{name}'."))
    }

    fn update_agent_effect_artifact(
        &mut self,
        name: Option<String>,
        dsp_source: String,
        ui_source: String,
    ) -> Result<String, String> {
        let existing = self
            .agent_panel
            .current_effect_artifact
            .as_ref()
            .ok_or_else(|| "No draft effect artifact exists to update.".to_string())?;
        let name = name.unwrap_or_else(|| existing.name.clone());
        self.validate_agent_effect_artifact(&name, &dsp_source, &ui_source)?;
        self.agent_panel.current_effect_artifact = Some(EffectDraft {
            name: name.clone(),
            dsp_source,
            ui_source,
        });
        Ok(format!("Updated validated draft effect artifact '{name}'."))
    }

    fn validate_agent_effect_artifact(
        &self,
        name: &str,
        dsp_source: &str,
        ui_source: &str,
    ) -> Result<(), String> {
        validate_effect_dsp_source(dsp_source)
            .map_err(|error| format!("dsp.lisp validation error for '{name}':\n{error}"))?;
        let compile_result = crate::lisp_host::compile_and_load(dsp_source, self.graph.sample_rate)
            .map_err(|error| format!("compile error for '{name}':\n{error}"))?;
        validate_effect_ui_source(ui_source, &compile_result.manifest)
            .map_err(|error| format!("ui.lisp validation error for '{name}':\n{error}"))?;
        let audition = audition_loaded_effect(&compile_result, self.graph.sample_rate)
            .map_err(|error| format!("audition failed for '{name}':\n{error}"))?;
        let feedback = audition_feedback(&audition);
        if audition.silent || audition.clipped || audition.differs_from_input == Some(false) {
            return Err(feedback);
        }
        Ok(())
    }

    fn finalize_agent_effect_artifact(&mut self, name: String) -> Result<String, String> {
        let artifact = self
            .agent_panel
            .current_effect_artifact
            .clone()
            .ok_or_else(|| "No validated draft effect artifact exists to finalize.".to_string())?;
        let final_name = format!("{}/", name.trim_end_matches('/'));
        if crate::lisp_host::effect_source_path(&final_name).exists()
            || crate::lisp_host::effect_ui_path(&final_name).exists()
        {
            return Err(format!(
                "Effect '{}' already exists.",
                name.trim_end_matches('/')
            ));
        }
        self.validate_agent_effect_artifact(
            &final_name,
            &artifact.dsp_source,
            &artifact.ui_source,
        )?;
        self.save_effect_artifact_sources_with_rollback(
            &final_name,
            &artifact.dsp_source,
            &artifact.ui_source,
        )?;
        Ok(format!("Finalized effect artifact as '{final_name}'."))
    }

    fn save_effect_artifact_sources_with_rollback(
        &self,
        name: &str,
        dsp_source: &str,
        ui_source: &str,
    ) -> Result<(), String> {
        let previous_source = crate::lisp_host::load_effect_source(name).ok();
        let previous_ui = crate::lisp_host::load_effect_ui_source(name).ok();
        crate::lisp_host::save_effect(name, dsp_source)
            .map_err(|error| format!("Failed to save effect '{}': {error}", name))?;
        if let Err(error) = crate::lisp_host::save_effect_ui(name, ui_source) {
            self.restore_effect_source(name, previous_source.as_deref())?;
            self.restore_effect_ui_source(name, previous_ui.as_deref())?;
            return Err(format!("Failed to save effect UI '{}': {error}", name));
        }
        Ok(())
    }

    fn apply_agent_action(&mut self, action: AgentAppAction) -> Result<String, String> {
        match action {
            AgentAppAction::SetAgentIntent { kind } => {
                if kind == AgentKind::General {
                    return Err("General is not a focused agent intent.".to_string());
                }
                self.agent_panel.kind = kind;
                Ok(match kind {
                    AgentKind::Instrument => {
                        "Intent selected: instrument. Continuing with the focused instrument agent."
                            .to_string()
                    }
                    AgentKind::Effect => {
                        "Intent selected: effect. Continuing with the focused effect agent."
                            .to_string()
                    }
                    AgentKind::General => unreachable!(),
                })
            }
            AgentAppAction::CreateInstrumentArtifact { .. } => Err(
                "create_instrument_artifact is only supported by the conversation agent panel."
                    .to_string(),
            ),
            AgentAppAction::UpdateInstrumentArtifact { .. } => Err(
                "update_instrument_artifact is only supported by the conversation agent panel."
                    .to_string(),
            ),
            AgentAppAction::CreateEffectArtifact {
                name,
                dsp_source,
                ui_source,
            } => self.create_agent_effect_artifact(name, dsp_source, ui_source),
            AgentAppAction::UpdateEffectArtifact {
                name,
                dsp_source,
                ui_source,
            } => self.update_agent_effect_artifact(name, dsp_source, ui_source),
            AgentAppAction::FinalizeEffectArtifact { name } => {
                self.finalize_agent_effect_artifact(name)
            }
            AgentAppAction::CreateInstrumentTrack { name, source } => {
                let previous_source = crate::lisp_host::load_instrument_source(&name).ok();
                crate::lisp_host::save_instrument(&name, &source)
                    .map_err(|error| format!("Failed to save instrument '{}': {error}", name))?;
                let track_idx = match self.add_saved_instrument_track_sync(&name) {
                    Ok(track_idx) => track_idx,
                    Err(error) => {
                        self.restore_instrument_source(&name, previous_source.as_deref())?;
                        return Err(error);
                    }
                };
                Ok(format!(
                    "Created instrument track '{}' at track {}.",
                    name,
                    track_idx + 1
                ))
            }
            AgentAppAction::ApplyEffectToCurrentTrack { name, source } => {
                if self.tracks.is_empty() {
                    return Err(
                        "No current track is available. Create a track first, then apply the effect."
                            .to_string(),
                    );
                }
                let track = self.ui.cursor_track;
                let slot_idx = self.next_free_custom_slot().ok_or_else(|| {
                    format!(
                        "Track '{}' has no free custom effect slot.",
                        self.tracks
                            .get(track)
                            .cloned()
                            .unwrap_or_else(|| "current track".to_string())
                    )
                })?;
                let previous_source = crate::lisp_host::load_effect_source(&name).ok();
                crate::lisp_host::save_effect(&name, &source)
                    .map_err(|error| format!("Failed to save effect '{}': {error}", name))?;
                if let Err(error) = self.load_saved_effect_to_slot_recorded(track, slot_idx, &name) {
                    self.restore_effect_source(&name, previous_source.as_deref())?;
                    return Err(error);
                }
                Ok(format!(
                    "Applied effect '{}' to track '{}' in slot {}.",
                    name,
                    self.tracks
                        .get(track)
                        .cloned()
                        .unwrap_or_else(|| "current track".to_string()),
                    slot_idx + 1
                ))
            }
            AgentAppAction::UpdateCurrentEffect { name, source } => {
                let previous_source = crate::lisp_host::load_effect_source(&name).ok();
                if let Err(error) = self.replace_current_effect_sync(&name, &source) {
                    self.restore_effect_source(&name, previous_source.as_deref())?;
                    return Err(error);
                }
                Ok(format!("Updated current effect to '{}'.", name))
            }
            AgentAppAction::UpdateCurrentInstrument { name, source } => {
                let previous_source = crate::lisp_host::load_instrument_source(&name).ok();
                crate::lisp_host::save_instrument(&name, &source)
                    .map_err(|error| format!("Failed to save instrument '{}': {error}", name))?;
                if let Err(error) = self.replace_current_custom_instrument_sync(&name, &source) {
                    self.restore_instrument_source(&name, previous_source.as_deref())?;
                    return Err(error);
                }
                Ok(format!("Updated current instrument track to '{}'.", name))
            }
            AgentAppAction::SaveCurrentInstrumentPresets {
                instrument_name,
                presets,
            } => self.save_agent_instrument_presets(&instrument_name, &presets),
        }
    }

    fn current_agent_instrument_preset_schema(
        &self,
        current_track_index: Option<usize>,
        current_instrument_name: Option<&str>,
    ) -> Option<AgentInstrumentPresetSchema> {
        let track = current_track_index?;
        let instrument_name = current_instrument_name?;
        let desc = self.graph.instrument_descriptors.get(track)?;
        let slot = self.state.pattern.instrument_slots.get(track)?;
        let existing_presets = crate::lisp_host::load_instrument_presets(instrument_name)
            .map(|presets| presets.into_iter().map(|preset| preset.name).collect())
            .unwrap_or_default();
        let synth_indices = self.synth_param_indices(track);
        let mod_indices = self.mod_param_indices(track);
        let source_indices = self.source_param_actual_indices(track);

        let mut params = Vec::new();
        for (group, indices) in [
            ("synth", synth_indices),
            ("mod", mod_indices),
            ("source", source_indices),
        ] {
            for idx in indices {
                let Some(param) = desc.params.get(idx) else {
                    continue;
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
        }

        Some(AgentInstrumentPresetSchema {
            instrument_name: instrument_name.to_string(),
            source_file: Some(format!("instruments/{instrument_name}.lisp")),
            base_note_offset: self.instrument_base_note_offset(track),
            existing_presets,
            params,
        })
    }

    fn save_agent_instrument_presets(
        &mut self,
        instrument_name: &str,
        drafts: &[AgentInstrumentPresetDraft],
    ) -> Result<String, String> {
        let current_name = self
            .current_custom_instrument_name()
            .ok_or_else(|| "No current custom instrument track is selected.".to_string())?;
        if current_name != instrument_name {
            return Err(format!(
                "Current custom instrument is '{}', but the queued presets target '{}'. Re-select the intended instrument and try again.",
                current_name, instrument_name
            ));
        }
        let track = self.ui.cursor_track;
        let desc = self
            .current_instrument_descriptor()
            .ok_or_else(|| "No current instrument descriptor is available.".to_string())?;
        let existing =
            crate::lisp_host::load_instrument_presets(instrument_name).map_err(|error| {
                format!(
                    "Failed to load preset bank for '{}': {error}",
                    instrument_name
                )
            })?;
        let mut presets_by_name = existing
            .into_iter()
            .map(|preset| (preset.name.clone(), preset))
            .collect::<std::collections::BTreeMap<_, _>>();

        for draft in drafts {
            let mut params = std::collections::BTreeMap::new();
            let slot = &self.state.pattern.instrument_slots[track];
            for (idx, param) in desc.params.iter().enumerate() {
                params.insert(param.name.clone(), slot.defaults.get(idx));
            }
            for (param_name, value) in &draft.params {
                let (idx, param_desc) = desc
                    .params
                    .iter()
                    .enumerate()
                    .find(|(_, param)| param.name == *param_name)
                    .ok_or_else(|| {
                        format!(
                            "Preset '{}' references unknown parameter '{}'.",
                            draft.name, param_name
                        )
                    })?;
                validate_runtime_preset_value(&draft.name, param_name, *value, param_desc)?;
                let _ = idx;
                params.insert(param_name.clone(), *value);
            }
            let preset = crate::lisp_host::InstrumentPreset {
                id: draft.name.clone(),
                name: draft.name.clone(),
                base_note_offset: draft
                    .base_note_offset
                    .unwrap_or_else(|| self.instrument_base_note_offset(track)),
                params,
                key_locks: crate::effects::capture_key_locks_by_param_name(slot, desc),
            };
            presets_by_name.insert(draft.name.clone(), preset);
        }

        let mut presets = presets_by_name.into_values().collect::<Vec<_>>();
        presets.sort_by(|a, b| a.name.cmp(&b.name));
        crate::lisp_host::save_instrument_presets(instrument_name, &presets).map_err(|error| {
            format!(
                "Failed to save preset bank for '{}': {error}",
                instrument_name
            )
        })?;
        Ok(format!(
            "Saved {} preset(s) for '{}': {}.",
            drafts.len(),
            instrument_name,
            drafts
                .iter()
                .map(|preset| preset.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    fn restore_instrument_source(
        &self,
        name: &str,
        previous_source: Option<&str>,
    ) -> Result<(), String> {
        match previous_source {
            Some(source) => crate::lisp_host::save_instrument(name, source)
                .map_err(|error| format!("Failed to restore instrument '{}': {error}", name)),
            None => std::fs::remove_file(format!("instruments/{name}.lisp"))
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|error| format!("Failed to remove instrument '{}': {error}", name)),
        }
    }

    fn restore_effect_source(
        &self,
        name: &str,
        previous_source: Option<&str>,
    ) -> Result<(), String> {
        match previous_source {
            Some(source) => crate::lisp_host::save_effect(name, source)
                .map_err(|error| format!("Failed to restore effect '{}': {error}", name)),
            None => std::fs::remove_file(crate::lisp_host::effect_source_path(name))
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|error| format!("Failed to remove effect '{}': {error}", name)),
        }
    }

    fn restore_effect_ui_source(
        &self,
        name: &str,
        previous_source: Option<&str>,
    ) -> Result<(), String> {
        match previous_source {
            Some(source) => crate::lisp_host::save_effect_ui(name, source)
                .map_err(|error| format!("Failed to restore effect UI '{}': {error}", name)),
            None => std::fs::remove_file(crate::lisp_host::effect_ui_path(name))
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|error| format!("Failed to remove effect UI '{}': {error}", name)),
        }
    }

    /// Return all selected step indices (visual or contiguous range, falls back to cursor).
    fn selected_steps(&self) -> Vec<usize> {
        if !self.ui.visual_steps.is_empty() {
            let mut steps: Vec<usize> = self.ui.visual_steps.iter().copied().collect();
            steps.sort();
            steps
        } else {
            let (lo, hi) = self.selected_range();
            (lo..=hi).collect()
        }
    }

    fn num_steps(&self) -> usize {
        if self.tracks.is_empty() {
            STEPS_PER_PAGE
        } else {
            self.state.pattern.track_params[self.ui.cursor_track].get_num_steps()
        }
    }

    fn current_page(&self) -> usize {
        self.ui.cursor_step / STEPS_PER_PAGE
    }

    /// Page to display: follows playhead when playing, unless the user recently
    /// interacted or has a selection active.
    fn display_page(&self) -> usize {
        if !self.state.is_playing() {
            return self.current_page();
        }
        // Selection active → stay on cursor page
        if self.ui.selection_anchor.is_some() {
            return self.current_page();
        }
        // User recently interacted → stay on cursor page
        if let Some(until) = self.ui.follow_override_until {
            if Instant::now() < until {
                return self.current_page();
            }
        }
        // Follow playhead
        let ns = self.num_steps();
        let ph = self.state.track_step(self.ui.cursor_track) % ns;
        ph / STEPS_PER_PAGE
    }

    fn page_range(&self) -> (usize, usize) {
        let page = self.display_page();
        let page_start = page * STEPS_PER_PAGE;
        let page_end = (page_start + STEPS_PER_PAGE).min(self.num_steps());
        (page_start, page_end)
    }

    /// Pause page-follow for 5 seconds after user interaction.
    fn touch_follow_timer(&mut self) {
        self.ui.follow_override_until = Some(Instant::now() + std::time::Duration::from_secs(5));
    }

    /// Whether the given track is a Sampler instrument.
    pub fn is_sampler_track(&self, track: usize) -> bool {
        track >= self.graph.track_instrument_types.len()
            || self.graph.track_instrument_types[track] == InstrumentType::Sampler
    }

    fn selected_effect_slot(&self) -> Option<usize> {
        match self.ui.effect_tab {
            EffectTab::Slot(idx) => Some(idx),
            EffectTab::Synth | EffectTab::Mod | EffectTab::Sources | EffectTab::Reverb => None,
        }
    }

    /// Clamp cursor_step to the current track's num_steps.
    fn clamp_cursor_to_steps(&mut self) {
        let ns = self.num_steps();
        if self.ui.cursor_step >= ns {
            self.ui.cursor_step = ns - 1;
        }
    }

    pub fn register_sample_path(&mut self, sample_name: &str, path: PathBuf) {
        self.sample_path_registry
            .insert(sample_name.to_string(), path);
    }

    pub fn register_loaded_sample_path(
        &mut self,
        sample_name: &str,
        buffer_id: i32,
        path: PathBuf,
    ) {
        self.register_sample_path(sample_name, path.clone());
        if buffer_id >= 0 {
            self.sample_buffer_path_registry.insert(buffer_id, path);
        }
    }

    pub fn sampler_path_for_track(&self, track: usize) -> Option<PathBuf> {
        self.sampler_paths
            .get(track)
            .and_then(|path| path.as_ref())
            .cloned()
            .or_else(|| {
                self.graph
                    .track_buffer_ids
                    .get(track)
                    .and_then(|buffer_id| self.sample_buffer_path_registry.get(buffer_id))
                    .cloned()
            })
            .or_else(|| {
                self.tracks
                    .get(track)
                    .and_then(|name| self.sample_path_registry.get(name))
                    .cloned()
            })
    }

    pub(super) fn sync_sampler_path_from_sample(
        &mut self,
        track: usize,
        buffer_id: i32,
        sample_name: &str,
    ) {
        if track >= self.sampler_paths.len() {
            return;
        }
        self.sampler_paths[track] = self
            .sample_buffer_path_registry
            .get(&buffer_id)
            .cloned()
            .or_else(|| self.sample_path_registry.get(sample_name).cloned());
    }
}
