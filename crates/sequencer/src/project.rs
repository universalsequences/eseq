use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::effects::{EffectDescriptor, EffectSlotSnapshot, TensorParamSnapshot};
use crate::graph::ProjectGraphOverrides;
use crate::macro_engine::{
    normalize_macro_key, Macro, MacroCurve, MacroEngineError, MacroKind, MacroMapping,
    SceneMacroConfig, StealQuantize,
};
use crate::neural::{ParamNodeId, ProjectNeuralNetwork};
use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    BusId, ChordSnapshot, CustomInstrumentRunMode, InstrumentType, MidiFxPosition, ModConnection,
    ModDestination, PatternSnapshot, RackRouting, RackSlotParamPlocks, RackSlotSnapshot,
    RackTrackSnapshot, SwingResolution, Timebase, TrackId, TrackOutput, TrackParamsSnapshot,
    TrackRegistry, TrackSendSnapshot, TrackSoundState, MAX_STEPS, NUM_PARAMS, TRACK_PATTERN_WORDS,
};
use crate::track_color::TrackColor;

const PROJECTS_DIR: &str = "projects";
const SOUNDS_DIR: &str = "sounds";
const RACK_PRESETS_DIR: &str = "presets/racks";
// Version history:
//   1 — original format; dgenlisp node-state header was 6 slots, so saved
//       `param_node_indices` for dgen slots are `6 + cell_id`.
//   2 — dgenlisp header grew to 10 slots (node-owned process identity);
//       dgen `param_node_indices` are `10 + cell_id`. Version-1 files are
//       migrated on load (see `migrate_legacy_dgen_param_node_indices`).
//   3 — tracks gained stable ids and shared metadata around a kind enum.
const PROJECT_FILE_VERSION: u32 = 3;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSoundPreset {
    pub version: u32,
    pub metadata: ProjectSoundMetadata,
    pub track: ProjectTrack,
    pub rack: ProjectRackTrackPattern,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProjectSoundMetadata {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub author: String,
}

#[derive(Clone, Serialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub bpm: u32,
    #[serde(default = "default_master_volume")]
    pub master_volume: f32,
    pub current_pattern: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_track: Option<usize>,
    pub reverb: ProjectReverbState,
    #[serde(default = "default_project_buses")]
    pub buses: Vec<ProjectBusChannel>,
    pub tracks: Vec<ProjectTrack>,
    pub custom_effects: Vec<Vec<Option<String>>>,
    #[serde(default)]
    pub scratch: ProjectScratchState,
    pub patterns: Vec<ProjectPattern>,
    #[serde(default)]
    pub groups: Vec<ProjectTrackGroup>,
    #[serde(default)]
    pub macros: Vec<ProjectMacro>,
    #[serde(default = "default_next_macro_id")]
    pub next_macro_id: u32,
}

#[derive(Deserialize)]
struct ProjectFileWire {
    version: u32,
    name: String,
    bpm: u32,
    #[serde(default = "default_master_volume")]
    master_volume: f32,
    current_pattern: usize,
    #[serde(default)]
    current_track: Option<usize>,
    reverb: ProjectReverbState,
    #[serde(default = "default_project_buses")]
    buses: Vec<ProjectBusChannel>,
    tracks: Vec<ProjectTrackWire>,
    custom_effects: Vec<Vec<Option<String>>>,
    #[serde(default)]
    scratch: ProjectScratchState,
    patterns: Vec<ProjectPattern>,
    #[serde(default)]
    groups: Vec<ProjectTrackGroup>,
    #[serde(default)]
    macros: Vec<ProjectMacro>,
    #[serde(default = "default_next_macro_id")]
    next_macro_id: u32,
}

