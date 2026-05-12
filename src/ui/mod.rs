use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::agent::actions::{
    AgentAppAction, AgentInstrumentParamSchema, AgentInstrumentPresetDraft,
    AgentInstrumentPresetSchema, AgentSessionContext,
};
use crate::agent::network::{AgentTurnError, AgentTurnResult};
use crate::agent::protocol::{AgentToolRuntime, ToolCallOutcome};
use crate::agent::providers::{AgentMessage, AgentMessageRole, AgentProviderState};
use crate::agent::store::ConversationStore;
use crate::analysis::{AnalysisJob, AnalysisService};
use crate::audiograph::LiveGraphPtr;
use crate::effects::{EffectDescriptor, EffectSlotSnapshot, ParamKind, ParamScaling};
use crate::lisp_effect::{DGenManifest, LoadedDGenLib, ScratchControlRuntime};
use crate::recorder::{MasterRecorder, RecordingTake};
use crate::sequencer::{
    BusId, InstrumentType, KeyboardTrigger, SequencerState, StepParam, StepSnapshot,
    SwingResolution, Timebase, MAX_STEPS, STEPS_PER_PAGE,
};

mod browser;
mod cirklon;
pub mod command;
mod draw;
mod effect_params;
mod effects;
mod effects_draw;
mod graph;
mod hooks;
mod input;
mod params;
mod projects;
mod recording;
mod synth;

pub use browser::BrowserNode;
#[allow(unused_imports)]
pub use command::{apply_command, AppCommand};
pub use draw::draw;

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
        track: usize,
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
    receiver: std::sync::mpsc::Receiver<Result<crate::lisp_effect::CompileResult, String>>,
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

pub struct EngineDescriptor {
    pub name: String,
    pub source: String,
    pub manifest: DGenManifest,
    pub lib_index: usize,
}

#[derive(Default)]
pub struct EngineRegistry {
    pub engines: Vec<EngineDescriptor>,
}

impl EngineRegistry {
    pub fn find_by_name_and_source(&self, name: &str, source: &str) -> Option<usize> {
        self.engines
            .iter()
            .position(|entry| entry.name == name && entry.source == source)
    }

    pub fn get(&self, engine_id: usize) -> Option<&EngineDescriptor> {
        self.engines.get(engine_id)
    }

    pub fn replace_at(&mut self, engine_id: usize, entry: EngineDescriptor) {
        if engine_id < self.engines.len() {
            self.engines[engine_id] = entry;
        }
    }

    pub fn upsert(&mut self, entry: EngineDescriptor) -> usize {
        if let Some(existing_idx) = self.find_by_name_and_source(&entry.name, &entry.source) {
            self.engines[existing_idx] = entry;
            existing_idx
        } else {
            self.engines.push(entry);
            self.engines.len() - 1
        }
    }
}

#[cfg(test)]
mod engine_registry_tests {
    use super::{EngineDescriptor, EngineRegistry};
    use crate::lisp_effect::DGenManifest;

    fn manifest() -> DGenManifest {
        DGenManifest {
            dylib_path: std::path::PathBuf::new(),
            total_memory_slots: 0,
            params: Vec::new(),
            inputs: Vec::new(),
            modulators: Vec::new(),
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
        });
        let second = registry.upsert(EngineDescriptor {
            name: "bass/".to_string(),
            source: "(out 0 1 @name audio)".to_string(),
            manifest: manifest(),
            lib_index: 1,
        });

        assert_eq!(first, second);
        assert_eq!(registry.engines.len(), 1);
        assert_eq!(registry.engines[first].lib_index, 1);
    }
}

