use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::effects::EffectSlotSnapshot;
use crate::sequencer::{
    BusId, ChordSnapshot, InstrumentType, MidiFxPosition, PatternSnapshot, SwingResolution,
    Timebase, TrackOutput, TrackParamsSnapshot, TrackSendSnapshot, TrackSoundState, MAX_STEPS,
    NUM_PARAMS, TRACK_PATTERN_WORDS,
};

const PROJECTS_DIR: &str = "projects";
const PROJECT_FILE_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub bpm: u32,
    #[serde(default = "default_master_volume")]
    pub master_volume: f32,
    pub current_pattern: usize,
    pub reverb: ProjectReverbState,
    #[serde(default = "default_project_buses")]
    pub buses: Vec<ProjectBusChannel>,
    pub tracks: Vec<ProjectTrack>,
    pub custom_effects: Vec<Vec<Option<String>>>,
    #[serde(default)]
    pub scratch: ProjectScratchState,
    pub patterns: Vec<ProjectPattern>,
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
            volume: 1.0,
            mute: false,
            solo: false,
            gate_sequence: ProjectBusGateSequence::default(),
            custom_effects: Vec::new(),
            effect_slots: Vec::new(),
        },
        ProjectBusChannel {
            id: crate::sequencer::DEFAULT_BUS_A_ID,
            name: "Bus A".to_string(),
            volume: 1.0,
            mute: false,
            solo: false,
            gate_sequence: ProjectBusGateSequence::default(),
            custom_effects: Vec::new(),
            effect_slots: Vec::new(),
        },
        ProjectBusChannel {
            id: crate::sequencer::DEFAULT_BUS_B_ID,
            name: "Bus B".to_string(),
            volume: 1.0,
            mute: false,
            solo: false,
            gate_sequence: ProjectBusGateSequence::default(),
            custom_effects: Vec::new(),
            effect_slots: Vec::new(),
        },
    ]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectTrack {
    Sampler { sample_path: String },
    Custom { instrument_name: String },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectPattern {
    pub track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
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
    pub instrument_types: Vec<ProjectInstrumentType>,
    pub sample_paths: Vec<Option<String>>,
    pub sample_names: Vec<String>,
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

#[derive(Clone)]
pub struct ProjectEffectSlot {
    pub num_params: u32,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub param_node_indices: Vec<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectTrackSoundState {
    pub loaded_preset: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectInstrumentType {
    Sampler,
    Custom,
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
            instrument_types: snapshot
                .instrument_types
                .iter()
                .copied()
                .map(ProjectInstrumentType::from)
                .collect(),
            sample_paths,
            sample_names,
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
            timebase: value.timebase as u8,
            accumulator_idx: value.accumulator_idx,
            script_accumulator_name: value.script_accumulator_name,
            midi_fx_chain: value.midi_fx_chain,
            midi_fx_position: ProjectMidiFxPosition::from(value.midi_fx_position),
            accum_limit: value.accum_limit,
            accum_mode: value.accum_mode,
            fts_scale: value.fts_scale,
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
            timebase: Timebase::from_index(value.timebase as u32),
            accumulator_idx: value.accumulator_idx,
            script_accumulator_name: value.script_accumulator_name,
            midi_fx_chain: value.midi_fx_chain,
            midi_fx_position: MidiFxPosition::from(value.midi_fx_position),
            accum_limit: value.accum_limit,
            accum_mode: value.accum_mode,
            fts_scale: value.fts_scale,
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
            param_node_indices: value.param_node_indices.clone(),
        }
    }
}

impl ProjectEffectSlot {
    pub fn into_snapshot_with_node_id(self, node_id: u32) -> EffectSlotSnapshot {
        EffectSlotSnapshot {
            node_id,
            num_params: self.num_params,
            defaults: self.defaults,
            plocks: self.plocks,
            param_node_indices: self.param_node_indices,
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
        }
    }
}

impl From<ProjectInstrumentType> for InstrumentType {
    fn from(value: ProjectInstrumentType) -> Self {
        match value {
            ProjectInstrumentType::Sampler => InstrumentType::Sampler,
            ProjectInstrumentType::Custom => InstrumentType::Custom,
        }
    }
}

pub fn ensure_projects_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(projects_dir())
}

pub fn list_project_names() -> std::io::Result<Vec<String>> {
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
            items.push(stem.to_string());
        }
    }
    items.sort();
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