impl<'de> Deserialize<'de> for ProjectFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ProjectFileWire::deserialize(deserializer)?;
        let tracks = migrate_project_tracks(wire.version, wire.tracks).map_err(D::Error::custom)?;
        Ok(Self {
            version: wire.version,
            name: wire.name,
            bpm: wire.bpm,
            master_volume: wire.master_volume,
            current_pattern: wire.current_pattern,
            current_track: wire.current_track,
            reverb: wire.reverb,
            buses: wire.buses,
            tracks,
            custom_effects: wire.custom_effects,
            scratch: wire.scratch,
            patterns: wire.patterns,
            groups: wire.groups,
            macros: wire.macros,
            next_macro_id: wire.next_macro_id,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectMacro {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub name: String,
    pub value: f32,
    #[serde(default)]
    pub kind: ProjectMacroKind,
    #[serde(default)]
    pub mappings: Vec<ProjectMacroMapping>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum ProjectMacroKind {
    #[default]
    Mapped,
    Scene(ProjectSceneMacroConfig),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSceneMacroConfig {
    pub target_scene: usize,
    pub morph_params: bool,
    pub steal_patterns: bool,
    #[serde(default)]
    pub quantize: ProjectStealQuantize,
    #[serde(default)]
    pub track_mask: Option<Vec<bool>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectStealQuantize {
    Off,
    Sixteenth,
    #[default]
    Bar,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectMacroMapping {
    pub scope: ProjectParamScope,
    pub target: crate::process::ParamTarget,
    pub range_min: f32,
    pub range_max: f32,
    #[serde(default)]
    pub curve: ProjectMacroCurve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ProjectParamScope {
    Track(usize),
    Bus(u64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectMacroCurve {
    #[default]
    Linear,
    Exp,
    Log,
    LogDomain,
}

fn default_next_macro_id() -> u32 {
    1
}

impl From<&Macro> for ProjectMacro {
    fn from(value: &Macro) -> Self {
        Self {
            id: value.id,
            key: value.key.clone(),
            name: value.name.clone(),
            value: value.value,
            kind: match &value.kind {
                MacroKind::Mapped => ProjectMacroKind::Mapped,
                MacroKind::Scene(config) => {
                    ProjectMacroKind::Scene(ProjectSceneMacroConfig::from(config))
                }
            },
            mappings: value
                .mappings
                .iter()
                .filter(|_| matches!(value.kind, MacroKind::Mapped))
                .map(ProjectMacroMapping::from)
                .collect(),
        }
    }
}

impl TryFrom<ProjectMacro> for Macro {
    type Error = MacroEngineError;

    fn try_from(value: ProjectMacro) -> Result<Self, Self::Error> {
        if !value.value.is_finite() {
            return Err(MacroEngineError::NonFiniteValue);
        }
        let key = value.key.as_deref().map(normalize_macro_key).transpose()?;
        let kind = match value.kind {
            ProjectMacroKind::Mapped => MacroKind::Mapped,
            ProjectMacroKind::Scene(config) => MacroKind::Scene(config.into()),
        };
        let mut macro_definition = Macro::new(value.id, value.name, kind);
        macro_definition.key = key;
        macro_definition.value = value.value.clamp(0.0, 1.0);
        macro_definition.mappings = value
            .mappings
            .into_iter()
            .map(MacroMapping::try_from)
            .collect::<Result<_, _>>()?;
        Ok(macro_definition)
    }
}

impl From<&MacroMapping> for ProjectMacroMapping {
    fn from(value: &MacroMapping) -> Self {
        Self {
            scope: match value.scope {
                crate::macro_engine::ParamScope::Track(track) => ProjectParamScope::Track(track),
                crate::macro_engine::ParamScope::Bus(bus) => ProjectParamScope::Bus(bus.0),
            },
            target: value.target.clone(),
            range_min: value.range_min,
            range_max: value.range_max,
            curve: value.curve.into(),
        }
    }
}

impl TryFrom<ProjectMacroMapping> for MacroMapping {
    type Error = MacroEngineError;

    fn try_from(value: ProjectMacroMapping) -> Result<Self, Self::Error> {
        MacroMapping::new(
            match value.scope {
                ProjectParamScope::Track(track) => crate::macro_engine::ParamScope::Track(track),
                ProjectParamScope::Bus(bus) => {
                    crate::macro_engine::ParamScope::Bus(crate::sequencer::BusId(bus))
                }
            },
            value.target,
            value.range_min,
            value.range_max,
            value.curve.into(),
        )
    }
}

impl From<MacroCurve> for ProjectMacroCurve {
    fn from(value: MacroCurve) -> Self {
        match value {
            MacroCurve::Linear => Self::Linear,
            MacroCurve::Exp => Self::Exp,
            MacroCurve::Log => Self::Log,
            MacroCurve::LogDomain => Self::LogDomain,
        }
    }
}

impl From<ProjectMacroCurve> for MacroCurve {
    fn from(value: ProjectMacroCurve) -> Self {
        match value {
            ProjectMacroCurve::Linear => Self::Linear,
            ProjectMacroCurve::Exp => Self::Exp,
            ProjectMacroCurve::Log => Self::Log,
            ProjectMacroCurve::LogDomain => Self::LogDomain,
        }
    }
}

impl From<&SceneMacroConfig> for ProjectSceneMacroConfig {
    fn from(value: &SceneMacroConfig) -> Self {
        Self {
            target_scene: value.target_scene,
            morph_params: value.morph_params,
            steal_patterns: value.steal_patterns,
            quantize: value.quantize.into(),
            track_mask: value.track_mask.clone(),
        }
    }
}

impl From<ProjectSceneMacroConfig> for SceneMacroConfig {
    fn from(value: ProjectSceneMacroConfig) -> Self {
        Self {
            target_scene: value.target_scene,
            morph_params: value.morph_params,
            steal_patterns: value.steal_patterns,
            quantize: value.quantize.into(),
            track_mask: value.track_mask,
        }
    }
}

impl From<StealQuantize> for ProjectStealQuantize {
    fn from(value: StealQuantize) -> Self {
        match value {
            StealQuantize::Off => Self::Off,
            StealQuantize::Sixteenth => Self::Sixteenth,
            StealQuantize::Bar => Self::Bar,
        }
    }
}

impl From<ProjectStealQuantize> for StealQuantize {
    fn from(value: ProjectStealQuantize) -> Self {
        match value {
            ProjectStealQuantize::Off => Self::Off,
            ProjectStealQuantize::Sixteenth => Self::Sixteenth,
            ProjectStealQuantize::Bar => Self::Bar,
        }
    }
}

/// A track group: lightweight metadata folding a set of tracks into one mixer
/// unit backed by an auto-created bus. It references tracks by index and owns a
/// backing bus by id; it does not own track data. See
/// `docs/track-groups-spec.md`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectTrackGroup {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub color: [f32; 3],
    #[serde(default)]
    pub collapsed: bool,
    /// Ordered member track indices.
    pub members: Vec<usize>,
    /// Backing `ProjectBusChannel` this group routes to.
    pub bus_id: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProjectScratchState {
    #[serde(default)]
    pub buffer: String,
    #[serde(default)]
    pub cursor_row: usize,
    #[serde(default)]
    pub cursor_col: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectReverbState {
    pub size: f32,
    pub brightness: f32,
    pub replace: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectBusChannel {
    pub id: u64,
    pub name: String,
    #[serde(default = "default_track_volume")]
    pub volume: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub gate_sequence: ProjectBusGateSequence,
    #[serde(default)]
    pub custom_effects: Vec<Option<String>>,
    #[serde(default)]
    pub effect_slots: Vec<ProjectEffectSlot>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectBusGateSequence {
    #[serde(default = "default_bus_gate_steps")]
    pub steps: Vec<bool>,
    #[serde(default = "default_bus_gate_values")]
    pub velocities: Vec<f32>,
    #[serde(default = "default_bus_gate_values")]
    pub durations: Vec<f32>,
    #[serde(default = "default_bus_gate_syncs")]
    pub syncs: Vec<f32>,
    #[serde(default = "default_num_steps")]
    pub num_steps: usize,
    #[serde(default = "default_timebase")]
    pub timebase: u8,
    #[serde(default = "default_swing")]
    pub swing: f32,
    #[serde(default = "default_swing_resolution")]
    pub swing_resolution: u8,
    #[serde(default)]
    pub timebase_plocks: Vec<Option<u32>>,
    #[serde(default)]
    pub swing_plocks: Vec<Option<f32>>,
    #[serde(default)]
    pub swing_resolution_plocks: Vec<Option<u32>>,
}

impl Default for ProjectBusGateSequence {
    fn default() -> Self {
        Self {
            steps: default_bus_gate_steps(),
            velocities: default_bus_gate_values(),
            durations: default_bus_gate_values(),
            syncs: default_bus_gate_syncs(),
            num_steps: default_num_steps(),
            timebase: default_timebase(),
            swing: default_swing(),
            swing_resolution: default_swing_resolution(),
            timebase_plocks: vec![None; MAX_STEPS],
            swing_plocks: vec![None; MAX_STEPS],
            swing_resolution_plocks: vec![None; MAX_STEPS],
        }
    }
}

fn default_bus_gate_steps() -> Vec<bool> {
    vec![true; MAX_STEPS]
}

fn default_bus_gate_values() -> Vec<f32> {
    vec![1.0; MAX_STEPS]
}

fn default_bus_gate_syncs() -> Vec<f32> {
    vec![0.0; MAX_STEPS]
}

pub fn default_project_buses() -> Vec<ProjectBusChannel> {
    vec![
        ProjectBusChannel {
            id: crate::sequencer::MIX_BUS_ID,
            name: "Mix".to_string(),
            volume: crate::mixer_volume::default_fader(),
            mute: false,
            solo: false,
            gate_sequence: ProjectBusGateSequence::default(),
            custom_effects: Vec::new(),
            effect_slots: Vec::new(),
        },
        ProjectBusChannel {
            id: crate::sequencer::DEFAULT_BUS_A_ID,
            name: "Bus A".to_string(),
            volume: crate::mixer_volume::default_fader(),
            mute: false,
            solo: false,
            gate_sequence: ProjectBusGateSequence::default(),
            custom_effects: Vec::new(),
            effect_slots: Vec::new(),
        },
        ProjectBusChannel {
            id: crate::sequencer::DEFAULT_BUS_B_ID,
            name: "Bus B".to_string(),
            volume: crate::mixer_volume::default_fader(),
            mute: false,
            solo: false,
            gate_sequence: ProjectBusGateSequence::default(),
            custom_effects: Vec::new(),
            effect_slots: Vec::new(),
        },
    ]
}

#[derive(Clone, Serialize)]
pub struct ProjectTrack {
    pub id: TrackId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<TrackColor>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    #[serde(flatten)]
    pub kind: ProjectTrackKind,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectTrackKind {
    Sampler {
        sample_path: String,
    },
    Custom {
        instrument_name: String,
    },
    Modulator,
    Rack {
        #[serde(default)]
        routing: ProjectRackRouting,
        #[serde(default)]
        slots: Vec<ProjectRackTrackSlot>,
    },
}

#[derive(Deserialize)]
struct ProjectTrackWire {
    #[serde(default)]
    id: Option<TrackId>,
    #[serde(default)]
    color: Option<TrackColor>,
    #[serde(default)]
    collapsed: bool,
    #[serde(flatten)]
    kind: ProjectTrackKind,
}

impl<'de> Deserialize<'de> for ProjectTrack {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ProjectTrackWire::deserialize(deserializer)?;
        let id = wire.id.unwrap_or(TrackId::MIN);
        if id.0 == 0 {
            return Err(D::Error::custom("stable track id 0 is reserved"));
        }
        Ok(Self {
            id,
            color: wire.color,
            collapsed: wire.collapsed,
            kind: wire.kind,
        })
    }
}

fn migrate_project_tracks(
    version: u32,
    tracks: Vec<ProjectTrackWire>,
) -> Result<Vec<ProjectTrack>, String> {
    if version < 3 {
        let registry = TrackRegistry::for_legacy_track_count(tracks.len())
            .map_err(|error| format!("could not assign legacy track ids: {error:?}"))?;
        return Ok(tracks
            .into_iter()
            .zip(registry.ids().iter().copied())
            .map(|(track, id)| ProjectTrack {
                id,
                color: track.color,
                collapsed: track.collapsed,
                kind: track.kind,
            })
            .collect());
    }

    let mut used = HashSet::with_capacity(tracks.len());
    for id in tracks.iter().filter_map(|track| track.id) {
        if id.0 == 0 {
            return Err("stable track id 0 is reserved".to_string());
        }
        if !used.insert(id) {
            return Err(format!("duplicate stable track id {}", id.0));
        }
    }
    let mut next_missing = 1u64;
    let mut migrated = Vec::with_capacity(tracks.len());
    for track in tracks {
        let id = match track.id {
            Some(id) => id,
            None => {
                if next_missing == 0 {
                    return Err("stable track id space is exhausted".to_string());
                }
                while used.contains(&TrackId(next_missing)) {
                    next_missing = next_missing
                        .checked_add(1)
                        .ok_or_else(|| "stable track id space is exhausted".to_string())?;
                }
                let id = TrackId(next_missing);
                used.insert(id);
                next_missing = next_missing.checked_add(1).unwrap_or(0);
                id
            }
        };
        migrated.push(ProjectTrack {
            id,
            color: track.color,
            collapsed: track.collapsed,
            kind: track.kind,
        });
    }
    TrackRegistry::from_ids(migrated.iter().map(|track| track.id))
        .map_err(|error| format!("invalid stable track ids: {error:?}"))?;
    Ok(migrated)
}

impl ProjectTrack {
    pub fn color(&self) -> Option<TrackColor> {
        self.color.map(TrackColor::clamped)
    }

    pub fn collapsed(&self) -> bool {
        self.collapsed
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectRackTrackSlot {
    pub instrument_type: ProjectInstrumentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument_name: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectPattern {
    pub track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    #[serde(default)]
    pub neural_reset_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    #[serde(
        serialize_with = "serialize_step_data",
        deserialize_with = "deserialize_step_data"
    )]
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<ProjectTrackParams>,
    pub effect_slots: Vec<Vec<ProjectEffectSlot>>,
    #[serde(default)]
    pub midi_fx_slots: Vec<Vec<ProjectEffectSlot>>,
    pub instrument_slots: Vec<ProjectEffectSlot>,
    pub instrument_base_note_offsets: Vec<f32>,
    pub track_sound_states: Vec<ProjectTrackSoundState>,
    #[serde(
        serialize_with = "serialize_chord_snapshots",
        deserialize_with = "deserialize_chord_snapshots"
    )]
    pub chord_snapshots: Vec<Vec<Vec<f32>>>,
    #[serde(
        default,
        serialize_with = "serialize_chord_snapshots",
        deserialize_with = "deserialize_chord_snapshots"
    )]
    pub chord_duration_snapshots: Vec<Vec<Vec<f32>>>,
    #[serde(
        default,
        serialize_with = "serialize_chord_snapshots",
        deserialize_with = "deserialize_chord_snapshots"
    )]
    pub chord_delay_snapshots: Vec<Vec<Vec<f32>>>,
    #[serde(
        serialize_with = "serialize_timebase_plock_snapshots",
        deserialize_with = "deserialize_timebase_plock_snapshots"
    )]
    pub timebase_plock_snapshots: Vec<Vec<Option<u32>>>,
    #[serde(
        default,
        serialize_with = "serialize_timebase_plock_snapshots",
        deserialize_with = "deserialize_timebase_plock_snapshots"
    )]
    pub swing_plock_snapshots: Vec<Vec<Option<u32>>>,
    #[serde(
        default,
        serialize_with = "serialize_timebase_plock_snapshots",
        deserialize_with = "deserialize_timebase_plock_snapshots"
    )]
    pub swing_resolution_plock_snapshots: Vec<Vec<Option<u32>>>,
    #[serde(default)]
    pub bus_patterns: Vec<ProjectBusPatternSnapshot>,
    #[serde(default)]
    pub mod_connections: Vec<ProjectModConnection>,
    #[serde(default)]
    pub neural_networks: Vec<ProjectNeuralNetwork>,
    #[serde(default)]
    pub graph_overrides: Vec<ProjectGraphOverrides>,
    pub instrument_types: Vec<ProjectInstrumentType>,
    #[serde(default)]
    pub instrument_run_modes: Vec<ProjectCustomInstrumentRunMode>,
    pub sample_paths: Vec<Option<String>>,
    pub sample_names: Vec<String>,
    #[serde(default)]
    pub rack_tracks: Vec<Option<ProjectRackTrackPattern>>,
    #[serde(default)]
    pub process_chains: Vec<crate::process::TrackProcessChain>,
    #[serde(default)]
    pub project_process_lane_overrides: Vec<crate::process::ProjectLaneOverrides>,
    #[serde(default)]
    pub project_process_chain: crate::process::TrackProcessChain,
    #[serde(default)]
    pub plock_variant_registries: Vec<PlockVariantRegistry>,
    #[serde(default)]
    pub key_lock_variant_registries: Vec<PlockVariantRegistry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectModConnection {
    pub source_track: usize,
    #[serde(default)]
    pub destination: Option<ProjectModDestination>,
    #[serde(default)]
    pub dest_track: Option<usize>,
    #[serde(default)]
    pub dest_input: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum ProjectModDestination {
    Track(usize),
    Bus(u64),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectTrackParams {
    pub gate: bool,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub swing: f32,
    #[serde(default = "default_swing_resolution")]
    pub swing_resolution: u8,
    pub num_steps: usize,
    #[serde(default = "default_track_volume")]
    pub volume: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    pub send: f32,
    #[serde(default)]
    pub output: ProjectTrackOutput,
    #[serde(default)]
    pub sends: Vec<ProjectTrackSend>,
    pub polyphonic: bool,
    #[serde(default = "default_max_polyphony")]
    pub max_polyphony: usize,
    pub timebase: u8,
    #[serde(default)]
    pub accumulator_idx: usize,
    #[serde(default)]
    pub script_accumulator_name: Option<String>,
    #[serde(default)]
    pub midi_fx_chain: Vec<String>,
    #[serde(default = "default_midi_fx_position")]
    pub midi_fx_position: ProjectMidiFxPosition,
    #[serde(default = "default_accum_limit")]
    pub accum_limit: f32,
    #[serde(default)]
    pub accum_mode: u32,
    #[serde(default)]
    pub fts_scale: usize,
    #[serde(default)]
    pub mute_group: u8,
    #[serde(default = "default_true")]
    pub global_transpose: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectTrackOutput {
    Mix,
    Bus { id: u64 },
    None,
}

impl Default for ProjectTrackOutput {
    fn default() -> Self {
        Self::Mix
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectTrackSend {
    pub destination: u64,
    pub amount: f32,
}

#[derive(Clone, Default)]
pub struct ProjectEffectSlot {
    pub num_params: u32,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub plock_param_ids: Vec<Vec<Option<ParamNodeId>>>,
    pub key_locks: std::collections::BTreeMap<u8, Vec<Option<f32>>>,
    pub key_lock_param_ids: std::collections::BTreeMap<u8, Vec<Option<ParamNodeId>>>,
    pub tensor_params: Vec<TensorParamSnapshot>,
    pub param_node_indices: Vec<u32>,
    pub param_node_spans: Vec<u32>,
    /// Effect-specific instance data that isn't a numeric param. Currently just
    /// the Convolution Reverb's impulse-response reference (sample hash/stem).
    pub ir: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProjectTrackSoundState {
    pub loaded_preset: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectInstrumentType {
    Sampler,
    Custom,
    Modulator,
    Rack,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRackRouting {
    #[default]
    Broadcast,
    ByPitch,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectRackTrackPattern {
    #[serde(default)]
    pub routing: ProjectRackRouting,
    #[serde(default)]
    pub slots: Vec<ProjectRackSlotPattern>,
    #[serde(default = "default_project_rack_macros")]
    pub macros: Vec<ProjectRackMacro>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectRackMacro {
    pub id: u8,
    pub name: String,
    #[serde(default)]
    pub value: f32,
    #[serde(default)]
    pub mappings: Vec<ProjectRackMacroMapping>,
    #[serde(default)]
    pub plocks: Vec<Option<f32>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectRackMacroMapping {
    pub target: ProjectRackMacroTarget,
    pub range_min: f32,
    pub range_max: f32,
    #[serde(default)]
    pub curve: ProjectRackMacroCurve,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectRackMacroTarget {
    SlotParam {
        slot: usize,
        param: String,
    },
    SlotInstrumentParam {
        slot: usize,
        param: String,
        param_index: usize,
    },
    SlotEffectParam {
        slot: usize,
        effect_slot: usize,
        param: String,
        param_index: usize,
    },
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRackMacroCurve {
    #[default]
    Linear,
    Exp,
    Log,
}

pub(crate) fn default_project_rack_macros() -> Vec<ProjectRackMacro> {
    (0..crate::sequencer::RACK_MACRO_COUNT)
        .map(|id| ProjectRackMacro {
            id: id as u8,
            name: format!("Macro {}", id + 1),
            value: 0.0,
            mappings: Vec::new(),
            plocks: vec![None; crate::sequencer::MAX_STEPS],
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectRackSlotPattern {
    pub instrument_type: ProjectInstrumentType,
    #[serde(default)]
    pub instrument_run_mode: ProjectCustomInstrumentRunMode,
    #[serde(default)]
    pub instrument_base_note_offset: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_note: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choke_group: Option<u8>,
    #[serde(default = "default_rack_slot_gain")]
    pub gain: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default = "default_max_polyphony")]
    pub max_polyphony: usize,
    #[serde(default)]
    pub param_plocks: Vec<Vec<Option<f32>>>,
    #[serde(default)]
    pub instrument_slot: ProjectEffectSlot,
    #[serde(default)]
    pub effect_slots: Vec<ProjectEffectSlot>,
    #[serde(default)]
    pub custom_effects: Vec<Option<String>>,
    #[serde(default)]
    pub track_sound_state: ProjectTrackSoundState,
    #[serde(default)]
    pub sample_path: Option<String>,
    #[serde(default)]
    pub sample_name: Option<String>,
}

fn default_rack_slot_gain() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectCustomInstrumentRunMode {
    #[default]
    Instrument,
    FreePatch,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectMidiFxPosition {
    PreAccumulator,
    PostAccumulator,
}

fn default_midi_fx_position() -> ProjectMidiFxPosition {
    ProjectMidiFxPosition::PostAccumulator
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProjectBusPatternSnapshot {
    pub id: u64,
    #[serde(default)]
    pub gate_sequence: ProjectBusGateSequence,
    #[serde(default)]
    pub effect_slots: Vec<ProjectEffectSlot>,
}

impl ProjectPattern {
    pub fn from_snapshot(
        snapshot: &PatternSnapshot,
        sample_paths: Vec<Option<String>>,
        sample_names: Vec<String>,
        bus_patterns: Vec<ProjectBusPatternSnapshot>,
    ) -> Self {
        Self {
            track_bits: snapshot.track_bits.clone(),
            neural_reset_bits: snapshot.neural_reset_bits.clone(),
            step_data: snapshot.step_data.clone(),
            track_params: snapshot
                .track_params
                .iter()
                .cloned()
                .map(ProjectTrackParams::from)
                .collect(),
            effect_slots: snapshot
                .effect_slots
                .iter()
                .map(|slots| slots.iter().map(ProjectEffectSlot::from).collect())
                .collect(),
            midi_fx_slots: snapshot
                .midi_fx_slots
                .iter()
                .map(|slots| slots.iter().map(ProjectEffectSlot::from).collect())
                .collect(),
            instrument_slots: snapshot
                .instrument_slots
                .iter()
                .map(ProjectEffectSlot::from)
                .collect(),
            instrument_base_note_offsets: snapshot.instrument_base_note_offsets.clone(),
            track_sound_states: snapshot
                .track_sound_states
                .iter()
                .cloned()
                .map(ProjectTrackSoundState::from)
                .collect(),
            chord_snapshots: snapshot
                .chord_snapshots
                .iter()
                .map(|snap| snap.steps.clone())
                .collect(),
            chord_duration_snapshots: snapshot
                .chord_snapshots
                .iter()
                .map(|snap| snap.durations.clone())
                .collect(),
            chord_delay_snapshots: snapshot
                .chord_snapshots
                .iter()
                .map(|snap| snap.delays.clone())
                .collect(),
            timebase_plock_snapshots: snapshot
                .timebase_plock_snapshots
                .iter()
                .map(|steps| steps.to_vec())
                .collect(),
            swing_plock_snapshots: snapshot
                .swing_plock_snapshots
                .iter()
                .map(|steps| steps.to_vec())
                .collect(),
            swing_resolution_plock_snapshots: snapshot
                .swing_resolution_plock_snapshots
                .iter()
                .map(|steps| steps.to_vec())
                .collect(),
            bus_patterns,
            mod_connections: snapshot
                .mod_connections
                .iter()
                .copied()
                .map(ProjectModConnection::from)
                .collect(),
            neural_networks: snapshot.neural_networks.clone(),
            graph_overrides: snapshot.graph_overrides.clone(),
            instrument_types: snapshot
                .instrument_types
                .iter()
                .copied()
                .map(ProjectInstrumentType::from)
                .collect(),
            instrument_run_modes: snapshot
                .instrument_run_modes
                .iter()
                .copied()
                .map(ProjectCustomInstrumentRunMode::from)
                .collect(),
            sample_paths,
            sample_names,
            rack_tracks: snapshot
                .rack_tracks
                .iter()
                .cloned()
                .map(|rack| rack.map(ProjectRackTrackPattern::from))
                .collect(),
            process_chains: snapshot.process_chains.clone(),
            project_process_lane_overrides: snapshot.project_process_lane_overrides.clone(),
            project_process_chain: snapshot.project_process_chain.clone(),
            plock_variant_registries: snapshot.plock_variant_registries.clone(),
            key_lock_variant_registries: snapshot.key_lock_variant_registries.clone(),
        }
    }
}

impl From<TrackParamsSnapshot> for ProjectTrackParams {
    fn from(value: TrackParamsSnapshot) -> Self {
        Self {
            gate: value.gate,
            attack_ms: value.attack_ms,
            release_ms: value.release_ms,
            swing: value.swing,
            swing_resolution: value.swing_resolution as u8,
            num_steps: value.num_steps,
            volume: value.volume,
            pan: value.pan,
            mute: value.mute,
            solo: value.solo,
            send: value.send,
            output: ProjectTrackOutput::from(value.output),
            sends: value
                .sends
                .into_iter()
                .map(ProjectTrackSend::from)
                .collect(),
            polyphonic: value.polyphonic,
            max_polyphony: value.max_polyphony,
            timebase: value.timebase as u8,
            accumulator_idx: value.accumulator_idx,
            script_accumulator_name: value.script_accumulator_name,
            midi_fx_chain: value.midi_fx_chain,
            midi_fx_position: ProjectMidiFxPosition::from(value.midi_fx_position),
            accum_limit: value.accum_limit,
            accum_mode: value.accum_mode,
            fts_scale: value.fts_scale,
            mute_group: value.mute_group.min(8),
            global_transpose: value.global_transpose,
        }
    }
}

impl From<ProjectTrackParams> for TrackParamsSnapshot {
    fn from(value: ProjectTrackParams) -> Self {
        Self {
            gate: value.gate,
            attack_ms: value.attack_ms,
            release_ms: value.release_ms,
            swing: value.swing,
            swing_resolution: SwingResolution::from_index(value.swing_resolution as u32),
            num_steps: value.num_steps,
            volume: value.volume,
            pan: value.pan,
            mute: value.mute,
            solo: value.solo,
            send: value.send,
            output: TrackOutput::from(value.output),
            sends: value
                .sends
                .into_iter()
                .map(TrackSendSnapshot::from)
                .collect(),
            polyphonic: value.polyphonic,
            max_polyphony: value.max_polyphony.clamp(1, crate::voice::MAX_VOICES),
            timebase: Timebase::from_index(value.timebase as u32),
            accumulator_idx: value.accumulator_idx,
            script_accumulator_name: value.script_accumulator_name,
            midi_fx_chain: value.midi_fx_chain,
            midi_fx_position: MidiFxPosition::from(value.midi_fx_position),
            accum_limit: value.accum_limit,
            accum_mode: value.accum_mode,
            fts_scale: value.fts_scale,
            mute_group: value.mute_group.min(8),
            global_transpose: value.global_transpose,
        }
    }
}

impl From<TrackOutput> for ProjectTrackOutput {
    fn from(value: TrackOutput) -> Self {
        match value {
            TrackOutput::Mix => Self::Mix,
            TrackOutput::Bus(id) => Self::Bus { id: id.0 },
            TrackOutput::None => Self::None,
        }
    }
}

impl From<ProjectTrackOutput> for TrackOutput {
    fn from(value: ProjectTrackOutput) -> Self {
        match value {
            ProjectTrackOutput::Mix => Self::Mix,
            ProjectTrackOutput::Bus { id } => Self::Bus(BusId(id)),
            ProjectTrackOutput::None => Self::None,
        }
    }
}

impl From<TrackSendSnapshot> for ProjectTrackSend {
    fn from(value: TrackSendSnapshot) -> Self {
        Self {
            destination: value.destination.0,
            amount: value.amount,
        }
    }
}

impl From<ProjectTrackSend> for TrackSendSnapshot {
    fn from(value: ProjectTrackSend) -> Self {
        Self {
            destination: BusId(value.destination),
            amount: value.amount.clamp(0.0, 1.0),
        }
    }
}

impl From<MidiFxPosition> for ProjectMidiFxPosition {
    fn from(value: MidiFxPosition) -> Self {
        match value {
            MidiFxPosition::PreAccumulator => Self::PreAccumulator,
            MidiFxPosition::PostAccumulator => Self::PostAccumulator,
        }
    }
}

impl From<ProjectMidiFxPosition> for MidiFxPosition {
    fn from(value: ProjectMidiFxPosition) -> Self {
        match value {
            ProjectMidiFxPosition::PreAccumulator => Self::PreAccumulator,
            ProjectMidiFxPosition::PostAccumulator => Self::PostAccumulator,
        }
    }
}

impl From<&EffectSlotSnapshot> for ProjectEffectSlot {
    fn from(value: &EffectSlotSnapshot) -> Self {
        Self {
            num_params: value.num_params,
            defaults: value.defaults.clone(),
            plocks: value.plocks.clone(),
            plock_param_ids: value.plock_param_ids.clone(),
            key_locks: value.key_locks.clone(),
            key_lock_param_ids: value.key_lock_param_ids.clone(),
            tensor_params: value.tensor_params.clone(),
            param_node_indices: value.param_node_indices.clone(),
            param_node_spans: value.param_node_spans.clone(),
            ir: value.ir.clone(),
        }
    }
}

impl ProjectEffectSlot {
    pub fn into_snapshot_with_node_id(self, node_id: u32) -> EffectSlotSnapshot {
        self.into_snapshot_with_node_ids(node_id, 0)
    }

    pub fn into_snapshot_with_node_ids(
        self,
        node_id: u32,
        modulator_node_id: u32,
    ) -> EffectSlotSnapshot {
        EffectSlotSnapshot {
            node_id,
            modulator_node_id,
            num_params: self.num_params,
            defaults: self.defaults,
            plocks: self.plocks,
            plock_param_ids: self.plock_param_ids,
            key_locks: self.key_locks,
            key_lock_param_ids: self.key_lock_param_ids,
            tensor_params: self.tensor_params,
            param_node_indices: self.param_node_indices,
            param_node_spans: self.param_node_spans,
            transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
            ir: self.ir,
        }
    }
}

impl From<TrackSoundState> for ProjectTrackSoundState {
    fn from(value: TrackSoundState) -> Self {
        Self {
            loaded_preset: value.loaded_preset,
            dirty: value.dirty,
        }
    }
}

impl ProjectTrackSoundState {
    pub fn into_track_sound_state(self, engine_id: Option<usize>) -> TrackSoundState {
        TrackSoundState {
            engine_id,
            loaded_preset: self.loaded_preset,
            dirty: self.dirty,
        }
    }
}

impl From<InstrumentType> for ProjectInstrumentType {
    fn from(value: InstrumentType) -> Self {
        match value {
            InstrumentType::Sampler => Self::Sampler,
            InstrumentType::Custom => Self::Custom,
            InstrumentType::Modulator => Self::Modulator,
            InstrumentType::Rack => Self::Rack,
        }
    }
}

impl From<ProjectInstrumentType> for InstrumentType {
    fn from(value: ProjectInstrumentType) -> Self {
        match value {
            ProjectInstrumentType::Sampler => InstrumentType::Sampler,
            ProjectInstrumentType::Custom => InstrumentType::Custom,
            ProjectInstrumentType::Modulator => InstrumentType::Modulator,
            ProjectInstrumentType::Rack => InstrumentType::Rack,
        }
    }
}

impl From<CustomInstrumentRunMode> for ProjectCustomInstrumentRunMode {
    fn from(value: CustomInstrumentRunMode) -> Self {
        match value {
            CustomInstrumentRunMode::Instrument => Self::Instrument,
            CustomInstrumentRunMode::FreePatch => Self::FreePatch,
        }
    }
}

impl From<ProjectCustomInstrumentRunMode> for CustomInstrumentRunMode {
    fn from(value: ProjectCustomInstrumentRunMode) -> Self {
        match value {
            ProjectCustomInstrumentRunMode::Instrument => CustomInstrumentRunMode::Instrument,
            ProjectCustomInstrumentRunMode::FreePatch => CustomInstrumentRunMode::FreePatch,
        }
    }
}

impl From<RackRouting> for ProjectRackRouting {
    fn from(value: RackRouting) -> Self {
        match value {
            RackRouting::Broadcast => Self::Broadcast,
            RackRouting::ByPitch => Self::ByPitch,
        }
    }
}

impl From<ProjectRackRouting> for RackRouting {
    fn from(value: ProjectRackRouting) -> Self {
        match value {
            ProjectRackRouting::Broadcast => RackRouting::Broadcast,
            ProjectRackRouting::ByPitch => RackRouting::ByPitch,
        }
    }
}

impl From<RackTrackSnapshot> for ProjectRackTrackPattern {
    fn from(value: RackTrackSnapshot) -> Self {
        Self {
            routing: ProjectRackRouting::from(value.routing),
            slots: value
                .slots
                .into_iter()
                .map(ProjectRackSlotPattern::from)
                .collect(),
            macros: value
                .macros
                .into_iter()
                .map(ProjectRackMacro::from)
                .collect(),
        }
    }
}

impl From<ProjectRackTrackPattern> for RackTrackSnapshot {
    fn from(value: ProjectRackTrackPattern) -> Self {
        let mut rack = Self {
            routing: RackRouting::from(value.routing),
            slots: value
                .slots
                .into_iter()
                .map(RackSlotSnapshot::from)
                .collect(),
            macros: value
                .macros
                .into_iter()
                .filter_map(|value| value.try_into().ok())
                .collect(),
            runtime_macro_values: None,
            runtime_macro_track: 0,
        };
        rack.normalize_macros();
        rack
    }
}

impl From<crate::sequencer::RackMacro> for ProjectRackMacro {
    fn from(value: crate::sequencer::RackMacro) -> Self {
        Self {
            id: value.id.index() as u8,
            name: value.name,
            value: value.value,
            mappings: value
                .mappings
                .into_iter()
                .map(ProjectRackMacroMapping::from)
                .collect(),
            plocks: value.plocks,
        }
    }
}

impl TryFrom<ProjectRackMacro> for crate::sequencer::RackMacro {
    type Error = ();
    fn try_from(value: ProjectRackMacro) -> Result<Self, Self::Error> {
        let id = crate::sequencer::RackMacroId::from_index(value.id as usize).ok_or(())?;
        Ok(Self {
            id,
            name: value.name,
            value: value.value.clamp(0.0, 1.0),
            mappings: value.mappings.into_iter().map(Into::into).collect(),
            plocks: value.plocks,
        })
    }
}

impl From<crate::sequencer::RackMacroMapping> for ProjectRackMacroMapping {
    fn from(value: crate::sequencer::RackMacroMapping) -> Self {
        Self {
            target: value.target.into(),
            range_min: value.range_min,
            range_max: value.range_max,
            curve: match value.curve {
                crate::sequencer::RackMacroCurve::Linear => ProjectRackMacroCurve::Linear,
                crate::sequencer::RackMacroCurve::Exp => ProjectRackMacroCurve::Exp,
                crate::sequencer::RackMacroCurve::Log => ProjectRackMacroCurve::Log,
            },
        }
    }
}
impl From<ProjectRackMacroMapping> for crate::sequencer::RackMacroMapping {
    fn from(value: ProjectRackMacroMapping) -> Self {
        Self {
            target: value.target.into(),
            range_min: value.range_min,
            range_max: value.range_max,
            curve: match value.curve {
                ProjectRackMacroCurve::Linear => crate::sequencer::RackMacroCurve::Linear,
                ProjectRackMacroCurve::Exp => crate::sequencer::RackMacroCurve::Exp,
                ProjectRackMacroCurve::Log => crate::sequencer::RackMacroCurve::Log,
            },
        }
    }
}
impl From<crate::sequencer::RackMacroTarget> for ProjectRackMacroTarget {
    fn from(value: crate::sequencer::RackMacroTarget) -> Self {
        match value {
            crate::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                Self::SlotParam { slot, param }
            }
            crate::sequencer::RackMacroTarget::SlotInstrumentParam {
                slot,
                param,
                param_index,
            } => Self::SlotInstrumentParam {
                slot,
                param,
                param_index,
            },
            crate::sequencer::RackMacroTarget::SlotEffectParam {
                slot,
                effect_slot,
                param,
                param_index,
            } => Self::SlotEffectParam {
                slot,
                effect_slot,
                param,
                param_index,
            },
        }
    }
}
impl From<ProjectRackMacroTarget> for crate::sequencer::RackMacroTarget {
    fn from(value: ProjectRackMacroTarget) -> Self {
        match value {
            ProjectRackMacroTarget::SlotParam { slot, param } => Self::SlotParam { slot, param },
            ProjectRackMacroTarget::SlotInstrumentParam {
                slot,
                param,
                param_index,
            } => Self::SlotInstrumentParam {
                slot,
                param,
                param_index,
            },
            ProjectRackMacroTarget::SlotEffectParam {
                slot,
                effect_slot,
                param,
                param_index,
            } => Self::SlotEffectParam {
                slot,
                effect_slot,
                param,
                param_index,
            },
        }
    }
}

impl From<RackSlotSnapshot> for ProjectRackSlotPattern {
    fn from(value: RackSlotSnapshot) -> Self {
        let (sample_path, sample_name) = value
            .sample_id
            .map(|(_, name, _)| (None, Some(name)))
            .unwrap_or((None, None));
        Self {
            instrument_type: ProjectInstrumentType::from(value.instrument_type),
            instrument_run_mode: ProjectCustomInstrumentRunMode::from(value.instrument_run_mode),
            instrument_base_note_offset: value.instrument_base_note_offset,
            pad_note: value.pad_note,
            choke_group: value.choke_group,
            gain: value.gain,
            pan: value.pan,
            mute: value.mute,
            solo: value.solo,
            max_polyphony: value.max_polyphony,
            param_plocks: value.param_plocks.rows,
            instrument_slot: ProjectEffectSlot::from(&value.instrument_slot),
            effect_slots: value
                .effect_slots
                .iter()
                .map(ProjectEffectSlot::from)
                .collect(),
            custom_effects: value.custom_effect_names,
            track_sound_state: ProjectTrackSoundState::from(value.track_sound_state),
            sample_path,
            sample_name,
        }
    }
}

impl From<ProjectRackSlotPattern> for RackSlotSnapshot {
    fn from(value: ProjectRackSlotPattern) -> Self {
        let sample_id = value
            .sample_name
            .filter(|name| !name.trim().is_empty())
            .map(|name| (-1, name, 44_100));
        let mut slot = Self {
            instrument_type: InstrumentType::from(value.instrument_type),
            instrument_run_mode: CustomInstrumentRunMode::from(value.instrument_run_mode),
            instrument_base_note_offset: value.instrument_base_note_offset,
            pad_note: value.pad_note,
            choke_group: value.choke_group,
            gain: value.gain,
            pan: value.pan.clamp(-1.0, 1.0),
            mute: value.mute,
            solo: value.solo,
            max_polyphony: value.max_polyphony.clamp(1, crate::voice::MAX_VOICES),
            param_plocks: RackSlotParamPlocks::from_rows(value.param_plocks),
            instrument_slot: value.instrument_slot.into_snapshot_with_node_ids(0, 0),
            effect_slots: value
                .effect_slots
                .into_iter()
                .map(|slot| slot.into_snapshot_with_node_ids(0, 0))
                .collect(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: value.custom_effects,
            track_sound_state: value.track_sound_state.into_track_sound_state(None),
            sample_id,
        };
        slot.normalize_effect_chain();
        slot
    }
}

impl From<ModConnection> for ProjectModConnection {
    fn from(value: ModConnection) -> Self {
        let destination = match value.destination {
            ModDestination::Track(track) => ProjectModDestination::Track(track),
            ModDestination::Bus(bus) => ProjectModDestination::Bus(bus.0),
        };
        Self {
            source_track: value.source_track,
            destination: Some(destination),
            dest_track: None,
            dest_input: value.dest_input,
        }
    }
}

impl From<ProjectModConnection> for ModConnection {
    fn from(value: ProjectModConnection) -> Self {
        let destination = match value.destination {
            Some(ProjectModDestination::Track(track)) => ModDestination::Track(track),
            Some(ProjectModDestination::Bus(bus)) => ModDestination::Bus(BusId(bus)),
            None => ModDestination::Track(value.dest_track.unwrap_or(0)),
        };
        Self {
            source_track: value.source_track,
            destination,
            dest_input: value.dest_input,
        }
    }
}

pub fn ensure_projects_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(projects_dir())
}

pub fn list_project_names() -> std::io::Result<Vec<String>> {
    let mut items: Vec<String> = list_project_entries()?
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    items.sort();
    Ok(items)
}

#[derive(Clone, Debug)]
pub struct ProjectListEntry {
    pub name: String,
    pub modified_at: Option<SystemTime>,
}

pub fn list_project_entries() -> std::io::Result<Vec<ProjectListEntry>> {
    let dir = projects_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            let modified_at = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok();
            items.push(ProjectListEntry {
                name: stem.to_string(),
                modified_at,
            });
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

pub fn save_project(name: &str, project: &ProjectFile) -> std::io::Result<PathBuf> {
    ensure_projects_dir()?;
    let file_name = sanitize_project_name(name);
    let path = projects_dir().join(format!("{file_name}.json"));
    let json = serde_json::to_string(project).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to serialize project '{}': {error}", path.display()),
        )
    })?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn load_project(name: &str) -> std::io::Result<ProjectFile> {
    let path = projects_dir().join(format!("{}.json", sanitize_project_name(name)));
    let src = std::fs::read_to_string(&path)?;
    serde_json::from_str(&src).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse project '{}': {error}", path.display()),
        )
    })
}

pub fn save_sound_preset(name: &str, sound: &ProjectSoundPreset) -> std::io::Result<PathBuf> {
    save_container_preset(Path::new(SOUNDS_DIR), "sound", "Sound", name, sound)
}

pub fn save_rack_preset(name: &str, preset: &ProjectSoundPreset) -> std::io::Result<PathBuf> {
    save_container_preset(
        Path::new(RACK_PRESETS_DIR),
        "rackpreset",
        "rack preset",
        name,
        preset,
    )
}

fn save_container_preset(
    directory: &Path,
    extension: &str,
    kind: &str,
    name: &str,
    preset: &ProjectSoundPreset,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join(format!("{}.{}", sanitize_project_name(name), extension));
    let json = serde_json::to_string(preset).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to serialize {kind} '{}': {error}", path.display()),
        )
    })?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn load_sound_preset(path: &Path) -> std::io::Result<ProjectSoundPreset> {
    load_container_preset(path, "Sound")
}

pub fn load_rack_preset(name: &str) -> std::io::Result<ProjectSoundPreset> {
    let path = rack_preset_path(name);
    load_container_preset(&path, "rack preset")
}

pub fn rack_preset_path(name: &str) -> PathBuf {
    Path::new(RACK_PRESETS_DIR).join(format!("{}.rackpreset", sanitize_project_name(name)))
}

fn load_container_preset(path: &Path, kind: &str) -> std::io::Result<ProjectSoundPreset> {
    let src = std::fs::read_to_string(path)?;
    let sound: ProjectSoundPreset = serde_json::from_str(&src).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse {kind} '{}': {error}", path.display()),
        )
    })?;
    if !matches!(sound.track.kind, ProjectTrackKind::Rack { .. }) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{kind} '{}' does not contain a rack track", path.display()),
        ));
    }
    Ok(sound)
}

pub fn list_rack_presets() -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(RACK_PRESETS_DIR)?;
    let mut names = std::fs::read_dir(RACK_PRESETS_DIR)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rackpreset"))
        .filter_map(|path| {
            let fallback = path.file_stem()?.to_str()?.to_owned();
            let preset = load_container_preset(&path, "rack preset").ok()?;
            let name = preset.metadata.name.trim();
            Some(if name.is_empty() {
                fallback
            } else {
                name.to_owned()
            })
        })
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

pub fn list_sound_presets() -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(SOUNDS_DIR)?;
    let mut paths = std::fs::read_dir(SOUNDS_DIR)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sound"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

pub fn sanitize_project_name(name: &str) -> String {
    let sanitized: String = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').to_string()
}

fn projects_dir() -> &'static Path {
    Path::new(PROJECTS_DIR)
}

pub fn project_file_version() -> u32 {
    PROJECT_FILE_VERSION
}

fn deserialize_step_data<'de, D>(deserializer: D) -> Result<Vec<Vec<[f32; NUM_PARAMS]>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = StepDataRepr::deserialize(deserializer)?;
    match raw {
        StepDataRepr::Dense(raw) => {
            let mut tracks = Vec::with_capacity(raw.len());
            for track in raw {
                let mut steps = Vec::with_capacity(track.len());
                for values in track {
                    steps.push(step_values_from_vec(values));
                }
                tracks.push(steps);
            }
            Ok(tracks)
        }
        StepDataRepr::Sparse(raw) => {
            let mut tracks = Vec::with_capacity(raw.len());
            for track in raw {
                let mut steps = vec![default_step_values(); MAX_STEPS];
                for entry in track.entries {
                    if entry.step < steps.len() {
                        steps[entry.step] = step_values_from_vec(entry.values);
                    }
                }
                tracks.push(steps);
            }
            Ok(tracks)
        }
    }
}

fn serialize_step_data<S>(
    step_data: &[Vec<[f32; NUM_PARAMS]>],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let default = default_step_values();
    let sparse = step_data
        .iter()
        .map(|track| SparseStepTrack {
            entries: track
                .iter()
                .enumerate()
                .filter_map(|(step, values)| {
                    (values != &default).then(|| SparseStepDataEntry {
                        step,
                        values: values.to_vec(),
                    })
                })
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>();
    sparse.serialize(serializer)
}

fn serialize_chord_snapshots<S>(
    chord_snapshots: &[Vec<Vec<f32>>],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let sparse = chord_snapshots
        .iter()
        .map(|track| SparseChordTrack {
            entries: track
                .iter()
                .enumerate()
                .filter_map(|(step, notes)| {
                    (!notes.is_empty()).then(|| SparseChordEntry {
                        step,
                        notes: notes.clone(),
                    })
                })
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>();
    sparse.serialize(serializer)
}

fn deserialize_chord_snapshots<'de, D>(deserializer: D) -> Result<Vec<Vec<Vec<f32>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = ChordSnapshotsRepr::deserialize(deserializer)?;
    match raw {
        ChordSnapshotsRepr::Dense(raw) => Ok(raw),
        ChordSnapshotsRepr::Sparse(raw) => {
            let mut tracks = Vec::with_capacity(raw.len());
            for track in raw {
                let mut steps = vec![Vec::new(); MAX_STEPS];
                for entry in track.entries {
                    if entry.step < steps.len() {
                        steps[entry.step] = entry.notes;
                    }
                }
                tracks.push(steps);
            }
            Ok(tracks)
        }
    }
}

fn serialize_timebase_plock_snapshots<S>(
    snapshots: &[Vec<Option<u32>>],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let sparse = snapshots
        .iter()
        .map(|track| SparseTimebaseTrack {
            entries: track
                .iter()
                .enumerate()
                .filter_map(|(step, value)| value.map(|value| SparseTimebaseEntry { step, value }))
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>();
    sparse.serialize(serializer)
}

fn deserialize_timebase_plock_snapshots<'de, D>(
    deserializer: D,
) -> Result<Vec<Vec<Option<u32>>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = TimebaseSnapshotsRepr::deserialize(deserializer)?;
    match raw {
        TimebaseSnapshotsRepr::Dense(raw) => Ok(raw),
        TimebaseSnapshotsRepr::Sparse(raw) => {
            let mut tracks = Vec::with_capacity(raw.len());
            for track in raw {
                let mut steps = vec![None; MAX_STEPS];
                for entry in track.entries {
                    if entry.step < steps.len() {
                        steps[entry.step] = Some(entry.value);
                    }
                }
                tracks.push(steps);
            }
            Ok(tracks)
        }
    }
}

fn default_accum_limit() -> f32 {
    48.0
}

fn default_max_polyphony() -> usize {
    6
}

fn default_true() -> bool {
    true
}

fn default_track_volume() -> f32 {
    crate::mixer_volume::default_fader()
}

fn default_num_steps() -> usize {
    16
}

fn default_timebase() -> u8 {
    Timebase::Sixteenth as u8
}

fn default_swing() -> f32 {
    50.0
}

fn default_swing_resolution() -> u8 {
    SwingResolution::Sixteenth as u8
}

fn default_master_volume() -> f32 {
    1.0
}

fn default_step_values() -> [f32; NUM_PARAMS] {
    let mut params = [0.0; NUM_PARAMS];
    for (idx, param) in crate::sequencer::StepParam::ALL.into_iter().enumerate() {
        params[idx] = param.default_value();
    }
    params
}

fn step_values_from_vec(values: Vec<f32>) -> [f32; NUM_PARAMS] {
    let mut params = default_step_values();
    if values.len() == NUM_PARAMS - 1 {
        for (idx, value) in values.into_iter().enumerate() {
            params[idx] = value;
        }
    } else if values.len() == NUM_PARAMS - 2 {
        for (idx, value) in values.into_iter().enumerate() {
            let target_idx = if idx >= crate::sequencer::StepParam::Pan.index() {
                idx + 1
            } else {
                idx
            };
            params[target_idx] = value;
        }
    } else {
        for (idx, value) in values.into_iter().enumerate().take(NUM_PARAMS) {
            params[idx] = value;
        }
    }
    params
}

#[derive(Clone, Serialize, Deserialize)]
struct SparseStepDataEntry {
    step: usize,
    values: Vec<f32>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SparseStepTrack {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entries: Vec<SparseStepDataEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StepDataRepr {
    Dense(Vec<Vec<Vec<f32>>>),
    Sparse(Vec<SparseStepTrack>),
}

#[derive(Clone, Serialize, Deserialize)]
struct SparseChordEntry {
    step: usize,
    notes: Vec<f32>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SparseChordTrack {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entries: Vec<SparseChordEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ChordSnapshotsRepr {
    Dense(Vec<Vec<Vec<f32>>>),
    Sparse(Vec<SparseChordTrack>),
}

#[derive(Clone, Serialize, Deserialize)]
struct SparseTimebaseEntry {
    step: usize,
    value: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct SparseTimebaseTrack {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entries: Vec<SparseTimebaseEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TimebaseSnapshotsRepr {
    Dense(Vec<Vec<Option<u32>>>),
    Sparse(Vec<SparseTimebaseTrack>),
}

#[derive(Serialize, Deserialize)]
struct SparseEffectSlotPlock {
    step: usize,
    param: usize,
    value: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    param_id: Option<ParamNodeId>,
}

#[derive(Serialize, Deserialize)]
struct SparseEffectSlotKeyLock {
    note: u8,
    param: usize,
    value: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    param_id: Option<ParamNodeId>,
}

#[derive(Serialize, Deserialize)]
struct SparseTensorParamPlock {
    step: usize,
    value: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
struct SparseProjectTensorParam {
    name: String,
    shape: Vec<usize>,
    cell_offset: usize,
    default: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plocks_sparse: Vec<SparseTensorParamPlock>,
}

#[derive(Serialize, Deserialize)]
struct SparseProjectEffectSlot {
    num_params: u32,
    defaults: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plocks_sparse: Vec<SparseEffectSlotPlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    key_locks_sparse: Vec<SparseEffectSlotKeyLock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tensor_params: Vec<SparseProjectTensorParam>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    param_node_indices: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    param_node_spans: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ir: Option<String>,
}

#[derive(Deserialize)]
struct DenseProjectEffectSlot {
    num_params: u32,
    defaults: Vec<f32>,
    plocks: Vec<Vec<Option<f32>>>,
    #[serde(default)]
    plock_param_ids: Vec<Vec<Option<ParamNodeId>>>,
    #[serde(default)]
    key_locks: std::collections::BTreeMap<u8, Vec<Option<f32>>>,
    #[serde(default)]
    key_lock_param_ids: std::collections::BTreeMap<u8, Vec<Option<ParamNodeId>>>,
    #[serde(default)]
    tensor_params: Vec<SparseProjectTensorParam>,
    param_node_indices: Vec<u32>,
    #[serde(default)]
    param_node_spans: Vec<u32>,
    #[serde(default)]
    ir: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProjectEffectSlotRepr {
    Sparse(SparseProjectEffectSlot),
    Dense(DenseProjectEffectSlot),
    Empty(()),
}

fn sparse_tensor_param_from_snapshot(tensor: &TensorParamSnapshot) -> SparseProjectTensorParam {
    let plocks_sparse = tensor
        .plocks
        .iter()
        .enumerate()
        .filter_map(|(step, value)| {
            value.as_ref().map(|value| SparseTensorParamPlock {
                step,
                value: value.clone(),
            })
        })
        .collect();
    SparseProjectTensorParam {
        name: tensor.name.clone(),
        shape: tensor.shape.clone(),
        cell_offset: tensor.cell_offset,
        default: tensor.default.clone(),
        plocks_sparse,
    }
}

fn tensor_snapshot_from_sparse(tensor: SparseProjectTensorParam) -> TensorParamSnapshot {
    let mut plocks = vec![None; MAX_STEPS];
    for entry in tensor.plocks_sparse {
        if entry.step < MAX_STEPS {
            plocks[entry.step] = Some(entry.value);
        }
    }
    TensorParamSnapshot {
        name: tensor.name,
        shape: tensor.shape,
        cell_offset: tensor.cell_offset,
        default: tensor.default,
        plocks,
    }
}

impl Serialize for ProjectEffectSlot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let plocks_sparse = self
            .plocks
            .iter()
            .enumerate()
            .flat_map(|(step, row)| {
                row.iter().enumerate().filter_map(move |(param, value)| {
                    value.map(|value| SparseEffectSlotPlock {
                        step,
                        param,
                        value,
                        param_id: self
                            .plock_param_ids
                            .get(step)
                            .and_then(|ids| ids.get(param))
                            .copied()
                            .flatten(),
                    })
                })
            })
            .collect::<Vec<_>>();
        let tensor_params = self
            .tensor_params
            .iter()
            .map(sparse_tensor_param_from_snapshot)
            .collect::<Vec<_>>();
        let key_locks_sparse = self
            .key_locks
            .iter()
            .flat_map(|(&note, row)| {
                row.iter().enumerate().filter_map(move |(param, value)| {
                    value.map(|value| SparseEffectSlotKeyLock {
                        note,
                        param,
                        value,
                        param_id: self
                            .key_lock_param_ids
                            .get(&note)
                            .and_then(|ids| ids.get(param))
                            .copied()
                            .flatten(),
                    })
                })
            })
            .collect::<Vec<_>>();

        if self.num_params == 0
            && self.defaults.is_empty()
            && self.param_node_indices.is_empty()
            && plocks_sparse.is_empty()
            && key_locks_sparse.is_empty()
            && tensor_params.is_empty()
            && self.ir.is_none()
        {
            return Option::<()>::None.serialize(serializer);
        }

        SparseProjectEffectSlot {
            num_params: self.num_params,
            defaults: self.defaults.clone(),
            plocks_sparse,
            key_locks_sparse,
            tensor_params,
            param_node_indices: self.param_node_indices.clone(),
            param_node_spans: self.param_node_spans.clone(),
            ir: self.ir.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProjectEffectSlot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let repr = ProjectEffectSlotRepr::deserialize(deserializer)?;
        Ok(match repr {
            ProjectEffectSlotRepr::Sparse(slot) => {
                let mut plocks = vec![vec![None; slot.defaults.len()]; MAX_STEPS];
                let mut plock_param_ids = vec![vec![None; slot.defaults.len()]; MAX_STEPS];
                for entry in slot.plocks_sparse {
                    if entry.step < plocks.len() && entry.param < slot.defaults.len() {
                        plocks[entry.step][entry.param] = Some(entry.value);
                        plock_param_ids[entry.step][entry.param] = entry.param_id;
                    }
                }
                let mut key_locks = std::collections::BTreeMap::new();
                let mut key_lock_param_ids = std::collections::BTreeMap::new();
                for entry in slot.key_locks_sparse {
                    if entry.param >= slot.defaults.len() {
                        continue;
                    }
                    let row = key_locks
                        .entry(entry.note)
                        .or_insert_with(|| vec![None; slot.defaults.len()]);
                    row[entry.param] = Some(entry.value);
                    let id_row = key_lock_param_ids
                        .entry(entry.note)
                        .or_insert_with(|| vec![None; slot.defaults.len()]);
                    id_row[entry.param] = entry.param_id;
                }
                Self {
                    num_params: slot.num_params,
                    defaults: slot.defaults,
                    plocks,
                    plock_param_ids,
                    key_locks,
                    key_lock_param_ids,
                    tensor_params: slot
                        .tensor_params
                        .into_iter()
                        .map(tensor_snapshot_from_sparse)
                        .collect(),
                    param_node_indices: slot.param_node_indices,
                    param_node_spans: slot.param_node_spans,
                    ir: slot.ir,
                }
            }
            ProjectEffectSlotRepr::Dense(slot) => Self {
                num_params: slot.num_params,
                defaults: slot.defaults,
                plocks: slot.plocks,
                plock_param_ids: slot.plock_param_ids,
                key_locks: slot.key_locks,
                key_lock_param_ids: slot.key_lock_param_ids,
                tensor_params: slot
                    .tensor_params
                    .into_iter()
                    .map(tensor_snapshot_from_sparse)
                    .collect(),
                param_node_indices: slot.param_node_indices,
                param_node_spans: slot.param_node_spans,
                ir: slot.ir,
            },
            ProjectEffectSlotRepr::Empty(_) => Self {
                num_params: 0,
                defaults: Vec::new(),
                plocks: vec![Vec::new(); MAX_STEPS],
                plock_param_ids: vec![Vec::new(); MAX_STEPS],
                key_locks: std::collections::BTreeMap::new(),
                key_lock_param_ids: std::collections::BTreeMap::new(),
                tensor_params: Vec::new(),
                param_node_indices: Vec::new(),
                param_node_spans: Vec::new(),
                ir: None,
            },
        })
    }
}

pub fn chord_snapshot_from_steps(steps: Vec<Vec<f32>>) -> ChordSnapshot {
    let durations = steps.iter().map(|notes| vec![0.0; notes.len()]).collect();
    let delays = steps.iter().map(|notes| vec![0.0; notes.len()]).collect();
    ChordSnapshot {
        steps,
        durations,
        delays,
    }
}

pub fn chord_snapshot_from_steps_durations_and_delays(
    steps: Vec<Vec<f32>>,
    mut durations: Vec<Vec<f32>>,
    mut delays: Vec<Vec<f32>>,
) -> ChordSnapshot {
    durations.resize_with(steps.len(), Vec::new);
    delays.resize_with(steps.len(), Vec::new);
    for (idx, notes) in steps.iter().enumerate() {
        durations[idx].resize(notes.len(), 0.0);
        delays[idx].resize(notes.len(), 0.0);
        for delay in &mut delays[idx] {
            *delay = delay.clamp(
                crate::sequencer::StepParam::Delay.min(),
                crate::sequencer::StepParam::Delay.max(),
            );
        }
    }
    ChordSnapshot {
        steps,
        durations,
        delays,
    }
}

pub fn chord_snapshot_from_steps_and_durations(
    steps: Vec<Vec<f32>>,
    durations: Vec<Vec<f32>>,
) -> ChordSnapshot {
    let delays = steps.iter().map(|notes| vec![0.0; notes.len()]).collect();
    chord_snapshot_from_steps_durations_and_delays(steps, durations, delays)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project() -> ProjectFile {
        ProjectFile {
            version: project_file_version(),
            name: "roundtrip".to_string(),
            bpm: 120,
            master_volume: 1.0,
            current_pattern: 0,
            current_track: Some(1),
            reverb: ProjectReverbState {
                size: 0.2,
                brightness: 0.8,
                replace: 0.3,
            },
            buses: default_project_buses(),
            groups: Vec::new(),
            tracks: vec![
                ProjectTrack {
                    id: TrackId(1),
                    color: Some(TrackColor::new(0.96, 0.28, 0.52)),
                    collapsed: true,
                    kind: ProjectTrackKind::Custom {
                        instrument_name: "prophet-5".to_string(),
                    },
                },
                ProjectTrack {
                    id: TrackId(2),
                    color: Some(TrackColor::new(0.98, 0.56, 0.20)),
                    collapsed: false,
                    kind: ProjectTrackKind::Sampler {
                        sample_path: "samples/drums/kick.wav".to_string(),
                    },
                },
            ],
            custom_effects: vec![vec![Some("widener".to_string()), None], vec![None, None]],
            scratch: ProjectScratchState {
                buffer: "(+ 1 2)".to_string(),
                cursor_row: 3,
                cursor_col: 7,
            },
            patterns: vec![ProjectPattern {
                track_bits: vec![[0b1011, 0, 0, 0], [0b0101, 1, 0, 0]],
                neural_reset_bits: vec![[0b0010, 0, 0, 0], [0, 0, 0, 0]],
                step_data: vec![vec![default_step_values(); 256]; 2],
                track_params: vec![
                    ProjectTrackParams {
                        gate: true,
                        attack_ms: 0.0,
                        release_ms: 10.0,
                        swing: 50.0,
                        swing_resolution: SwingResolution::Sixteenth as u8,
                        num_steps: 64,
                        volume: 0.8,
                        pan: -0.25,
                        mute: false,
                        solo: true,
                        send: 0.25,
                        output: ProjectTrackOutput::Bus {
                            id: crate::sequencer::DEFAULT_BUS_A_ID,
                        },
                        sends: vec![ProjectTrackSend {
                            destination: crate::sequencer::DEFAULT_BUS_B_ID,
                            amount: 0.4,
                        }],
                        polyphonic: true,
                        max_polyphony: 6,
                        timebase: Timebase::Sixteenth as u8,
                        accumulator_idx: 1,
                        script_accumulator_name: None,
                        midi_fx_chain: Vec::new(),
                        midi_fx_position: ProjectMidiFxPosition::PostAccumulator,
                        accum_limit: 24.0,
                        accum_mode: 2,
                        fts_scale: 0,
                        mute_group: 3,
                        global_transpose: true,
                    },
                    ProjectTrackParams {
                        gate: false,
                        attack_ms: 5.0,
                        release_ms: 25.0,
                        swing: 55.0,
                        swing_resolution: SwingResolution::Quarter as u8,
                        num_steps: 128,
                        volume: 1.1,
                        pan: 0.4,
                        mute: true,
                        solo: false,
                        send: 0.5,
                        output: ProjectTrackOutput::Mix,
                        sends: Vec::new(),
                        polyphonic: false,
                        max_polyphony: 6,
                        timebase: Timebase::Eighth as u8,
                        accumulator_idx: 0,
                        script_accumulator_name: None,
                        midi_fx_chain: Vec::new(),
                        midi_fx_position: ProjectMidiFxPosition::PostAccumulator,
                        accum_limit: 48.0,
                        accum_mode: 0,
                        fts_scale: 0,
                        mute_group: 0,
                        global_transpose: true,
                    },
                ],
                effect_slots: vec![vec![], vec![]],
                midi_fx_slots: vec![vec![], vec![]],
                instrument_slots: vec![
                    ProjectEffectSlot {
                        num_params: 2,
                        defaults: vec![0.1, 0.2],
                        plocks: vec![vec![None, Some(0.8)]; 256],
                        plock_param_ids: vec![vec![None, None]; 256],
                        key_locks: std::collections::BTreeMap::new(),
                        key_lock_param_ids: std::collections::BTreeMap::new(),
                        param_node_indices: vec![0, 1],
                        param_node_spans: vec![1, 1],
                        tensor_params: Vec::new(),
                        ir: None,
                    },
                    ProjectEffectSlot {
                        num_params: 0,
                        defaults: vec![],
                        plocks: vec![vec![]; 256],
                        plock_param_ids: vec![vec![]; 256],
                        key_locks: std::collections::BTreeMap::new(),
                        key_lock_param_ids: std::collections::BTreeMap::new(),
                        param_node_indices: vec![],
                        param_node_spans: vec![],
                        tensor_params: Vec::new(),
                        ir: None,
                    },
                ],
                instrument_base_note_offsets: vec![0.0, 12.0],
                track_sound_states: vec![
                    ProjectTrackSoundState {
                        loaded_preset: Some("lead".to_string()),
                        dirty: false,
                    },
                    ProjectTrackSoundState {
                        loaded_preset: None,
                        dirty: true,
                    },
                ],
                chord_snapshots: {
                    let mut snapshots = vec![vec![Vec::new(); 256], vec![Vec::new(); 256]];
                    snapshots[1][3] = vec![60.0, 64.0, 67.0];
                    snapshots
                },
                chord_duration_snapshots: {
                    let mut snapshots = vec![vec![Vec::new(); 256], vec![Vec::new(); 256]];
                    snapshots[1][3] = vec![1.0, 0.75, 0.5];
                    snapshots
                },
                chord_delay_snapshots: {
                    let mut snapshots = vec![vec![Vec::new(); 256], vec![Vec::new(); 256]];
                    snapshots[1][3] = vec![0.0, 0.25, 0.5];
                    snapshots
                },
                timebase_plock_snapshots: vec![vec![None; 256], vec![None; 256]],
                swing_plock_snapshots: vec![vec![None; 256], vec![None; 256]],
                swing_resolution_plock_snapshots: vec![vec![None; 256], vec![None; 256]],
                bus_patterns: Vec::new(),
                instrument_types: vec![
                    ProjectInstrumentType::Custom,
                    ProjectInstrumentType::Sampler,
                ],
                instrument_run_modes: vec![
                    ProjectCustomInstrumentRunMode::FreePatch,
                    ProjectCustomInstrumentRunMode::Instrument,
                ],
                mod_connections: vec![ProjectModConnection {
                    source_track: 0,
                    destination: Some(ProjectModDestination::Track(1)),
                    dest_track: None,
                    dest_input: 2,
                }],
                neural_networks: {
                    let mut network = ProjectNeuralNetwork::default();
                    network.id = 42;
                    network.name = "drum-net".to_string();
                    network.enabled = true;
                    network.num_neurons = 6;
                    network.neurons[0].route = Some(1);
                    network.neurons[0].output_overrides.instrument =
                        vec![crate::neural::ProjectParamOverride {
                            target_track: 1,
                            param_id: crate::neural::ParamNodeId {
                                logical_id: 7,
                                node_param_idx: 3,
                            },
                            param_index: 2,
                            value: 0.75,
                        }];
                    network.neurons[0].output_overrides.effects =
                        vec![crate::neural::ProjectEffectParamOverride {
                            target_track: 1,
                            slot_index: 0,
                            param_id: crate::neural::ParamNodeId {
                                logical_id: 11,
                                node_param_idx: 5,
                            },
                            param_index: 4,
                            value: 0.33,
                        }];
                    vec![network]
                },
                graph_overrides: vec![ProjectGraphOverrides {
                    sequencer_id: 99,
                    sequencer_name: "neural".to_string(),
                    node_intrinsics: vec![crate::graph::ProjectGraphNodeIntrinsicOverride {
                        group: "nrn".to_string(),
                        instance: 1,
                        resolution: Some(vec![Timebase::Eighth as u8]),
                        delay_steps: Some(3),
                        quantize: Some(crate::graph::ProjectGraphQuantizeOverride::Off),
                        route: Some(crate::graph::ProjectGraphRouteOverride::Track(1)),
                        seed_from: Some(crate::graph::ProjectGraphSeedFrom::Tracks(vec![0])),
                        seed_on_reset: None,
                        duration: None,
                        swing: None,
                    }],
                    node_params: vec![crate::graph::ProjectGraphNodeParamOverride {
                        group: "nrn".to_string(),
                        instance: 1,
                        param: "threshold".to_string(),
                        value: 0.75,
                    }],
                    edge_params: vec![crate::graph::ProjectGraphEdgeParamOverride {
                        group: "nrn->nrn".to_string(),
                        from: 0,
                        to: 1,
                        param: "weight".to_string(),
                        value: 0.5,
                    }],
                    reset_every_beats: None,
                    max_poly: None,
                    max_poly_selection: None,
                    node_count: Some(12),
                }],
                sample_paths: vec![None, Some("samples/drums/kick.wav".to_string())],
                sample_names: vec!["prophet-5".to_string(), "kick".to_string()],
                rack_tracks: vec![None, None],
                process_chains: vec![
                    crate::process::TrackProcessChain {
                        slots: vec![crate::process::TrackProcessSlot {
                            instance_id: crate::process::ProcessInstanceId(42),
                            instance_name: Some("json-sparse-h".to_string()),
                            class_name: "json-sparse".to_string(),
                            enabled: true,
                            project_layer: false,
                            inlets: std::collections::BTreeMap::new(),
                            lanes: std::collections::BTreeMap::from([(
                                "amount".to_string(),
                                crate::process::ProcessLane {
                                    values: vec![0.0, 1.0, 0.0, 2.0],
                                },
                            )]),
                            bindings: std::collections::BTreeMap::from([(
                                "shape".to_string(),
                                Some(crate::process::ParamTarget::InstrumentParam {
                                    param: "release".to_string(),
                                    param_id: None,
                                }),
                            )]),
                        }],
                    },
                    crate::process::TrackProcessChain::default(),
                ],
                project_process_lane_overrides: Vec::new(),
                project_process_chain: crate::process::TrackProcessChain {
                    slots: vec![crate::process::TrackProcessSlot {
                        instance_id: crate::process::ProcessInstanceId(77),
                        instance_name: Some("json-project-mask-h".to_string()),
                        class_name: "json-project-mask".to_string(),
                        enabled: true,
                        project_layer: true,
                        inlets: std::collections::BTreeMap::new(),
                        lanes: std::collections::BTreeMap::from([(
                            "prob".to_string(),
                            crate::process::ProcessLane {
                                values: vec![1.0, 0.5],
                            },
                        )]),
                        bindings: std::collections::BTreeMap::new(),
                    }],
                },
                plock_variant_registries: Vec::new(),
                key_lock_variant_registries: Vec::new(),
            }],
            macros: Vec::new(),
            next_macro_id: 1,
        }
    }

    #[test]
    fn current_project_format_roundtrips() {
        let mut project = sample_project();
        let identity = crate::process::project_slot_identity_id(
            &project.patterns[0].project_process_chain.slots[0],
        );
        project.patterns[0].project_process_lane_overrides =
            vec![std::collections::BTreeMap::from([(
                identity,
                std::collections::BTreeMap::from([(
                    "prob".to_string(),
                    crate::process::ProcessLane {
                        values: vec![0.25, 0.75],
                    },
                )]),
            )])];
        let json = serde_json::to_string(&project).expect("serialize current project");
        let restored: ProjectFile =
            serde_json::from_str(&json).expect("deserialize current project");

        assert_eq!(restored.name, project.name);
        assert_eq!(restored.current_pattern, project.current_pattern);
        assert_eq!(restored.current_track, project.current_track);
        assert_eq!(
            restored.patterns[0].project_process_lane_overrides,
            project.patterns[0].project_process_lane_overrides
        );
        assert_eq!(restored.scratch.buffer, project.scratch.buffer);
        assert_eq!(restored.scratch.cursor_row, project.scratch.cursor_row);
        assert_eq!(restored.scratch.cursor_col, project.scratch.cursor_col);
        assert_eq!(
            restored.tracks[0].color(),
            Some(TrackColor::new(0.96, 0.28, 0.52))
        );
        assert_eq!(
            restored.tracks[1].color(),
            Some(TrackColor::new(0.98, 0.56, 0.20))
        );
        assert!(restored.tracks[0].collapsed());
        assert!(!restored.tracks[1].collapsed());
        assert_eq!(restored.tracks[0].id, TrackId(1));
        assert_eq!(restored.tracks[1].id, TrackId(2));
        assert_eq!(restored.patterns.len(), 1);
        assert_eq!(restored.patterns[0].track_bits[0], [0b1011, 0, 0, 0]);
        assert_eq!(restored.patterns[0].track_bits[1], [0b0101, 1, 0, 0]);
        assert_eq!(restored.patterns[0].neural_reset_bits[0], [0b0010, 0, 0, 0]);
        assert_eq!(restored.patterns[0].step_data[0].len(), 256);
        assert_eq!(
            restored.patterns[0].chord_snapshots[1][3],
            vec![60.0, 64.0, 67.0]
        );
        assert_eq!(
            restored.patterns[0].chord_duration_snapshots[1][3],
            vec![1.0, 0.75, 0.5]
        );
        assert_eq!(
            restored.patterns[0].chord_delay_snapshots[1][3],
            vec![0.0, 0.25, 0.5]
        );
        assert_eq!(restored.patterns[0].timebase_plock_snapshots[0].len(), 256);
        assert_eq!(
            restored.patterns[0].process_chains[0].slots[0].instance_id,
            crate::process::ProcessInstanceId(42)
        );
        assert_eq!(
            restored.patterns[0].process_chains[0].slots[0]
                .instance_name
                .as_deref(),
            Some("json-sparse-h")
        );
        assert_eq!(
            restored.patterns[0].process_chains[0].slots[0].class_name,
            "json-sparse"
        );
        assert_eq!(
            restored.patterns[0].process_chains[0].slots[0].lanes["amount"].values,
            vec![0.0, 1.0, 0.0, 2.0]
        );
        assert_eq!(
            restored.patterns[0].process_chains[0].slots[0].bindings["shape"],
            Some(crate::process::ParamTarget::InstrumentParam {
                param: "release".to_string(),
                param_id: None,
            })
        );
        let project_slot = &restored.patterns[0].project_process_chain.slots[0];
        assert_eq!(
            project_slot.instance_id,
            crate::process::ProcessInstanceId(77)
        );
        assert_eq!(project_slot.class_name, "json-project-mask");
        assert!(project_slot.project_layer);
        assert_eq!(project_slot.lanes["prob"].values, vec![1.0, 0.5]);
        assert_eq!(restored.patterns[0].track_params[0].accumulator_idx, 1);
        assert_eq!(restored.patterns[0].track_params[0].accum_limit, 24.0);
        assert_eq!(restored.patterns[0].track_params[0].accum_mode, 2);
        assert_eq!(restored.patterns[0].track_params[0].mute_group, 3);
        assert_eq!(
            restored.patterns[0].mod_connections,
            vec![ProjectModConnection {
                source_track: 0,
                destination: Some(ProjectModDestination::Track(1)),
                dest_track: None,
                dest_input: 2,
            }]
        );
        assert_eq!(
            restored.patterns[0].instrument_run_modes,
            vec![
                ProjectCustomInstrumentRunMode::FreePatch,
                ProjectCustomInstrumentRunMode::Instrument,
            ]
        );
        assert_eq!(restored.patterns[0].neural_networks.len(), 1);
        let network = &restored.patterns[0].neural_networks[0];
        assert_eq!(network.id, 42);
        assert_eq!(network.name, "drum-net");
        assert_eq!(network.num_neurons, 6);
        assert_eq!(network.neurons[0].route, Some(1));
        assert_eq!(
            network.neurons[0].output_overrides.instrument[0].target_track,
            1
        );
        assert_eq!(
            network.neurons[0].output_overrides.instrument[0].param_id,
            crate::neural::ParamNodeId {
                logical_id: 7,
                node_param_idx: 3,
            }
        );
        assert_eq!(
            network.neurons[0].output_overrides.effects[0].target_track,
            1
        );
        assert_eq!(
            network.neurons[0].output_overrides.effects[0].param_id,
            crate::neural::ParamNodeId {
                logical_id: 11,
                node_param_idx: 5,
            }
        );
        assert_eq!(restored.patterns[0].graph_overrides.len(), 1);
        let graph = &restored.patterns[0].graph_overrides[0];
        assert_eq!(graph.sequencer_name, "neural");
        assert_eq!(graph.node_intrinsics[0].delay_steps, Some(3));
        assert_eq!(graph.node_params[0].param, "threshold");
        assert_eq!(graph.node_params[0].value, 0.75);
        assert_eq!(graph.edge_params[0].group, "nrn->nrn");
        assert_eq!(graph.edge_params[0].value, 0.5);
        assert_eq!(graph.node_count, Some(12));
        assert!(restored.patterns[0].track_params[0].solo);
        assert!(restored.patterns[0].track_params[1].mute);
        assert_eq!(restored.buses.len(), 3);
        assert_eq!(restored.buses[0].id, crate::sequencer::MIX_BUS_ID);
        assert!(matches!(
            restored.patterns[0].track_params[0].output,
            ProjectTrackOutput::Bus { id } if id == crate::sequencer::DEFAULT_BUS_A_ID
        ));
        assert_eq!(
            restored.patterns[0].track_params[0].sends[0].destination,
            crate::sequencer::DEFAULT_BUS_B_ID
        );
    }

    #[test]
    fn project_track_id_migration_is_deterministic_and_validates_current_ids() {
        let mut legacy = serde_json::to_value(sample_project()).expect("serialize legacy fixture");
        legacy["version"] = serde_json::json!(2);
        for track in legacy["tracks"]
            .as_array_mut()
            .expect("project tracks array")
        {
            track.as_object_mut().expect("project track object").remove("id");
        }
        let migrated: ProjectFile =
            serde_json::from_value(legacy).expect("migrate tracks without stable ids");
        assert_eq!(migrated.tracks[0].id, TrackId(1));
        assert_eq!(migrated.tracks[1].id, TrackId(2));

        let mut missing = serde_json::to_value(sample_project()).expect("serialize v3 fixture");
        missing["tracks"][0]["id"] = serde_json::json!(42);
        missing["tracks"][1]
            .as_object_mut()
            .expect("second track object")
            .remove("id");
        let filled: ProjectFile =
            serde_json::from_value(missing).expect("fill one missing current track id");
        assert_eq!(filled.tracks[0].id, TrackId(42));
        assert_eq!(filled.tracks[1].id, TrackId(1));

        let mut duplicate = serde_json::to_value(sample_project()).expect("serialize v3 fixture");
        duplicate["tracks"][1]["id"] = duplicate["tracks"][0]["id"].clone();
        let error = match serde_json::from_value::<ProjectFile>(duplicate) {
            Ok(_) => panic!("duplicate current track ids must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("duplicate stable track id"));
    }

    #[test]
    fn project_macros_roundtrip_stable_ids_ranges_curves_and_identity() {
        let mut project = sample_project();
        project.next_macro_id = 9;
        project.macros = vec![ProjectMacro {
            id: 7,
            key: Some("player/delay-push".to_string()),
            name: "Push".to_string(),
            value: 0.75,
            kind: ProjectMacroKind::Mapped,
            mappings: vec![
                ProjectMacroMapping {
                    scope: ProjectParamScope::Track(1),
                    target: crate::process::ParamTarget::EffectParam {
                        slot: 2,
                        effect: "delay".to_string(),
                        param: "feedback".to_string(),
                        param_id: Some(ParamNodeId {
                            logical_id: 91,
                            node_param_idx: 4,
                        }),
                    },
                    range_min: 0.2,
                    range_max: 0.93,
                    curve: ProjectMacroCurve::Exp,
                },
                ProjectMacroMapping {
                    scope: ProjectParamScope::Bus(crate::sequencer::DEFAULT_BUS_A_ID),
                    target: crate::process::ParamTarget::EffectParam {
                        slot: 0,
                        effect: "reverb".to_string(),
                        param: "mix".to_string(),
                        param_id: None,
                    },
                    range_min: 0.0,
                    range_max: 0.7,
                    curve: ProjectMacroCurve::Linear,
                },
            ],
        }];

        let json = serde_json::to_string(&project).expect("serialize macros");
        let restored: ProjectFile = serde_json::from_str(&json).expect("deserialize macros");
        assert_eq!(restored.next_macro_id, 9);
        assert_eq!(restored.macros, project.macros);
        assert_eq!(restored.macros[0].key.as_deref(), Some("player/delay-push"));

        let macro_definition = Macro::try_from(restored.macros[0].clone()).expect("valid macro");
        assert_eq!(macro_definition.id, 7);
        assert_eq!(macro_definition.mappings[0].curve, MacroCurve::Exp);
        assert_eq!(
            macro_definition.mappings[1].scope,
            crate::macro_engine::ParamScope::Bus(crate::sequencer::BusId::DEFAULT_A)
        );
    }

    #[test]
    fn scene_macro_persistence_excludes_transient_diff_mappings() {
        let mut definition = Macro::new(
            9,
            "Scene Push",
            MacroKind::Scene(SceneMacroConfig {
                target_scene: 2,
                morph_params: true,
                steal_patterns: true,
                quantize: StealQuantize::Bar,
                track_mask: Some(vec![true, false]),
            }),
        );
        definition.mappings.push(
            MacroMapping::new(
                0,
                crate::process::ParamTarget::EffectParam {
                    slot: 0,
                    effect: "filter".to_string(),
                    param: "cutoff".to_string(),
                    param_id: None,
                },
                100.0,
                1_000.0,
                MacroCurve::LogDomain,
            )
            .unwrap(),
        );
        let persisted = ProjectMacro::from(&definition);
        assert!(persisted.mappings.is_empty());
        assert!(matches!(persisted.kind, ProjectMacroKind::Scene(_)));
    }

    #[test]
    fn project_macro_without_script_key_loads_as_interactive_macro() {
        let json = r#"{
            "id": 4,
            "name": "Interactive",
            "value": 0.0,
            "kind": "Mapped",
            "mappings": []
        }"#;
        let restored: ProjectMacro = serde_json::from_str(json).expect("load unkeyed macro");

        assert_eq!(restored.id, 4);
        assert_eq!(restored.key, None);
    }

    #[test]
    fn script_key_survives_user_rename_and_project_conversion() {
        let mut engine = crate::macro_engine::MacroEngine::default();
        let id = engine
            .ensure_macro(":Player/Filter", "Filter")
            .expect("ensure");
        engine.rename_macro(id, "Opening Filter").expect("rename");

        let persisted = ProjectMacro::from(engine.macro_definition(id).unwrap());
        let restored = Macro::try_from(persisted).expect("restore macro");
        assert_eq!(restored.key.as_deref(), Some("player/filter"));
        assert_eq!(restored.name, "Opening Filter");
    }

    #[test]
    fn projects_without_macro_fields_load_empty_with_initial_id_cursor() {
        let project = sample_project();
        let mut json = serde_json::to_value(project).expect("serialize project");
        let object = json.as_object_mut().expect("project object");
        object.remove("macros");
        object.remove("next_macro_id");

        let restored: ProjectFile = serde_json::from_value(json).expect("load old project");
        assert!(restored.macros.is_empty());
        assert_eq!(restored.next_macro_id, 1);
    }

    #[test]
    fn graph_overrides_missing_node_count_loads_as_manifest_default() {
        let json = r#"{
            "sequencer_id": 7,
            "sequencer_name": "legacy",
            "node_intrinsics": [],
            "node_params": [],
            "edge_params": []
        }"#;
        let overrides: ProjectGraphOverrides =
            serde_json::from_str(json).expect("deserialize legacy graph overrides");
        assert_eq!(overrides.node_count, None);
    }

    #[test]
    fn current_pattern_current_track_and_note_delays_roundtrip() {
        let mut project = sample_project();
        let current_pattern = project.patterns[0].clone();
        let mut previous_pattern = current_pattern.clone();
        previous_pattern.track_bits =
            vec![[0; TRACK_PATTERN_WORDS]; previous_pattern.track_bits.len()];
        previous_pattern.chord_snapshots =
            vec![vec![Vec::new(); MAX_STEPS]; previous_pattern.chord_snapshots.len()];
        previous_pattern.chord_duration_snapshots =
            vec![vec![Vec::new(); MAX_STEPS]; previous_pattern.chord_duration_snapshots.len()];
        previous_pattern.chord_delay_snapshots =
            vec![vec![Vec::new(); MAX_STEPS]; previous_pattern.chord_delay_snapshots.len()];

        project.patterns = vec![previous_pattern, current_pattern];
        project.current_pattern = 1;
        project.current_track = Some(1);

        let json = serde_json::to_string(&project).expect("serialize project");
        let restored: ProjectFile = serde_json::from_str(&json).expect("deserialize project");
        let current = &restored.patterns[restored.current_pattern];

        assert_eq!(restored.current_track, Some(1));
        assert_eq!(current.track_bits[1], [0b0101, 1, 0, 0]);
        assert_eq!(current.chord_snapshots[1][3], vec![60.0, 64.0, 67.0]);
        assert_eq!(current.chord_delay_snapshots[1][3], vec![0.0, 0.25, 0.5]);
    }

    #[test]
    fn legacy_step_data_without_pan_deserializes() {
        let json = r#"{
            "version": 1,
            "name": "legacy",
            "bpm": 120,
            "master_volume": 1.0,
            "current_pattern": 0,
            "reverb": {"size": 0.2, "brightness": 0.8, "replace": 0.3},
            "tracks": [{"kind": "sampler", "sample_path": "samples/kick.wav"}],
            "custom_effects": [[]],
            "scratch": {"buffer": "", "cursor_row": 0, "cursor_col": 0},
            "patterns": [{
                "track_bits": [[0,0,0,0]],
                "step_data": [[[1.0, 0.5, 1.0, 0.0, 0.0, 7.0, 1.0, 0.0]]],
                "track_params": [{
                    "gate": true,
                    "attack_ms": 0.0,
                    "release_ms": 0.0,
                    "swing": 50.0,
                    "swing_resolution": 4,
                    "num_steps": 16,
                    "volume": 1.0,
                    "send": 0.0,
                    "polyphonic": true,
                    "timebase": 4,
                    "accumulator_idx": 0,
                    "accum_limit": 48.0,
                    "accum_mode": 0,
                    "fts_scale": 0
                }],
                "effect_slots": [[]],
                "instrument_slots": [{"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]}],
                "instrument_base_note_offsets": [0.0],
                "track_sound_states": [{"loaded_preset": null, "dirty": false}],
                "chord_snapshots": [[[]]],
                "timebase_plock_snapshots": [[null]],
                "instrument_types": ["sampler"],
                "sample_paths": ["samples/kick.wav"],
                "sample_names": ["kick"]
            }]
        }"#;

        let project: ProjectFile = serde_json::from_str(json).expect("deserialize legacy project");
        let step = project.patterns[0].step_data[0][0];
        assert_eq!(project.current_track, None);
        assert_eq!(step[crate::sequencer::StepParam::Transpose.index()], 7.0);
        assert_eq!(step[crate::sequencer::StepParam::Pan.index()], 0.0);
        assert_eq!(step[crate::sequencer::StepParam::Chop.index()], 1.0);
        assert_eq!(step[crate::sequencer::StepParam::Delay.index()], 0.0);
        assert!(project.patterns[0].graph_overrides.is_empty());
    }

    #[test]
    fn legacy_project_tracks_without_colors_deserialize() {
        let sampler: ProjectTrack =
            serde_json::from_str(r#"{"kind":"sampler","sample_path":"samples/kick.wav"}"#)
                .expect("deserialize legacy sampler track");
        let custom: ProjectTrack =
            serde_json::from_str(r#"{"kind":"custom","instrument_name":"prophet-5"}"#)
                .expect("deserialize legacy custom track");

        assert_eq!(sampler.color(), None);
        assert_eq!(custom.color(), None);
    }

    #[test]
    fn legacy_project_defaults_missing_instrument_run_modes() {
        let json = r#"{
            "version": 1,
            "name": "legacy",
            "bpm": 120,
            "current_pattern": 0,
            "reverb": {"size": 0.2, "brightness": 0.8, "replace": 0.3},
            "tracks": [{"kind":"custom","instrument_name":"lead"}],
            "custom_effects": [[]],
            "patterns": [{
                "track_bits": [[0,0,0,0]],
                "step_data": [[[1.0,0.5,1.0,0.0,0.0,0.0,1.0,0.0]]],
                "track_params": [{
                    "gate": true,
                    "attack_ms": 0.0,
                    "release_ms": 0.0,
                    "swing": 50.0,
                    "num_steps": 16,
                    "send": 0.0,
                    "polyphonic": true,
                    "timebase": 4
                }],
                "effect_slots": [[]],
                "instrument_slots": [{"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]}],
                "instrument_base_note_offsets": [0.0],
                "track_sound_states": [{"loaded_preset": null, "dirty": false}],
                "chord_snapshots": [[[]]],
                "timebase_plock_snapshots": [[null]],
                "instrument_types": ["custom"],
                "sample_paths": [null],
                "sample_names": ["lead"]
            }]
        }"#;

        let project: ProjectFile = serde_json::from_str(json).expect("deserialize legacy project");

        assert_eq!(project.patterns[0].track_params[0].mute_group, 0);
        assert!(project.patterns[0].instrument_run_modes.is_empty());
    }

    #[test]
    fn project_serializes_sparse_step_and_plock_data() {
        let project = sample_project();
        let json = serde_json::to_string(&project).expect("serialize sparse project");

        assert!(json.contains("\"plocks_sparse\""));
        assert!(json.contains("\"step\":0"));
        assert!(!json.contains("\"plocks\":[[null"));
    }

    #[test]
    fn project_mod_connection_roundtrip_preserves_bus_destination() {
        let connection = ModConnection {
            source_track: 2,
            destination: ModDestination::Bus(BusId(77)),
            dest_input: 3,
        };

        let saved = ProjectModConnection::from(connection);
        assert_eq!(
            saved.destination,
            Some(ProjectModDestination::Bus(77)),
            "project route should store an explicit bus destination"
        );
        assert_eq!(ModConnection::from(saved), connection);
    }

    #[test]
    fn sparse_effect_slot_deserializes() {
        let json = r#"{
            "num_params": 2,
            "defaults": [0.1, 0.2],
            "plocks_sparse": [{"step": 3, "param": 1, "value": 0.8}],
            "key_locks_sparse": [{"note": 69, "param": 0, "value": 0.45, "param_id": {"logical_id": 42, "node_param_idx": 0}}],
            "param_node_indices": [0, 1]
        }"#;

        let slot: ProjectEffectSlot = serde_json::from_str(json).expect("deserialize sparse slot");
        assert_eq!(slot.num_params, 2);
        assert_eq!(slot.defaults, vec![0.1, 0.2]);
        assert_eq!(slot.plocks[3][1], Some(0.8));
        assert_eq!(slot.plocks[0][1], None);
        assert_eq!(slot.key_locks[&69][0], Some(0.45));
        assert_eq!(
            slot.key_lock_param_ids[&69][0],
            Some(ParamNodeId {
                logical_id: 42,
                node_param_idx: 0,
            })
        );
        // Slots saved before the IR field deserialize with ir = None.
        assert_eq!(slot.ir, None);
    }

    #[test]
    fn rack_track_serializes_and_deserializes_broadcast_slots() {
        let json = r#"
        {
            "version": 1,
            "name": "rack",
            "bpm": 120,
            "current_pattern": 0,
            "reverb": {"size":0.5,"brightness":0.5,"replace":0.0},
            "tracks": [{
                "kind": "rack",
                "routing": "broadcast",
                "slots": [
                    {"instrument_type":"custom","instrument_name":"emulations/prophet-5"},
                    {"instrument_type":"sampler","sample_path":"samples/kick.wav","sample_name":"kick"}
                ]
            }],
            "custom_effects": [[]],
            "patterns": [{
                "track_bits": [[0,0,0,0]],
                "step_data": [[[0,0,0,0,0,0,0,0,0,0]]],
                "track_params": [{
                    "gate": true,
                    "attack_ms": 0.0,
                    "release_ms": 0.0,
                    "swing": 50.0,
                    "num_steps": 16,
                    "volume": 1.0,
                    "send": 0.0,
                    "polyphonic": true,
                    "max_polyphony": 4,
                    "timebase": 2
                }],
                "effect_slots": [[]],
                "instrument_slots": [{"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]}],
                "instrument_base_note_offsets": [0.0],
                "track_sound_states": [{"loaded_preset":null,"dirty":false}],
                "chord_snapshots": [[]],
                "timebase_plock_snapshots": [[]],
                "instrument_types": ["custom"],
                "sample_paths": [null],
                "sample_names": [""],
                "rack_tracks": [{
                    "routing": "broadcast",
                    "slots": [{
                        "instrument_type": "custom",
                        "instrument_run_mode": "instrument",
                        "instrument_base_note_offset": 12.0,
                        "gain": 0.75,
                        "pan": -0.25,
                        "max_polyphony": 3,
                        "instrument_slot": {"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]},
                        "effect_slots": [{"num_params":1,"defaults":[0.75],"plocks":[],"param_node_indices":[4]}],
                        "custom_effects": ["builtin:OTT"],
                        "track_sound_state": {"loaded_preset":"wide","dirty":true}
                    }]
                }]
            }]
        }
        "#;

        let project: ProjectFile = serde_json::from_str(json).unwrap();
        match &project.tracks[0].kind {
            ProjectTrackKind::Rack { routing, slots } => {
                assert_eq!(*routing, ProjectRackRouting::Broadcast);
                assert_eq!(slots.len(), 2);
                assert_eq!(
                    slots[0].instrument_name.as_deref(),
                    Some("emulations/prophet-5")
                );
                assert_eq!(slots[1].sample_name.as_deref(), Some("kick"));
            }
            _ => panic!("expected rack track"),
        }
        let rack = project.patterns[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(rack.routing, ProjectRackRouting::Broadcast);
        assert_eq!(rack.slots[0].instrument_base_note_offset, 12.0);
        assert_eq!(rack.slots[0].gain, 0.75);
        assert_eq!(rack.slots[0].pan, -0.25);
        assert_eq!(rack.slots[0].max_polyphony, 3);
        assert_eq!(
            rack.slots[0].track_sound_state.loaded_preset.as_deref(),
            Some("wide")
        );
        assert!(rack.slots[0].track_sound_state.dirty);
        assert_eq!(rack.slots[0].effect_slots.len(), 1);
        assert_eq!(rack.slots[0].effect_slots[0].defaults, vec![0.75]);
        assert_eq!(
            rack.slots[0].custom_effects,
            vec![Some("builtin:OTT".to_string())]
        );
        let runtime_slot = RackSlotSnapshot::from(rack.slots[0].clone());
        assert_eq!(
            runtime_slot.effect_slots.len(),
            crate::lisp_host::MAX_CUSTOM_FX
        );
        assert_eq!(runtime_slot.effect_slots[0].defaults, vec![0.75]);
        assert_eq!(
            runtime_slot.custom_effect_names[0].as_deref(),
            Some("builtin:OTT")
        );
    }

    #[test]
    fn rack_track_serializes_and_deserializes_by_pitch_pad_metadata() {
        let json = r#"
        {
            "version": 1,
            "name": "drum-rack",
            "bpm": 120,
            "current_pattern": 0,
            "reverb": {"size":0.5,"brightness":0.5,"replace":0.0},
            "tracks": [{
                "kind": "rack",
                "routing": "by_pitch",
                "slots": [
                    {"instrument_type":"sampler","sample_path":"samples/kick.wav","sample_name":"kick"},
                    {"instrument_type":"custom","instrument_name":"emulations/digitone"}
                ]
            }],
            "custom_effects": [[]],
            "patterns": [{
                "track_bits": [[0,0,0,0]],
                "step_data": [[[0,0,0,0,0,0,0,0,0,0]]],
                "track_params": [{
                    "gate": true,
                    "attack_ms": 0.0,
                    "release_ms": 0.0,
                    "swing": 50.0,
                    "num_steps": 16,
                    "volume": 1.0,
                    "send": 0.0,
                    "polyphonic": true,
                    "max_polyphony": 4,
                    "timebase": 2
                }],
                "effect_slots": [[]],
                "instrument_slots": [{"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]}],
                "instrument_base_note_offsets": [0.0],
                "track_sound_states": [{"loaded_preset":null,"dirty":false}],
                "chord_snapshots": [[]],
                "timebase_plock_snapshots": [[]],
                "instrument_types": ["rack"],
                "sample_paths": [null],
                "sample_names": [""],
                "rack_tracks": [{
                    "routing": "by_pitch",
                    "slots": [{
                        "instrument_type": "sampler",
                        "instrument_run_mode": "instrument",
                        "instrument_base_note_offset": 0.0,
                        "pad_note": 0,
                        "choke_group": 1,
                        "gain": 0.9,
                        "pan": 0.1,
                        "max_polyphony": 1,
                        "instrument_slot": {"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]},
                        "sample_name": "kick"
                    }, {
                        "instrument_type": "custom",
                        "instrument_run_mode": "instrument",
                        "instrument_base_note_offset": 7.0,
                        "pad_note": 2,
                        "choke_group": 1,
                        "gain": 0.75,
                        "pan": -0.25,
                        "max_polyphony": 2,
                        "instrument_slot": {"num_params":0,"defaults":[],"plocks":[],"param_node_indices":[]},
                        "track_sound_state": {"loaded_preset":"digitone","dirty":true}
                    }]
                }]
            }]
        }
        "#;

        let project: ProjectFile = serde_json::from_str(json).unwrap();
        match &project.tracks[0].kind {
            ProjectTrackKind::Rack { routing, slots } => {
                assert_eq!(*routing, ProjectRackRouting::ByPitch);
                assert_eq!(slots.len(), 2);
                assert_eq!(slots[0].sample_name.as_deref(), Some("kick"));
                assert_eq!(
                    slots[1].instrument_name.as_deref(),
                    Some("emulations/digitone")
                );
            }
            _ => panic!("expected rack track"),
        }
        let rack = project.patterns[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(rack.routing, ProjectRackRouting::ByPitch);
        assert_eq!(rack.slots[0].pad_note, Some(0));
        assert_eq!(rack.slots[0].choke_group, Some(1));
        assert_eq!(rack.slots[0].max_polyphony, 1);
        assert_eq!(rack.slots[1].pad_note, Some(2));
        assert_eq!(rack.slots[1].choke_group, Some(1));
        assert_eq!(rack.slots[1].instrument_base_note_offset, 7.0);

        let serialized = serde_json::to_string(&project).unwrap();
        assert!(serialized.contains("\"routing\":\"by_pitch\""));
        assert!(serialized.contains("\"pad_note\":0"));
        assert!(serialized.contains("\"choke_group\":1"));

        let restored: ProjectFile = serde_json::from_str(&serialized).unwrap();
        let restored_rack = restored.patterns[0].rack_tracks[0].as_ref().unwrap();
        assert_eq!(restored_rack.routing, ProjectRackRouting::ByPitch);
        assert_eq!(restored_rack.slots[0].pad_note, Some(0));
        assert_eq!(restored_rack.slots[1].pad_note, Some(2));
    }

    #[test]
    fn rack_macro_bank_roundtrips_stable_ids_names_plocks_and_relative_targets() {
        let mut macros = default_project_rack_macros();
        macros[0].name = "Wonky".to_string();
        macros[0].value = 0.4;
        macros[0].plocks[3] = Some(0.8);
        macros[0].mappings.push(ProjectRackMacroMapping {
            target: ProjectRackMacroTarget::SlotEffectParam {
                slot: 1,
                effect_slot: 2,
                param: "feedback".to_string(),
                param_index: 4,
            },
            range_min: 0.1,
            range_max: 0.9,
            curve: ProjectRackMacroCurve::Exp,
        });
        let rack = ProjectRackTrackPattern {
            routing: ProjectRackRouting::Broadcast,
            slots: Vec::new(),
            macros,
        };
        let json = serde_json::to_string(&rack).unwrap();
        let restored: ProjectRackTrackPattern = serde_json::from_str(&json).unwrap();
        let runtime = RackTrackSnapshot::from(restored);
        assert_eq!(runtime.macros.len(), crate::sequencer::RACK_MACRO_COUNT);
        assert_eq!(runtime.macros[0].id.stable_key(), "macro_1");
        assert_eq!(runtime.macros[0].name, "Wonky");
        assert_eq!(runtime.macros[0].plocks[3], Some(0.8));
        assert!(matches!(
            runtime.macros[0].mappings[0].target,
            crate::sequencer::RackMacroTarget::SlotEffectParam {
                slot: 1,
                effect_slot: 2,
                param_index: 4,
                ..
            }
        ));
    }

    #[test]
    fn sound_preset_roundtrips_rack_sources_metadata_and_slot_fx() {
        let sound = ProjectSoundPreset {
            version: project_file_version(),
            metadata: ProjectSoundMetadata {
                name: "Wide Plate".to_string(),
                tags: vec!["pad".to_string(), "wide".to_string()],
                author: "Test".to_string(),
            },
            track: ProjectTrack {
                id: TrackId(1),
                color: None,
                collapsed: false,
                kind: ProjectTrackKind::Rack {
                    routing: ProjectRackRouting::Broadcast,
                    slots: vec![ProjectRackTrackSlot {
                        instrument_type: ProjectInstrumentType::Sampler,
                        sample_path: Some("samples/pad.wav".to_string()),
                        sample_name: Some("pad".to_string()),
                        instrument_name: None,
                    }],
                },
            },
            rack: ProjectRackTrackPattern {
                macros: default_project_rack_macros(),
                routing: ProjectRackRouting::Broadcast,
                slots: vec![ProjectRackSlotPattern {
                    instrument_type: ProjectInstrumentType::Sampler,
                    instrument_run_mode: ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: 0.0,
                    pad_note: None,
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: 4,
                    param_plocks: Vec::new(),
                    instrument_slot: ProjectEffectSlot::default(),
                    effect_slots: vec![ProjectEffectSlot {
                        num_params: 1,
                        defaults: vec![0.42],
                        ..ProjectEffectSlot::default()
                    }],
                    custom_effects: vec![Some("builtin:OTT".to_string())],
                    track_sound_state: ProjectTrackSoundState::default(),
                    sample_path: Some("samples/pad.wav".to_string()),
                    sample_name: Some("pad".to_string()),
                }],
            },
        };
        let json = serde_json::to_string(&sound).expect("serialize Sound preset");
        let restored: ProjectSoundPreset =
            serde_json::from_str(&json).expect("deserialize Sound preset");
        assert_eq!(restored.metadata.tags, vec!["pad", "wide"]);
        assert_eq!(restored.rack.slots[0].effect_slots[0].defaults, vec![0.42]);
        assert_eq!(
            restored.rack.slots[0].custom_effects[0].as_deref(),
            Some("builtin:OTT")
        );
    }

    #[test]
    fn container_preset_storage_roundtrips_validated_rack_payload() {
        let directory = std::env::temp_dir().join(format!(
            "eseq-rack-preset-storage-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let sound = ProjectSoundPreset {
            version: project_file_version(),
            metadata: ProjectSoundMetadata {
                name: "Wide Rack".to_string(),
                tags: Vec::new(),
                author: String::new(),
            },
            track: ProjectTrack {
                id: TrackId(1),
                color: None,
                collapsed: false,
                kind: ProjectTrackKind::Rack {
                    routing: ProjectRackRouting::Broadcast,
                    slots: Vec::new(),
                },
            },
            rack: ProjectRackTrackPattern {
                macros: default_project_rack_macros(),
                routing: ProjectRackRouting::Broadcast,
                slots: Vec::new(),
            },
        };

        let path =
            save_container_preset(&directory, "rackpreset", "rack preset", "Wide Rack", &sound)
                .expect("save rack preset payload");
        let restored = load_container_preset(&path, "rack preset")
            .expect("load validated rack preset payload");
        assert_eq!(restored.metadata.name, "Wide Rack");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Wide-Rack.rackpreset")
        );
        std::fs::remove_dir_all(&directory).expect("clean rack preset test directory");
    }

    #[test]
    fn effect_slot_persists_ir_reference() {
        let slot = ProjectEffectSlot {
            num_params: 2,
            defaults: vec![0.35, 1.0],
            plocks: vec![Vec::new(); MAX_STEPS],
            plock_param_ids: vec![Vec::new(); MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![10, 11],
            param_node_spans: vec![1, 1],
            tensor_params: Vec::new(),
            ir: Some("lexicon-300-rich-plate".to_string()),
        };
        let json = serde_json::to_string(&slot).expect("serialize slot with ir");
        assert!(json.contains("\"ir\":\"lexicon-300-rich-plate\""), "{json}");
        let back: ProjectEffectSlot = serde_json::from_str(&json).expect("roundtrip slot with ir");
        assert_eq!(back.ir.as_deref(), Some("lexicon-300-rich-plate"));
    }

    #[test]
    fn effect_slot_roundtrips_sparse_key_locks() {
        let mut key_locks = std::collections::BTreeMap::new();
        key_locks.insert(69, vec![Some(0.45), None]);
        let mut key_lock_param_ids = std::collections::BTreeMap::new();
        key_lock_param_ids.insert(
            69,
            vec![
                Some(ParamNodeId {
                    logical_id: 42,
                    node_param_idx: 0,
                }),
                None,
            ],
        );
        let slot = ProjectEffectSlot {
            num_params: 2,
            defaults: vec![0.1, 0.2],
            plocks: vec![Vec::new(); MAX_STEPS],
            plock_param_ids: vec![Vec::new(); MAX_STEPS],
            key_locks,
            key_lock_param_ids,
            param_node_indices: vec![10, 11],
            param_node_spans: vec![1, 1],
            tensor_params: Vec::new(),
            ir: None,
        };

        let json = serde_json::to_string(&slot).expect("serialize key-lock slot");
        assert!(json.contains("\"key_locks_sparse\""), "{json}");
        assert!(!json.contains("\"key_locks\":{\""), "{json}");

        let back: ProjectEffectSlot = serde_json::from_str(&json).expect("roundtrip key-lock slot");
        assert_eq!(back.key_locks[&69][0], Some(0.45));
        assert_eq!(
            back.key_lock_param_ids[&69][0],
            Some(ParamNodeId {
                logical_id: 42,
                node_param_idx: 0,
            })
        );
    }

    #[test]
    fn effect_slot_roundtrips_tensor_defaults_and_sparse_whole_matrix_plocks() {
        let mut tensor_plocks = vec![None; MAX_STEPS];
        tensor_plocks[11] = Some(vec![0.0, 0.25, 0.75, 1.0]);
        let slot = ProjectEffectSlot {
            num_params: 0,
            defaults: Vec::new(),
            plocks: vec![Vec::new(); MAX_STEPS],
            plock_param_ids: vec![Vec::new(); MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: Vec::new(),
            param_node_spans: Vec::new(),
            tensor_params: vec![TensorParamSnapshot {
                name: "strike_mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 64,
                default: vec![0.1, 0.2, 0.3, 0.4],
                plocks: tensor_plocks,
            }],
            ir: None,
        };

        let json = serde_json::to_string(&slot).expect("serialize tensor slot");

        assert!(json.contains("\"tensor_params\""), "{json}");
        assert!(json.contains("\"plocks_sparse\""), "{json}");
        let back: ProjectEffectSlot =
            serde_json::from_str(&json).expect("roundtrip slot with tensor params");
        assert_eq!(back.tensor_params.len(), 1);
        assert_eq!(back.tensor_params[0].name, "strike_mask");
        assert_eq!(back.tensor_params[0].shape, vec![2, 2]);
        assert_eq!(back.tensor_params[0].cell_offset, 64);
        assert_eq!(back.tensor_params[0].default, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(
            back.tensor_params[0].plocks[11],
            Some(vec![0.0, 0.25, 0.75, 1.0])
        );
        assert!(back.tensor_params[0].plocks[10].is_none());
    }
}