pub struct EngineNodeIds {
    pub synth_ids: Vec<i32>,
    pub synth_outputs: usize,
    pub gatepitch_ids: Vec<i32>,
    pub modulator_ids: Vec<i32>,
    pub route_gain_ids: Vec<Vec<[i32; 2]>>,
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
    lisp_libs: Vec<LoadedDGenLib>,
    pub instrument_libs: Vec<LoadedDGenLib>,
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
    pub provider_state: AgentProviderState,
    pub transcript: Vec<AgentTranscriptEntry>,
    pub conversation: Vec<AgentMessage>,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub scroll_offset: usize,
    pub auto_retry_budget: usize,
    pub model_dropdown_open: bool,
    pub model_dropdown_cursor: usize,
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
    pub track_engine_ids: Vec<Option<usize>>,
    pub track_synth_node_ids: Vec<Vec<i32>>,
    pub track_gatepitch_node_ids: Vec<Vec<i32>>,
    pub engine_node_ids: Vec<Option<EngineNodeIds>>,
    pub effect_descriptors: Vec<Vec<EffectDescriptor>>,
    pub instrument_descriptors: Vec<EffectDescriptor>,
    pub record_armed: Vec<bool>,
    pub keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
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

#[derive(Default, Clone)]
pub struct LayoutRects {
    pub cirklon_area: ratatui::prelude::Rect,
    pub track_list: ratatui::prelude::Rect,
    pub param_tabs: ratatui::prelude::Rect,
    pub bars: ratatui::prelude::Rect,
    pub trigger_row: ratatui::prelude::Rect,
    pub track_params_inner: ratatui::prelude::Rect,
    pub effects_tabs: ratatui::prelude::Rect,
    pub effects_inner: ratatui::prelude::Rect,
    pub effects_block: ratatui::prelude::Rect,
    pub info_bar: ratatui::prelude::Rect,
    pub rec_button: ratatui::prelude::Rect,
    pub master_rec_button: ratatui::prelude::Rect,
    pub pattern_buttons_area: ratatui::prelude::Rect,
    pub page_blocks_area: ratatui::prelude::Rect,
    pub sidebar_tabs: ratatui::prelude::Rect,
    pub sidebar_inner: ratatui::prelude::Rect,
    pub piano_area: ratatui::prelude::Rect,
}

/// Per-track node IDs needed for graph rewiring.
#[derive(Clone)]
#[allow(dead_code)]
pub struct TrackNodeIds {
    pub sampler_ids: Vec<i32>, // up to MAX_VOICES
    pub voice_sum_id: i32,
    pub voice_sum_r_id: i32,
    pub pan_id: i32,
    pub filter_id: i32,
    pub delay_id: i32,
    pub send_id: i32,
    pub bus_send_ids: Vec<BusSendNodeIds>,
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

#[derive(Clone)]
pub struct BusPatternSnapshot {
    pub id: BusId,
    pub gate_sequence: BusGateSequence,
    pub effect_plocks: Vec<Vec<Vec<Option<f32>>>>,
}

#[derive(Clone)]
pub struct BusGateSequence {
    pub steps: [bool; MAX_STEPS],
    pub velocities: [f32; MAX_STEPS],
    pub durations: [f32; MAX_STEPS],
    pub syncs: [f32; MAX_STEPS],
    pub num_steps: usize,
    pub timebase: Timebase,
    pub swing: f32,
    pub swing_resolution: SwingResolution,
    pub timebase_plocks: [Option<Timebase>; MAX_STEPS],
    pub swing_plocks: [Option<f32>; MAX_STEPS],
    pub swing_resolution_plocks: [Option<SwingResolution>; MAX_STEPS],
}

impl Default for BusGateSequence {
    fn default() -> Self {
        Self {
            steps: [true; MAX_STEPS],
            velocities: [1.0; MAX_STEPS],
            durations: [1.0; MAX_STEPS],
            syncs: [0.0; MAX_STEPS],
            num_steps: 16,
            timebase: Timebase::Sixteenth,
            swing: 50.0,
            swing_resolution: SwingResolution::Sixteenth,
            timebase_plocks: [None; MAX_STEPS],
            swing_plocks: [None; MAX_STEPS],
            swing_resolution_plocks: [None; MAX_STEPS],
        }
    }
}

impl BusGateSequence {
    pub fn toggle_step(&mut self, step: usize) -> Option<bool> {
        let value = self.steps.get_mut(step)?;
        *value = !*value;
        Some(*value)
    }