fn default_track_volume() -> f32 {
    1.0
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
}

#[derive(Serialize, Deserialize)]
struct SparseProjectEffectSlot {
    num_params: u32,
    defaults: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plocks_sparse: Vec<SparseEffectSlotPlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    param_node_indices: Vec<u32>,
}

#[derive(Deserialize)]
struct DenseProjectEffectSlot {
    num_params: u32,
    defaults: Vec<f32>,
    plocks: Vec<Vec<Option<f32>>>,
    param_node_indices: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProjectEffectSlotRepr {
    Sparse(SparseProjectEffectSlot),
    Dense(DenseProjectEffectSlot),
    Empty(()),
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
                    value.map(|value| SparseEffectSlotPlock { step, param, value })
                })
            })
            .collect::<Vec<_>>();

        if self.num_params == 0
            && self.defaults.is_empty()
            && self.param_node_indices.is_empty()
            && plocks_sparse.is_empty()
        {
            return Option::<()>::None.serialize(serializer);
        }

        SparseProjectEffectSlot {
            num_params: self.num_params,
            defaults: self.defaults.clone(),
            plocks_sparse,
            param_node_indices: self.param_node_indices.clone(),
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
                for entry in slot.plocks_sparse {
                    if entry.step < plocks.len() && entry.param < slot.defaults.len() {
                        plocks[entry.step][entry.param] = Some(entry.value);
                    }
                }
                Self {
                    num_params: slot.num_params,
                    defaults: slot.defaults,
                    plocks,
                    param_node_indices: slot.param_node_indices,
                }
            }
            ProjectEffectSlotRepr::Dense(slot) => Self {
                num_params: slot.num_params,
                defaults: slot.defaults,
                plocks: slot.plocks,
                param_node_indices: slot.param_node_indices,
            },
            ProjectEffectSlotRepr::Empty(_) => Self {
                num_params: 0,
                defaults: Vec::new(),
                plocks: vec![Vec::new(); MAX_STEPS],
                param_node_indices: Vec::new(),
            },
        })
    }
}

pub fn chord_snapshot_from_steps(steps: Vec<Vec<f32>>) -> ChordSnapshot {
    let durations = steps.iter().map(|notes| vec![0.0; notes.len()]).collect();
    ChordSnapshot { steps, durations }
}