    pub fn set_step_velocity(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.velocities.get_mut(step)?;
        *slot = value.clamp(0.0, 1.0);
        Some(*slot)
    }

    pub fn set_step_duration(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.durations.get_mut(step)?;
        *slot = value.clamp(0.1, 2.0);
        Some(*slot)
    }

    pub fn set_step_sync(&mut self, step: usize, value: f32) -> Option<f32> {
        let slot = self.syncs.get_mut(step)?;
        *slot = value
            .round()
            .clamp(0.0, (crate::sequencer::SYNC_COUNT - 1) as f32);
        Some(*slot)
    }

    pub fn set_num_steps(&mut self, value: usize) {
        self.num_steps = value.clamp(1, MAX_STEPS);
    }

    pub fn has_step_plock(&self, step: usize) -> bool {
        step < MAX_STEPS
            && (self.timebase_plocks[step].is_some()
                || self.swing_plocks[step].is_some()
                || self.swing_resolution_plocks[step].is_some())
    }
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
            custom_effect_names: vec![None; crate::lisp_effect::MAX_CUSTOM_FX],
        }
    }

    pub fn default_effect_descriptors() -> Vec<EffectDescriptor> {
        (0..crate::lisp_effect::MAX_CUSTOM_FX)
            .map(|_| EffectDescriptor::empty_custom_slot())
            .collect()
    }

    pub fn default_effect_slots() -> Vec<EffectSlotSnapshot> {
        (0..crate::lisp_effect::MAX_CUSTOM_FX)
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
    pub layout: LayoutRects,
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
    pub tracks: Vec<String>,
    pub buses: Vec<BusChannelState>,
    pub bus_pattern_bank: Vec<Vec<BusPatternSnapshot>>,
    pub sampler_paths: Vec<Option<PathBuf>>,
    pub sample_path_registry: HashMap<String, PathBuf>,
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
}

impl App {
    pub fn submit_sample_analysis(&self, loaded: &crate::sampler::LoadedSample) {
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
            for (slot_idx, slot) in bus.effect_slots.iter_mut().enumerate() {
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
                                    values[param_idx] = value;
                                }
                            }
                        }
                        values
                    })
                    .collect();
            }
        }
        self.publish_bus_gate_runtime();
    }

    pub fn save_current_bus_pattern(&mut self) {
        let current_pattern = self
            .state
            .pattern
            .current_pattern
            .load(std::sync::atomic::Ordering::Relaxed) as usize;
        let num_patterns = self
            .state
            .pattern
            .num_patterns
            .load(std::sync::atomic::Ordering::Relaxed) as usize;
        self.ensure_bus_pattern_bank_len(num_patterns.max(current_pattern + 1));
        self.bus_pattern_bank[current_pattern] = self.capture_bus_pattern_snapshot();
    }

    pub fn ensure_bus_pattern_bank_len(&mut self, len: usize) {
        while self.bus_pattern_bank.len() < len {
            self.bus_pattern_bank
                .push(self.capture_bus_pattern_snapshot());
        }
    }

    pub fn switch_bus_pattern(&mut self, new_idx: usize) {
        self.save_current_bus_pattern();
        self.ensure_bus_pattern_bank_len(new_idx + 1);
        let snapshot = self.bus_pattern_bank[new_idx].clone();
        self.restore_bus_pattern_snapshot(&snapshot);
    }

    pub fn clone_bus_pattern_from_to(&mut self, source_idx: usize, new_idx: usize) {
        self.save_current_bus_pattern();
        let source = self
            .bus_pattern_bank
            .get(source_idx)
            .cloned()
            .unwrap_or_else(|| self.capture_bus_pattern_snapshot());
        self.ensure_bus_pattern_bank_len(new_idx + 1);
        self.bus_pattern_bank[new_idx] = source.clone();
        self.restore_bus_pattern_snapshot(&source);
    }

    pub fn delete_bus_pattern_at(&mut self, deleted_idx: usize, new_idx: usize) {
        if self.bus_pattern_bank.len() > 1 && deleted_idx < self.bus_pattern_bank.len() {
            self.bus_pattern_bank.remove(deleted_idx);
        }
        self.ensure_bus_pattern_bank_len(new_idx + 1);
        let snapshot = self.bus_pattern_bank[new_idx].clone();
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
                Some(BusGateRuntimeState {
                    id: bus.id,
                    gate_id: nodes.gate_id,
                    sequence: bus.gate_sequence.clone(),
                    effect_slots: bus.effect_slots.clone(),
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
            tracks: Vec::new(),
            buses: BusChannelState::default_buses(),
            bus_pattern_bank: Vec::new(),
            sampler_paths: Vec::new(),
            sample_path_registry: HashMap::new(),
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
                layout: LayoutRects::default(),
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
                lisp_libs: Vec::new(),
                instrument_libs: Vec::new(),
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
                provider_state,
                transcript: Vec::new(),
                conversation: Vec::new(),
                input_buffer: String::new(),
                input_cursor: 0,
                scroll_offset: 0,
                auto_retry_budget: 0,
                model_dropdown_open: false,
                model_dropdown_cursor: 0,
                pending_request: None,
                load_error,
            },
            agent_store: ConversationStore::new(sample_rate),
            master_recorder,
            sample_analysis: AnalysisService::new(),
            pending_recording_take: None,
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
                track_engine_ids: Vec::new(),
                track_synth_node_ids: Vec::new(),
                track_gatepitch_node_ids: Vec::new(),
                engine_node_ids: Vec::new(),
                effect_descriptors: Vec::new(),
                instrument_descriptors: Vec::new(),
                record_armed: Vec::new(),
                keyboard_tx,
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
        });
        self.agent_panel.input_buffer.clear();
        self.agent_panel.input_cursor = 0;
        self.agent_panel.auto_retry_budget = 1;

        self.start_agent_request()
    }

    fn current_agent_system_prompt(&self) -> String {
        "You help design DGenLisp instruments and effects for this sequencer. Prefer using tools instead of pasting full code into chat. Use create_instrument_track to make new synth/instrument tracks. Use read_current_instrument_source before iterating on an existing custom synth when you need the current code. Use update_current_instrument to replace the current custom instrument track in place. When the user asks for presets, inspect_current_instrument_preset_schema before creating them, then use create_current_instrument_presets to save one or more named presets for the current custom instrument. For effects, use apply_effect_to_current_track only when the user wants a brand new effect added to the chain. Use read_current_effect_source and update_current_effect when the user is asking to tweak, refine, or iterate on the currently selected custom effect. If no current track exists for an effect, tell the user to create a track first. Be concise and action-oriented.\n\nDGenLisp instrument rules:\n- Instrument definitions must use the actual local DGenLisp instrument syntax used by examples in this repo.\n- Instrument params must follow the sequencer's modulation metadata rules.\n- Any instrument param that can be modulation-targeted must declare a valid @mod-mode.\n- If the patch declares modulatable params, it must also declare at least one input marked with @modulator, following the style used by local examples.\n- If the instrument does not need modulation inputs, do not mark params as modulation-targetable.\n- When adding or changing params, preserve the modulation annotation style used by existing local instruments.\n- If you are unsure about instrument param/modulation structure or instrument declaration syntax, inspect local examples or the current instrument source first.\n- For preset generation, use exact runtime parameter names from inspect_current_instrument_preset_schema and stay within the declared ranges.\n\nDGenLisp effect rules:\n- Effects do not use synth-style modulators.\n- Do not declare @modulator inputs or synth-style modulation metadata in effects.\n- The only modulation-like routing allowed in effects is sidechaining, following the local effect examples and manifest conventions.\n- If the user asks to change an existing effect, prefer replacing the selected custom effect instead of adding a second effect.\n- If generated code fails to compile or reload, revise the code to satisfy the instrument/effect rules instead of asking the user to fix it manually.".to_string()
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
            .and_then(|name| crate::lisp_effect::load_instrument_source(name).ok());
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
            .and_then(|name| crate::lisp_effect::load_effect_source(name).ok());
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
        let conversation = self.agent_panel.conversation.clone();
        let session_context = self.current_agent_session_context();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = match crate::agent::network::AgentNetworkClient::load_default() {
                Ok(client) => client.execute_turn(
                    provider,
                    &model,
                    &system_prompt,
                    &conversation,
                    session_context,
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
                let action_results = result
                    .pending_actions
                    .into_iter()
                    .map(|action| self.apply_agent_action(action))
                    .collect::<Vec<_>>();
                let action_errors = action_results
                    .iter()
                    .filter_map(|result| result.as_ref().err().cloned())
                    .collect::<Vec<_>>();
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
                    });
                    self.agent_panel.transcript.push(AgentTranscriptEntry {
                        role: "system".to_string(),
                        text: repair_message,
                    });
                    if let Err(error) = self.start_agent_request() {
                        self.editor.status_message =
                            Some((format!("Agent retry failed: {error}"), Instant::now()));
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
        self.agent_panel.model_dropdown_open = false;
        self.editor.status_message = Some(("Cleared agent session".to_string(), Instant::now()));
    }

    fn apply_agent_action(&mut self, action: AgentAppAction) -> Result<String, String> {
        match action {
            AgentAppAction::CreateInstrumentTrack { name, source } => {
                let previous_source = crate::lisp_effect::load_instrument_source(&name).ok();
                crate::lisp_effect::save_instrument(&name, &source)
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
                let previous_source = crate::lisp_effect::load_effect_source(&name).ok();
                crate::lisp_effect::save_effect(&name, &source)
                    .map_err(|error| format!("Failed to save effect '{}': {error}", name))?;
                if let Err(error) = self.load_saved_effect_to_slot_sync(track, slot_idx, &name) {
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
                let previous_source = crate::lisp_effect::load_effect_source(&name).ok();
                if let Err(error) = self.replace_current_effect_sync(&name, &source) {
                    self.restore_effect_source(&name, previous_source.as_deref())?;
                    return Err(error);
                }
                Ok(format!("Updated current effect to '{}'.", name))
            }
            AgentAppAction::UpdateCurrentInstrument { name, source } => {
                let previous_source = crate::lisp_effect::load_instrument_source(&name).ok();
                crate::lisp_effect::save_instrument(&name, &source)
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
        let existing_presets = crate::lisp_effect::load_instrument_presets(instrument_name)
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
            crate::lisp_effect::load_instrument_presets(instrument_name).map_err(|error| {
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
            for (idx, param) in desc.params.iter().enumerate() {
                params.insert(
                    param.name.clone(),
                    self.state.pattern.instrument_slots[track].defaults.get(idx),
                );
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
            let preset = crate::lisp_effect::InstrumentPreset {
                id: draft.name.clone(),
                name: draft.name.clone(),
                base_note_offset: draft
                    .base_note_offset
                    .unwrap_or_else(|| self.instrument_base_note_offset(track)),
                params,
            };
            presets_by_name.insert(draft.name.clone(), preset);
        }

        let mut presets = presets_by_name.into_values().collect::<Vec<_>>();
        presets.sort_by(|a, b| a.name.cmp(&b.name));
        crate::lisp_effect::save_instrument_presets(instrument_name, &presets).map_err(
            |error| {
                format!(
                    "Failed to save preset bank for '{}': {error}",
                    instrument_name
                )
            },
        )?;
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
            Some(source) => crate::lisp_effect::save_instrument(name, source)
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
            Some(source) => crate::lisp_effect::save_effect(name, source)
                .map_err(|error| format!("Failed to restore effect '{}': {error}", name)),
            None => std::fs::remove_file(format!("effects/{name}.lisp"))
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

    pub(super) fn sync_sampler_path_from_name(&mut self, track: usize, sample_name: &str) {
        if track >= self.sampler_paths.len() {
            return;
        }
        self.sampler_paths[track] = self.sample_path_registry.get(sample_name).cloned();
    }
}