pub fn chord_snapshot_from_steps_and_durations(
    steps: Vec<Vec<f32>>,
    mut durations: Vec<Vec<f32>>,
) -> ChordSnapshot {
    durations.resize_with(steps.len(), Vec::new);
    for (idx, notes) in steps.iter().enumerate() {
        durations[idx].resize(notes.len(), 0.0);
    }
    ChordSnapshot { steps, durations }
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
            current_pattern: 1,
            reverb: ProjectReverbState {
                size: 0.2,
                brightness: 0.8,
                replace: 0.3,
            },
            buses: default_project_buses(),
            tracks: vec![
                ProjectTrack::Custom {
                    instrument_name: "prophet-5".to_string(),
                },
                ProjectTrack::Sampler {
                    sample_path: "samples/drums/kick.wav".to_string(),
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
                step_data: vec![vec![[1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]; 256]; 2],
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
                        timebase: Timebase::Sixteenth as u8,
                        accumulator_idx: 1,
                        script_accumulator_name: None,
                        midi_fx_chain: Vec::new(),
                        midi_fx_position: ProjectMidiFxPosition::PostAccumulator,
                        accum_limit: 24.0,
                        accum_mode: 2,
                        fts_scale: 0,
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
                        timebase: Timebase::Eighth as u8,
                        accumulator_idx: 0,
                        script_accumulator_name: None,
                        midi_fx_chain: Vec::new(),
                        midi_fx_position: ProjectMidiFxPosition::PostAccumulator,
                        accum_limit: 48.0,
                        accum_mode: 0,
                        fts_scale: 0,
                    },
                ],
                effect_slots: vec![vec![], vec![]],
                midi_fx_slots: vec![vec![], vec![]],
                instrument_slots: vec![
                    ProjectEffectSlot {
                        num_params: 2,
                        defaults: vec![0.1, 0.2],
                        plocks: vec![vec![None, Some(0.8)]; 256],
                        param_node_indices: vec![0, 1],
                    },
                    ProjectEffectSlot {
                        num_params: 0,
                        defaults: vec![],
                        plocks: vec![vec![]; 256],
                        param_node_indices: vec![],
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
                chord_snapshots: vec![vec![Vec::new(); 256], vec![Vec::new(); 256]],
                chord_duration_snapshots: vec![vec![Vec::new(); 256], vec![Vec::new(); 256]],
                timebase_plock_snapshots: vec![vec![None; 256], vec![None; 256]],
                swing_plock_snapshots: vec![vec![None; 256], vec![None; 256]],
                swing_resolution_plock_snapshots: vec![vec![None; 256], vec![None; 256]],
                bus_patterns: Vec::new(),
                instrument_types: vec![
                    ProjectInstrumentType::Custom,
                    ProjectInstrumentType::Sampler,
                ],
                sample_paths: vec![None, Some("samples/drums/kick.wav".to_string())],
                sample_names: vec!["prophet-5".to_string(), "kick".to_string()],
            }],
        }
    }

    #[test]
    fn current_project_format_roundtrips() {
        let project = sample_project();
        let json = serde_json::to_string(&project).expect("serialize current project");
        let restored: ProjectFile =
            serde_json::from_str(&json).expect("deserialize current project");

        assert_eq!(restored.name, project.name);
        assert_eq!(restored.scratch.buffer, project.scratch.buffer);
        assert_eq!(restored.scratch.cursor_row, project.scratch.cursor_row);
        assert_eq!(restored.scratch.cursor_col, project.scratch.cursor_col);
        assert_eq!(restored.patterns.len(), 1);
        assert_eq!(restored.patterns[0].track_bits[0], [0b1011, 0, 0, 0]);
        assert_eq!(restored.patterns[0].track_bits[1], [0b0101, 1, 0, 0]);
        assert_eq!(restored.patterns[0].step_data[0].len(), 256);
        assert_eq!(restored.patterns[0].timebase_plock_snapshots[0].len(), 256);
        assert_eq!(restored.patterns[0].track_params[0].accumulator_idx, 1);
        assert_eq!(restored.patterns[0].track_params[0].accum_limit, 24.0);
        assert_eq!(restored.patterns[0].track_params[0].accum_mode, 2);
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
        assert_eq!(step[crate::sequencer::StepParam::Transpose.index()], 7.0);
        assert_eq!(step[crate::sequencer::StepParam::Pan.index()], 0.0);
        assert_eq!(step[crate::sequencer::StepParam::Chop.index()], 1.0);
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
    fn sparse_effect_slot_deserializes() {
        let json = r#"{
            "num_params": 2,
            "defaults": [0.1, 0.2],
            "plocks_sparse": [{"step": 3, "param": 1, "value": 0.8}],
            "param_node_indices": [0, 1]
        }"#;

        let slot: ProjectEffectSlot = serde_json::from_str(json).expect("deserialize sparse slot");
        assert_eq!(slot.num_params, 2);
        assert_eq!(slot.defaults, vec![0.1, 0.2]);
        assert_eq!(slot.plocks[3][1], Some(0.8));
        assert_eq!(slot.plocks[0][1], None);
    }
}
