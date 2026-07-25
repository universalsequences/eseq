use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::plock_variants::PlockVariantRegistry;
use crate::sequencer::{
    BusId, EffectInstanceId, InstrumentDeviceValuesSnapshot, MidiFxInstanceId, PatternId,
    RackSlotId, RackSlotValuesSnapshot, SceneId, StepCellSnapshot, StepSlotPlocks, TrackId,
    TrackParamsSnapshot, TrackPatternId,
};
use crate::effects::EffectSlotValuesSnapshot;
use crate::macro_engine::TrackInstrumentMacroMappings;
use crate::macro_engine::TrackEffectMacroMappings;
use crate::macro_engine::TrackMidiFxMacroMappings;
use crate::sequencer::TrackInstrumentPatternStateSnapshot;
use crate::sequencer::RackSlotPatternStateSnapshot;

pub const DEFAULT_HISTORY_ENTRY_LIMIT: usize = 256;
pub const DEFAULT_HISTORY_BYTE_LIMIT: usize = 64 * 1024 * 1024;
pub const FALLBACK_GESTURE_IDLE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    UserEdit,
    Undo,
    Redo,
    ProjectLoad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryPolicy {
    Record,
    Coalesce(MergeKey),
    Ignore,
    Barrier,
}

#[derive(Clone, Debug)]
pub enum EditPatch {
    Composite(Vec<EditPatch>),
    StepCells(StepCellsPatch),
    PatternGeometry(PatternGeometryPatch),
    TrackParams(TrackParamsPatch),
    TrackParamsBatch(TrackParamsBatchPatch),
    BusMixer(BusMixerPatch),
    DeviceValues(DeviceValuesPatch),
    InstrumentBinding(InstrumentBindingPatch),
    EffectChain(EffectChainPatch),
    BusEffectChain(BusEffectChainPatch),
    BusEffectValues(BusEffectValuesPatch),
    RackEffectChain(RackEffectChainPatch),
    MidiFxChain(MidiFxChainPatch),
    RackSlotStructure(RackSlotStructurePatch),
    TrackCreation(TrackCreationPatch),
    TrackDeletion(TrackDeletionPatch),
    TrackPresentation(TrackPresentationPatch),
    SceneStructure(SceneStructurePatch),
    Song(SongStructurePatch),
    Arrangement(ArrangementStructurePatch),
    BusGroupStructure(BusGroupStructurePatch),
    MacroConfiguration(MacroConfigurationPatch),
    TransportParams(TransportParamsPatch),
}

#[derive(Clone, Debug)]
pub struct MacroConfigurationPatch {
    pub before: crate::macro_engine::MacroConfigurationState,
    pub after: crate::macro_engine::MacroConfigurationState,
}

impl MacroConfigurationPatch {
    pub fn retained_bytes(&self) -> usize {
        fn state_bytes(state: &crate::macro_engine::MacroConfigurationState) -> usize {
            state.macros.iter().map(|definition| {
                definition.name.capacity()
                    + definition.key.as_ref().map(String::capacity).unwrap_or(0)
                    + definition.mappings.capacity()
                        * std::mem::size_of::<crate::macro_engine::MacroMapping>()
            }).sum()
        }
        std::mem::size_of::<Self>() + state_bytes(&self.before) + state_bytes(&self.after)
    }
}

#[derive(Clone, Debug)]
pub struct BusStructureState {
    pub id: BusId,
    pub name: String,
    pub volume: f32,
    pub mute: bool,
    pub solo: bool,
    pub gate_sequence: crate::sequencer::BusGateSequence,
    pub effects: BusEffectChainState,
}

#[derive(Clone, Debug)]
pub struct BusGroupStructureState {
    pub buses: Vec<BusStructureState>,
    pub groups: Vec<GroupStructureState>,
    pub scenes: crate::sequencer::ProjectScenes,
}

#[derive(Clone, Debug)]
pub struct GroupStructureState {
    pub id: u64,
    pub name: String,
    pub color: [f32; 3],
    pub collapsed: bool,
    pub members: Vec<TrackId>,
    pub bus_id: BusId,
}

#[derive(Clone, Debug)]
pub struct BusGroupStructurePatch {
    pub before: BusGroupStructureState,
    pub after: BusGroupStructureState,
}

impl BusGroupStructurePatch {
    pub fn retained_bytes(&self) -> usize {
        fn state_bytes(state: &BusGroupStructureState) -> usize {
            state.buses.iter().map(|bus| {
                bus.name.capacity() + BusEffectChainPatch::state_bytes(&bus.effects)
            }).sum::<usize>()
                + state.groups.capacity()
                    * std::mem::size_of::<GroupStructureState>()
                + SceneStructurePatch::state_bytes(&state.scenes)
        }
        std::mem::size_of::<Self>() + state_bytes(&self.before) + state_bytes(&self.after)
    }
}

#[derive(Clone, Debug)]
pub struct TrackPresentationState {
    pub color: crate::track_color::TrackColor,
    pub collapsed: bool,
}

#[derive(Clone, Debug)]
pub struct TrackPresentationChange {
    pub track: TrackId,
    pub before: TrackPresentationState,
    pub after: TrackPresentationState,
}

#[derive(Clone, Debug)]
pub struct TrackPresentationPatch {
    pub changes: Vec<TrackPresentationChange>,
}

impl TrackPresentationPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.changes.capacity() * std::mem::size_of::<TrackPresentationChange>()
    }
}

#[derive(Clone, Debug)]
pub struct SceneStructurePatch {
    pub before: crate::sequencer::ProjectScenes,
    pub after: crate::sequencer::ProjectScenes,
}

impl SceneStructurePatch {
    fn state_bytes(state: &crate::sequencer::ProjectScenes) -> usize {
            state.track_pools.iter().map(|pool| {
                pool.patterns.capacity()
                    * std::mem::size_of::<(
                        crate::sequencer::PatternId,
                        crate::sequencer::TrackPatternData,
                    )>()
            }).sum::<usize>()
                + state.scenes.capacity() * std::mem::size_of::<crate::sequencer::Scene>()
                + state.track_overrides.capacity()
                    * std::mem::size_of::<Option<crate::sequencer::PatternId>>()
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + Self::state_bytes(&self.before)
            + Self::state_bytes(&self.after)
    }
}

/// Whole-object memento of the committed song (docs/song-mode-spec.md 5.6),
/// modeled on `SceneStructurePatch`. `None` means "no song committed". The
/// patch restores the exact prior song, including `next_row_id`, satisfying
/// undo invariants H5 (rows carry stable `SongRowId`s) and H10 (each song
/// primitive commits exactly one entry).
#[derive(Clone, Debug)]
pub struct SongStructurePatch {
    pub before: Option<crate::sequencer::ProjectSong>,
    pub after: Option<crate::sequencer::ProjectSong>,
}

impl SongStructurePatch {
    fn state_bytes(state: &Option<crate::sequencer::ProjectSong>) -> usize {
        state
            .as_ref()
            .map(|song| {
                song.rows.capacity() * std::mem::size_of::<crate::sequencer::ProjectSongRow>()
                    + song
                        .rows
                        .iter()
                        .map(|row| {
                            row.overrides.capacity()
                                * std::mem::size_of::<crate::sequencer::ProjectSongTrackOverride>()
                        })
                        .sum::<usize>()
            })
            .unwrap_or(0)
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + Self::state_bytes(&self.before)
            + Self::state_bytes(&self.after)
    }
}

/// Whole-object memento of the committed arrangement
/// (docs/arrangement-lane-model-spec.md 8), modeled on `SongStructurePatch`.
/// `None` means "no arrangement committed". The patch restores the exact
/// prior arrangement, including `next_clip_id`, so clip identity survives
/// undo (invariant H5) and each arrangement primitive commits exactly one
/// entry (H10). The compiled song is *not* stored: restoring the arrangement
/// recompiles it (spec 7), so the two can never be restored out of step.
#[derive(Clone, Debug)]
pub struct ArrangementStructurePatch {
    pub before: Option<crate::sequencer::ProjectArrangement>,
    pub after: Option<crate::sequencer::ProjectArrangement>,
}

impl ArrangementStructurePatch {
    fn state_bytes(state: &Option<crate::sequencer::ProjectArrangement>) -> usize {
        state
            .as_ref()
            .map(|arrangement| {
                arrangement.scene_lane.capacity()
                    * std::mem::size_of::<crate::sequencer::SceneEvent>()
                    + arrangement.track_lanes.capacity()
                        * std::mem::size_of::<Vec<crate::sequencer::ArrClip>>()
                    + arrangement
                        .track_lanes
                        .iter()
                        .map(|lane| {
                            lane.capacity() * std::mem::size_of::<crate::sequencer::ArrClip>()
                        })
                        .sum::<usize>()
            })
            .unwrap_or(0)
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + Self::state_bytes(&self.before)
            + Self::state_bytes(&self.after)
    }
}

#[derive(Clone, Debug)]
pub struct MidiFxInstanceState {
    pub id: MidiFxInstanceId,
    pub name: String,
    pub descriptor: crate::effects::EffectDescriptor,
}

#[derive(Clone, Debug)]
pub struct MidiFxChainState {
    pub instances: Vec<MidiFxInstanceState>,
    pub pattern_slots: Vec<EffectPatternSlots>,
    pub macro_mappings: TrackMidiFxMacroMappings,
    pub process_chains: Vec<(PatternId, crate::process::TrackProcessChain)>,
}

#[derive(Clone, Debug)]
pub struct MidiFxChainPatch {
    pub track: TrackId,
    pub before: MidiFxChainState,
    pub after: MidiFxChainState,
}

impl MidiFxChainPatch {
    pub fn retained_bytes(&self) -> usize {
        fn state_bytes(state: &MidiFxChainState) -> usize {
            state.instances.capacity() * std::mem::size_of::<MidiFxInstanceState>()
                + state.instances.iter().map(|instance| instance.name.capacity()).sum::<usize>()
                + state.pattern_slots.iter().map(|pattern| {
                    pattern.values.capacity() * std::mem::size_of::<EffectSlotValuesSnapshot>()
                        + pattern.values.iter().map(EffectSlotValuesSnapshot::retained_bytes).sum::<usize>()
                }).sum::<usize>()
                + state.process_chains.capacity()
                    * std::mem::size_of::<(PatternId, crate::process::TrackProcessChain)>()
        }
        std::mem::size_of::<Self>() + state_bytes(&self.before) + state_bytes(&self.after)
    }
}

#[derive(Clone, Debug)]
pub struct EffectInstanceState {
    pub id: EffectInstanceId,
    pub source: super::fx_chain::RetainedEffectSource,
    pub descriptor: crate::effects::EffectDescriptor,
}

#[derive(Clone, Debug)]
pub struct EffectPatternSlots {
    pub pattern: PatternId,
    pub values: Vec<EffectSlotValuesSnapshot>,
}

#[derive(Clone, Debug)]
pub struct EffectChainState {
    pub instances: Vec<EffectInstanceState>,
    pub pattern_slots: Vec<EffectPatternSlots>,
    pub macro_mappings: TrackEffectMacroMappings,
    pub bindings: crate::sequencer::TrackEffectBindingStateSnapshot,
}

#[derive(Clone, Debug)]
pub struct EffectChainPatch {
    pub track: TrackId,
    pub before: EffectChainState,
    pub after: EffectChainState,
}

#[derive(Clone, Debug)]
pub struct BusEffectChainState {
    pub instances: Vec<EffectInstanceState>,
    pub live: crate::sequencer::BusPatternSnapshot,
    pub live_values: Vec<EffectSlotValuesSnapshot>,
    pub scenes: Vec<crate::sequencer::BusPatternSnapshot>,
    pub macro_mappings: crate::macro_engine::TrackEffectMacroMappings,
}

#[derive(Clone, Debug)]
pub struct BusEffectChainPatch {
    pub bus: BusId,
    pub before: BusEffectChainState,
    pub after: BusEffectChainState,
}

impl BusEffectChainPatch {
    fn state_bytes(state: &BusEffectChainState) -> usize {
            let source_bytes = state.instances.iter().map(|instance| match &instance.source {
                super::fx_chain::RetainedEffectSource::NativeBuiltin { name } => name.capacity(),
                super::fx_chain::RetainedEffectSource::Compiled { name, source, asset_base, .. } => {
                    name.capacity()
                        + source.capacity()
                        + asset_base.as_ref().map(|path| path.as_os_str().len()).unwrap_or(0)
                }
            }).sum::<usize>();
            source_bytes
                + state.instances.capacity() * std::mem::size_of::<EffectInstanceState>()
                + state.live_values.iter().map(EffectSlotValuesSnapshot::retained_bytes).sum::<usize>()
                + state.scenes.capacity()
                    * std::mem::size_of::<crate::sequencer::BusPatternSnapshot>()
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + Self::state_bytes(&self.before)
            + Self::state_bytes(&self.after)
    }
}

#[derive(Clone, Debug)]
pub struct BusEffectValuesPatch {
    pub bus: BusId,
    pub instance: EffectInstanceId,
    pub scene: SceneId,
    pub before: EffectSlotValuesSnapshot,
    pub after: EffectSlotValuesSnapshot,
}

impl BusEffectValuesPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.before.retained_bytes()
            + self.after.retained_bytes()
    }
}

#[derive(Clone, Debug)]
pub struct RackEffectChainState {
    pub instances: Vec<EffectInstanceState>,
    pub patterns: crate::sequencer::RackSlotPatternStateSnapshot,
    pub macros: crate::sequencer::RackMacroPatternStateSnapshot,
}

#[derive(Clone, Debug)]
pub struct RackEffectChainPatch {
    pub track: TrackId,
    pub rack_slot: RackSlotId,
    pub before: RackEffectChainState,
    pub after: RackEffectChainState,
}

impl RackEffectChainPatch {
    fn state_bytes(state: &RackEffectChainState) -> usize {
        let source_bytes = state.instances.iter().map(|instance| match &instance.source {
            super::fx_chain::RetainedEffectSource::NativeBuiltin { name } => name.capacity(),
            super::fx_chain::RetainedEffectSource::Compiled { name, source, asset_base, .. } => {
                name.capacity()
                    + source.capacity()
                    + asset_base.as_ref().map(|path| path.as_os_str().len()).unwrap_or(0)
            }
        }).sum::<usize>();
        source_bytes
            + state.instances.capacity() * std::mem::size_of::<EffectInstanceState>()
            + state.patterns.patterns.capacity()
                * std::mem::size_of::<(PatternId, crate::sequencer::RackSlotSnapshot)>()
            + state.macros.patterns.capacity()
                * std::mem::size_of::<(PatternId, Vec<crate::sequencer::RackMacro>)>()
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + Self::state_bytes(&self.before)
            + Self::state_bytes(&self.after)
    }
}

impl EffectChainPatch {
    pub fn retained_bytes(&self) -> usize {
        fn state_bytes(state: &EffectChainState) -> usize {
            let source_bytes = state.instances.iter().map(|instance| match &instance.source {
                super::fx_chain::RetainedEffectSource::NativeBuiltin { name } => name.capacity(),
                super::fx_chain::RetainedEffectSource::Compiled { name, source, asset_base, .. } => {
                    name.capacity()
                        + source.capacity()
                        + asset_base.as_ref().map(|path| path.as_os_str().len()).unwrap_or(0)
                }
            }).sum::<usize>();
            source_bytes
                + state.instances.capacity() * std::mem::size_of::<EffectInstanceState>()
                + state.pattern_slots.iter().map(|pattern| {
                    pattern.values.capacity() * std::mem::size_of::<EffectSlotValuesSnapshot>()
                        + pattern.values.iter().map(EffectSlotValuesSnapshot::retained_bytes).sum::<usize>()
                }).sum::<usize>()
                + state.bindings.process_chains.capacity()
                    * std::mem::size_of::<(PatternId, crate::process::TrackProcessChain)>()
                + state.bindings.project_process_lane_overrides.capacity()
                    * std::mem::size_of::<(PatternId, crate::process::ProjectLaneOverrides)>()
        }
        std::mem::size_of::<Self>() + state_bytes(&self.before) + state_bytes(&self.after)
    }
}

#[derive(Clone, Debug)]
pub enum RackSlotStructureEdit {
    Add {
        after: RackSlotPatternStateSnapshot,
    },
    ReplaceSource {
        before: RackSlotPatternStateSnapshot,
        after: RackSlotPatternStateSnapshot,
    },
}

#[derive(Clone, Debug)]
pub struct RackSlotStructurePatch {
    pub track: TrackId,
    pub slot: RackSlotId,
    pub slot_index: usize,
    pub edit: RackSlotStructureEdit,
}

impl RackSlotStructurePatch {
    pub fn retained_bytes(&self) -> usize {
        let snapshots = match &self.edit {
            RackSlotStructureEdit::Add { after } => after.patterns.len() + 1,
            RackSlotStructureEdit::ReplaceSource { before, after } => {
                before.patterns.len() + after.patterns.len() + 2
            }
        };
        std::mem::size_of::<Self>()
            + snapshots * std::mem::size_of::<crate::sequencer::RackSlotSnapshot>()
    }
}

#[derive(Clone, Debug)]
pub enum TrackInstrumentSource {
    Custom { engine_id: usize },
    Sampler {
        buffer_id: i32,
        sample_rate: u32,
        path: Option<PathBuf>,
    },
    Rack {
        slots: Vec<RackContainerSlotState>,
    },
    Modulator,
}

#[derive(Clone, Debug)]
pub struct RackContainerSlotState {
    pub id: RackSlotId,
    pub effects: RackEffectChainState,
}

#[derive(Clone, Debug)]
pub struct TrackInstrumentState {
    pub source: TrackInstrumentSource,
    pub display_name: String,
    pub patterns: TrackInstrumentPatternStateSnapshot,
    pub macro_mappings: TrackInstrumentMacroMappings,
}

#[derive(Clone, Debug)]
pub struct InstrumentBindingPatch {
    pub track: TrackId,
    pub before: TrackInstrumentState,
    pub after: TrackInstrumentState,
}

#[derive(Clone, Debug)]
pub struct TrackCreationPatch {
    pub track: TrackId,
    pub state: TrackInstrumentState,
    pub color: Option<crate::track_color::TrackColor>,
    pub collapsed: bool,
    pub group: Option<(u64, u64)>,
}

impl TrackCreationPatch {
    pub fn retained_bytes(&self) -> usize {
        let source_bytes = match &self.state.source {
            TrackInstrumentSource::Custom { .. } | TrackInstrumentSource::Modulator => 0,
            TrackInstrumentSource::Sampler { path, .. } => path
                .as_ref()
                .map(|path| path.as_os_str().len())
                .unwrap_or(0),
            TrackInstrumentSource::Rack { slots } => slots.iter()
                .map(|slot| RackEffectChainPatch::state_bytes(&slot.effects))
                .sum(),
        };
        std::mem::size_of::<Self>()
            + source_bytes
            + self.state.display_name.capacity()
            + self.state.patterns.patterns.capacity()
                * std::mem::size_of::<(
                    PatternId,
                    crate::sequencer::TrackInstrumentPatternState,
                )>()
    }
}

#[derive(Clone, Debug)]
pub struct TrackDeletionPatch {
    pub track: TrackId,
    pub index: usize,
    pub instrument: TrackInstrumentState,
    pub effects: EffectChainState,
    pub midi_fx: MidiFxChainState,
    pub patterns: crate::sequencer::TrackPatternLaneState,
    pub color: Option<crate::track_color::TrackColor>,
    pub collapsed: bool,
    pub rack_selected_slot: usize,
    pub rack_pad_bank_start: i32,
    pub record_armed: bool,
    pub groups: Vec<crate::project::ProjectTrackGroup>,
    pub macro_mappings: crate::macro_engine::TrackTopologyMacroMappings,
}

impl TrackDeletionPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.instrument.display_name.capacity()
            + self.patterns.pool.patterns.capacity()
                * std::mem::size_of::<(PatternId, crate::sequencer::TrackPatternData)>()
            + self.effects.instances.capacity() * std::mem::size_of::<EffectInstanceState>()
            + self.midi_fx.instances.capacity() * std::mem::size_of::<MidiFxInstanceState>()
    }
}

impl InstrumentBindingPatch {
    pub fn retained_bytes(&self) -> usize {
        fn state_bytes(state: &TrackInstrumentState) -> usize {
            let source_bytes = match &state.source {
                TrackInstrumentSource::Custom { .. } | TrackInstrumentSource::Modulator => 0,
                TrackInstrumentSource::Sampler { path, .. } => path
                    .as_ref()
                    .map(|path| path.as_os_str().len())
                    .unwrap_or(0),
                TrackInstrumentSource::Rack { slots } => slots.iter()
                    .map(|slot| RackEffectChainPatch::state_bytes(&slot.effects))
                    .sum(),
            };
            source_bytes
                + state.display_name.capacity()
                + state.patterns.patterns.capacity()
                    * std::mem::size_of::<(
                        PatternId,
                        crate::sequencer::TrackInstrumentPatternState,
                    )>()
                + state.patterns.neural_overrides.capacity()
                    * std::mem::size_of::<crate::sequencer::NeuralInstrumentOverrideState>()
                + state.macro_mappings.mappings.capacity()
                    * std::mem::size_of::<(
                        crate::macro_engine::MacroId,
                        Vec<(usize, crate::macro_engine::MacroMapping)>,
                    )>()
        }
        std::mem::size_of::<Self>() + state_bytes(&self.before) + state_bytes(&self.after)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceId {
    TrackInstrument(TrackId),
    AudioEffect(EffectInstanceId),
    MidiEffect(MidiFxInstanceId),
    RackSlot(RackSlotId),
    RackInstrument(RackSlotId),
}

#[derive(Clone, Debug)]
pub enum DeviceValueSnapshot {
    Instrument(InstrumentDeviceValuesSnapshot),
    Slot(EffectSlotValuesSnapshot),
    RackSlot(RackSlotValuesSnapshot),
}

impl DeviceValueSnapshot {
    pub fn bit_exact_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Instrument(left), Self::Instrument(right)) => left.bit_exact_eq(right),
            (Self::Slot(left), Self::Slot(right)) => left.bit_exact_eq(right),
            (Self::RackSlot(left), Self::RackSlot(right)) => left.bit_exact_eq(right),
            _ => false,
        }
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Instrument(snapshot) => snapshot.retained_bytes(),
                Self::Slot(snapshot) => snapshot.retained_bytes(),
                Self::RackSlot(snapshot) => snapshot.retained_bytes(),
            }
    }
}

#[derive(Clone, Debug)]
pub struct DeviceValuesPatch {
    pub target: DeviceId,
    pub pattern: PatternId,
    pub before: DeviceValueSnapshot,
    pub after: DeviceValueSnapshot,
}

impl DeviceValuesPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.before.retained_bytes()
            + self.after.retained_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusMixerSnapshot {
    pub volume_bits: u32,
    pub mute: bool,
    pub solo: bool,
}

#[derive(Clone, Debug)]
pub struct BusMixerPatch {
    pub target: BusId,
    pub before: BusMixerSnapshot,
    pub after: BusMixerSnapshot,
}

impl BusMixerPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[derive(Clone, Debug)]
pub struct TrackParamsBatchPatch {
    pub tracks: Vec<TrackParamsPatch>,
}

impl TrackParamsBatchPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.tracks.capacity() * std::mem::size_of::<TrackParamsPatch>()
            + self
                .tracks
                .iter()
                .map(|patch| {
                    patch
                        .retained_bytes()
                        .saturating_sub(std::mem::size_of::<TrackParamsPatch>())
                })
                .sum::<usize>()
    }
}

#[derive(Clone, Debug)]
pub struct TrackParamsPatch {
    pub target: TrackPatternId,
    pub before: TrackParamsSnapshot,
    pub after: TrackParamsSnapshot,
    pub instrument_base_note_offset_before: u32,
    pub instrument_base_note_offset_after: u32,
}

impl TrackParamsPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + track_params_heap_bytes(&self.before)
            + track_params_heap_bytes(&self.after)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportAuthoringSnapshot {
    pub bpm: u32,
    pub master_volume_bits: u32,
    pub reverb_size_bits: u32,
    pub reverb_brightness_bits: u32,
    pub reverb_replace_bits: u32,
}

#[derive(Clone, Debug)]
pub struct TransportParamsPatch {
    pub before: TransportAuthoringSnapshot,
    pub after: TransportAuthoringSnapshot,
}

impl TransportParamsPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[derive(Clone, Debug)]
pub struct PatternGeometryPatch {
    pub target: TrackPatternId,
    pub num_steps_before: usize,
    pub num_steps_after: usize,
    pub cells: StepCellsPatch,
}

impl PatternGeometryPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.cells.retained_bytes()
    }
}

#[derive(Clone, Debug)]
pub struct StepCellsPatch {
    pub target: TrackPatternId,
    pub cells: Vec<StepCellDelta>,
    pub variant_registry_before: PlockVariantRegistry,
    pub variant_registry_after: PlockVariantRegistry,
}

#[derive(Clone, Debug)]
pub struct StepCellDelta {
    pub step: usize,
    pub before: StepCellSnapshot,
    pub after: StepCellSnapshot,
}

impl StepCellsPatch {
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .cells
                .iter()
                .map(|cell| {
                    std::mem::size_of::<StepCellDelta>()
                        + step_snapshot_heap_bytes(&cell.before)
                        + step_snapshot_heap_bytes(&cell.after)
                })
                .sum::<usize>()
            + registry_heap_bytes(&self.variant_registry_before)
            + registry_heap_bytes(&self.variant_registry_after)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MergeKey(String);

impl MergeKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GestureId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveGesture {
    pub id: GestureId,
    pub merge_key: MergeKey,
}

#[derive(Clone)]
struct PendingGesture<P> {
    label: String,
    merge_key: MergeKey,
    patch: P,
    retained_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryBudget {
    pub max_entries: usize,
    pub max_bytes: usize,
}

impl Default for HistoryBudget {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_HISTORY_ENTRY_LIMIT,
            max_bytes: DEFAULT_HISTORY_BYTE_LIMIT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HistoryEntry<P> {
    pub revision_before: u64,
    pub revision_after: u64,
    pub label: String,
    pub merge_key: Option<MergeKey>,
    pub patch: P,
    pub retained_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryMove {
    pub label: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryReplay<E> {
    Unavailable,
    Applied(HistoryMove),
    Failed(E),
}

/// Session-local linear undo history.
///
/// Patch replay is supplied by the edit executor. An entry changes stacks only
/// after that replay succeeds, which makes a failed undo or redo non-destructive.
#[derive(Clone)]
pub struct UndoManager<P> {
    undo: VecDeque<HistoryEntry<P>>,
    redo: Vec<HistoryEntry<P>>,
    current_revision: u64,
    next_revision: u64,
    saved_revision: Option<u64>,
    retained_bytes: usize,
    budget: HistoryBudget,
    active_gesture: Option<ActiveGesture>,
    active_gesture_updated_at: Option<Instant>,
    pending_gesture: Option<PendingGesture<P>>,
    newest_entry_exceeds_byte_budget: bool,
}

impl<P> Default for UndoManager<P> {
    fn default() -> Self {
        Self::new(HistoryBudget::default())
    }
}

impl<P> UndoManager<P> {
    pub fn new(budget: HistoryBudget) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            current_revision: 0,
            next_revision: 1,
            saved_revision: None,
            retained_bytes: 0,
            budget,
            active_gesture: None,
            active_gesture_updated_at: None,
            pending_gesture: None,
            newest_entry_exceeds_byte_budget: false,
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn next_undo_patch(&self) -> Option<&P> {
        self.undo.back().map(|entry| &entry.patch)
    }

    pub fn next_redo_patch(&self) -> Option<&P> {
        self.redo.last().map(|entry| &entry.patch)
    }

    pub fn recent_undo_patches(&self, count: usize) -> Option<Vec<&P>> {
        if count == 0 || count > self.undo.len() {
            return None;
        }
        Some(self.undo.iter().skip(self.undo.len() - count).map(|entry| &entry.patch).collect())
    }

    pub fn squash_recent(
        &mut self,
        count: usize,
        label: impl Into<String>,
        patch: P,
        retained_bytes: usize,
    ) -> Option<HistoryMove> {
        if count < 2 || count > self.undo.len() {
            return None;
        }
        let first = self.undo.len() - count;
        let revision_before = self.undo.get(first)?.revision_before;
        let revision_after = self.undo.back()?.revision_after;
        let removed_bytes = self.undo.iter().skip(first)
            .map(|entry| entry.retained_bytes).sum::<usize>();
        if self.saved_revision.is_some_and(|revision| {
            revision != revision_before
                && revision != revision_after
                && self.undo.iter().skip(first).any(|entry| entry.revision_after == revision)
        }) {
            self.saved_revision = None;
        }
        self.undo.truncate(first);
        self.retained_bytes = self.retained_bytes.saturating_sub(removed_bytes);
        let label = label.into();
        self.undo.push_back(HistoryEntry {
            revision_before,
            revision_after,
            label: label.clone(),
            merge_key: None,
            patch,
            retained_bytes,
        });
        self.retained_bytes = self.retained_bytes.saturating_add(retained_bytes);
        self.enforce_budget(Some(revision_after));
        Some(HistoryMove { label, revision: revision_after })
    }

    pub fn current_revision(&self) -> u64 {
        self.current_revision
    }

    pub fn saved_revision(&self) -> Option<u64> {
        self.saved_revision
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn active_gesture(&self) -> Option<&ActiveGesture> {
        self.active_gesture.as_ref()
    }

    pub fn newest_entry_exceeds_byte_budget(&self) -> bool {
        self.newest_entry_exceeds_byte_budget
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = Some(self.current_revision);
    }

    pub fn is_at_saved_revision(&self) -> bool {
        self.pending_gesture.is_none() && self.saved_revision == Some(self.current_revision)
    }

    pub fn begin_gesture(&mut self, gesture: ActiveGesture) -> Result<(), ActiveGesture> {
        if self.active_gesture.is_some() {
            return Err(gesture);
        }
        self.active_gesture = Some(gesture);
        self.active_gesture_updated_at = Some(Instant::now());
        Ok(())
    }

    pub fn finish_gesture(&mut self, id: GestureId) -> Option<ActiveGesture> {
        if self.active_gesture.as_ref().map(|gesture| gesture.id) != Some(id) {
            return None;
        }
        self.finish_active_gesture()
    }

    pub fn finish_active_gesture(&mut self) -> Option<ActiveGesture> {
        self.active_gesture_updated_at = None;
        let gesture = self.active_gesture.take();
        if let Some(pending) = self.pending_gesture.take() {
            self.commit(
                pending.label,
                Some(pending.merge_key),
                pending.patch,
                pending.retained_bytes,
            );
        }
        gesture
    }

    pub fn finish_active_gesture_if_idle(&mut self, idle_for: Duration) -> Option<ActiveGesture> {
        if !self.active_gesture_is_idle(idle_for) {
            return None;
        }
        self.finish_active_gesture()
    }

    pub fn active_gesture_is_idle(&self, idle_for: Duration) -> bool {
        self.active_gesture_updated_at
            .is_some_and(|updated| updated.elapsed() >= idle_for)
    }

    pub fn active_gesture_patch(&self, merge_key: &MergeKey) -> Option<&P> {
        if self.active_gesture.as_ref().map(|gesture| &gesture.merge_key) != Some(merge_key) {
            return None;
        }
        self.pending_gesture
            .as_ref()
            .filter(|pending| &pending.merge_key == merge_key)
            .map(|pending| &pending.patch)
    }

    pub fn stage_active_gesture(
        &mut self,
        label: impl Into<String>,
        merge_key: &MergeKey,
        patch: P,
        retained_bytes: usize,
    ) -> Option<HistoryMove> {
        if self.active_gesture.as_ref().map(|gesture| &gesture.merge_key) != Some(merge_key) {
            return None;
        }
        let label = label.into();
        self.pending_gesture = Some(PendingGesture {
            label: label.clone(),
            merge_key: merge_key.clone(),
            patch,
            retained_bytes,
        });
        self.active_gesture_updated_at = Some(Instant::now());
        Some(HistoryMove {
            label,
            revision: self.current_revision,
        })
    }

    pub fn discard_active_gesture_entry(&mut self, merge_key: &MergeKey) -> bool {
        if self.active_gesture.as_ref().map(|gesture| &gesture.merge_key) != Some(merge_key)
            || self
                .pending_gesture
                .as_ref()
                .map(|pending| &pending.merge_key)
                != Some(merge_key)
        {
            return false;
        }
        self.pending_gesture = None;
        self.active_gesture = None;
        self.active_gesture_updated_at = None;
        true
    }

    pub fn commit(
        &mut self,
        label: impl Into<String>,
        merge_key: Option<MergeKey>,
        patch: P,
        retained_bytes: usize,
    ) -> HistoryMove {
        self.clear_redo();
        let revision_before = self.current_revision;
        let revision_after = self.take_revision();
        let label = label.into();
        self.undo.push_back(HistoryEntry {
            revision_before,
            revision_after,
            label: label.clone(),
            merge_key,
            patch,
            retained_bytes,
        });
        self.retained_bytes = self.retained_bytes.saturating_add(retained_bytes);
        self.current_revision = revision_after;
        self.enforce_budget(Some(revision_after));
        HistoryMove {
            label,
            revision: revision_after,
        }
    }

    pub fn undo<E>(
        &mut self,
        apply: impl FnOnce(&P) -> Result<(), E>,
    ) -> HistoryReplay<E> {
        let Some(entry) = self.undo.back() else {
            return HistoryReplay::Unavailable;
        };
        if let Err(error) = apply(&entry.patch) {
            return HistoryReplay::Failed(error);
        }
        let entry = self.undo.pop_back().expect("undo entry disappeared");
        self.current_revision = entry.revision_before;
        let result = HistoryMove {
            label: entry.label.clone(),
            revision: self.current_revision,
        };
        self.redo.push(entry);
        HistoryReplay::Applied(result)
    }

    pub fn redo<E>(
        &mut self,
        apply: impl FnOnce(&P) -> Result<(), E>,
    ) -> HistoryReplay<E> {
        let Some(entry) = self.redo.last() else {
            return HistoryReplay::Unavailable;
        };
        if let Err(error) = apply(&entry.patch) {
            return HistoryReplay::Failed(error);
        }
        let entry = self.redo.pop().expect("redo entry disappeared");
        self.current_revision = entry.revision_after;
        let result = HistoryMove {
            label: entry.label.clone(),
            revision: self.current_revision,
        };
        self.undo.push_back(entry);
        HistoryReplay::Applied(result)
    }

    /// Record a successful unsupported authoring mutation.
    pub fn barrier(&mut self) {
        self.clear_entries();
        self.active_gesture = None;
        self.active_gesture_updated_at = None;
        self.pending_gesture = None;
        self.current_revision = self.take_revision();
    }

    /// Reset history after a successful project replacement.
    pub fn reset(&mut self) {
        self.clear_entries();
        self.current_revision = 0;
        self.next_revision = 1;
        self.saved_revision = None;
        self.active_gesture = None;
        self.active_gesture_updated_at = None;
        self.pending_gesture = None;
    }

    fn take_revision(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.saturating_add(1);
        revision
    }

    fn clear_redo(&mut self) {
        for entry in self.redo.drain(..) {
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        }
    }

    fn clear_entries(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.retained_bytes = 0;
        self.newest_entry_exceeds_byte_budget = false;
    }

    fn enforce_budget(&mut self, protected_revision: Option<u64>) {
        self.newest_entry_exceeds_byte_budget = protected_revision
            .and_then(|revision| {
                self.undo
                    .back()
                    .filter(|entry| entry.revision_after == revision)
            })
            .is_some_and(|entry| entry.retained_bytes > self.budget.max_bytes);

        while self.undo.len() + self.redo.len() > self.budget.max_entries
            || self.retained_bytes > self.budget.max_bytes
        {
            let undo_revision = self.undo.front().map(|entry| entry.revision_after);
            let redo_revision = self.redo.last().map(|entry| entry.revision_after);
            let remove_undo = match (undo_revision, redo_revision) {
                (Some(undo_revision), Some(redo_revision)) => undo_revision <= redo_revision,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            let candidate_revision = if remove_undo {
                undo_revision
            } else {
                redo_revision
            };
            if candidate_revision == protected_revision {
                break;
            }
            let entry = if remove_undo {
                self.undo.pop_front().expect("oldest undo entry disappeared")
            } else {
                self.redo.pop().expect("oldest redo entry disappeared")
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.retained_bytes);
        }
    }
}

pub fn bit_exact_f32_eq(left: f32, right: f32) -> bool {
    left.to_bits() == right.to_bits()
}

fn bit_exact_f32_slice_eq(left: &[f32], right: &[f32]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| bit_exact_f32_eq(*left, *right))
}

fn optional_f32_eq(left: Option<f32>, right: Option<f32>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => bit_exact_f32_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn slot_plocks_eq(left: &StepSlotPlocks, right: &StepSlotPlocks) -> bool {
    left.params.len() == right.params.len()
        && left
            .params
            .iter()
            .zip(&right.params)
            .all(|(left, right)| optional_f32_eq(*left, *right))
        && left.tensor_params.len() == right.tensor_params.len()
        && left
            .tensor_params
            .iter()
            .zip(&right.tensor_params)
            .all(|(left, right)| match (left, right) {
                (Some(left), Some(right)) => bit_exact_f32_slice_eq(left, right),
                (None, None) => true,
                _ => false,
            })
}

fn slot_plock_slice_eq(left: &[StepSlotPlocks], right: &[StepSlotPlocks]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| slot_plocks_eq(left, right))
}

pub fn step_snapshot_bit_exact_eq(
    left: &StepCellSnapshot,
    right: &StepCellSnapshot,
) -> bool {
    let crate::sequencer::StepSnapshot {
        active: left_active,
        neural_reset: left_neural_reset,
        params: left_params,
        chord: left_chord,
        chord_durations: left_chord_durations,
        chord_delays: left_chord_delays,
        timebase: left_timebase,
        swing: left_swing,
        swing_resolution: left_swing_resolution,
        midi_fx_plocks: left_midi_fx_plocks,
        effect_plocks: left_effect_plocks,
        instrument_plocks: left_instrument_plocks,
        rack_macro_plocks: left_rack_macro_plocks,
        rack_slot_param_plocks: left_rack_slot_param_plocks,
        rack_slot_instrument_plocks: left_rack_slot_instrument_plocks,
        rack_slot_effect_plocks: left_rack_slot_effect_plocks,
    } = left;
    let crate::sequencer::StepSnapshot {
        active: right_active,
        neural_reset: right_neural_reset,
        params: right_params,
        chord: right_chord,
        chord_durations: right_chord_durations,
        chord_delays: right_chord_delays,
        timebase: right_timebase,
        swing: right_swing,
        swing_resolution: right_swing_resolution,
        midi_fx_plocks: right_midi_fx_plocks,
        effect_plocks: right_effect_plocks,
        instrument_plocks: right_instrument_plocks,
        rack_macro_plocks: right_rack_macro_plocks,
        rack_slot_param_plocks: right_rack_slot_param_plocks,
        rack_slot_instrument_plocks: right_rack_slot_instrument_plocks,
        rack_slot_effect_plocks: right_rack_slot_effect_plocks,
    } = right;
    left_active == right_active
        && left_neural_reset == right_neural_reset
        && bit_exact_f32_slice_eq(left_params, right_params)
        && bit_exact_f32_slice_eq(left_chord, right_chord)
        && bit_exact_f32_slice_eq(left_chord_durations, right_chord_durations)
        && bit_exact_f32_slice_eq(left_chord_delays, right_chord_delays)
        && left_timebase == right_timebase
        && optional_f32_eq(*left_swing, *right_swing)
        && left_swing_resolution == right_swing_resolution
        && slot_plock_slice_eq(left_midi_fx_plocks, right_midi_fx_plocks)
        && slot_plock_slice_eq(left_effect_plocks, right_effect_plocks)
        && slot_plocks_eq(left_instrument_plocks, right_instrument_plocks)
        && left_rack_macro_plocks.len() == right_rack_macro_plocks.len()
        && left_rack_macro_plocks
            .iter()
            .zip(right_rack_macro_plocks)
            .all(|(left, right)| optional_f32_eq(*left, *right))
        && slot_plock_slice_eq(
            left_rack_slot_param_plocks,
            right_rack_slot_param_plocks,
        )
        && slot_plock_slice_eq(
            left_rack_slot_instrument_plocks,
            right_rack_slot_instrument_plocks,
        )
        && left_rack_slot_effect_plocks.len() == right_rack_slot_effect_plocks.len()
        && left_rack_slot_effect_plocks
            .iter()
            .zip(right_rack_slot_effect_plocks)
            .all(|(left, right)| slot_plock_slice_eq(left, right))
}

pub fn registry_bit_exact_eq(
    left: &PlockVariantRegistry,
    right: &PlockVariantRegistry,
) -> bool {
    left.previous_step_keys == right.previous_step_keys
        && left.entries.len() == right.entries.len()
        && left.entries.iter().zip(&right.entries).all(|(left, right)| {
            left.key == right.key
                && left.label == right.label
                && left.name == right.name
                && left.color_index == right.color_index
                && bit_exact_f32_slice_eq(&left.color, &right.color)
        })
}

fn slot_plocks_heap_bytes(plocks: &StepSlotPlocks) -> usize {
    plocks.params.capacity() * std::mem::size_of::<Option<f32>>()
        + plocks.tensor_params.capacity() * std::mem::size_of::<Option<Vec<f32>>>()
        + plocks
            .tensor_params
            .iter()
            .flatten()
            .map(|values| values.capacity() * std::mem::size_of::<f32>())
            .sum::<usize>()
}

fn track_params_heap_bytes(snapshot: &TrackParamsSnapshot) -> usize {
    let TrackParamsSnapshot {
        gate: _, attack_ms: _, release_ms: _, swing: _, swing_resolution: _, num_steps: _,
        volume: _, pan: _, mute: _, solo: _, send: _, output: _, sends, polyphonic: _,
        max_polyphony: _, timebase: _, accumulator_idx: _, script_accumulator_name,
        midi_fx_chain, midi_fx_position: _, accum_limit: _, accum_mode: _, fts_scale: _,
        mute_group: _, global_transpose: _,
    } = snapshot;
    sends.capacity() * std::mem::size_of::<crate::sequencer::TrackSendSnapshot>()
        + script_accumulator_name
            .as_ref()
            .map(String::capacity)
            .unwrap_or(0)
        + midi_fx_chain.capacity() * std::mem::size_of::<String>()
        + midi_fx_chain
            .iter()
            .map(String::capacity)
            .sum::<usize>()
}

fn step_snapshot_heap_bytes(snapshot: &StepCellSnapshot) -> usize {
    let crate::sequencer::StepSnapshot {
        active: _, neural_reset: _, params: _, chord, chord_durations, chord_delays,
        timebase: _, swing: _, swing_resolution: _, midi_fx_plocks, effect_plocks,
        instrument_plocks, rack_macro_plocks, rack_slot_param_plocks,
        rack_slot_instrument_plocks, rack_slot_effect_plocks,
    } = snapshot;
    let slot_slice_bytes = |slots: &Vec<StepSlotPlocks>| {
        slots.capacity() * std::mem::size_of::<StepSlotPlocks>()
            + slots.iter().map(slot_plocks_heap_bytes).sum::<usize>()
    };
    chord.capacity() * std::mem::size_of::<f32>()
        + chord_durations.capacity() * std::mem::size_of::<f32>()
        + chord_delays.capacity() * std::mem::size_of::<f32>()
        + slot_slice_bytes(midi_fx_plocks)
        + slot_slice_bytes(effect_plocks)
        + slot_plocks_heap_bytes(instrument_plocks)
        + rack_macro_plocks.capacity() * std::mem::size_of::<Option<f32>>()
        + slot_slice_bytes(rack_slot_param_plocks)
        + slot_slice_bytes(rack_slot_instrument_plocks)
        + rack_slot_effect_plocks.capacity()
            * std::mem::size_of::<Vec<StepSlotPlocks>>()
        + rack_slot_effect_plocks
            .iter()
            .map(|slots| slot_slice_bytes(slots))
            .sum::<usize>()
}

fn registry_heap_bytes(registry: &PlockVariantRegistry) -> usize {
    registry.entries.capacity()
        * std::mem::size_of::<crate::plock_variants::PlockVariantRegistryEntry>()
        + registry
            .entries
            .iter()
            .map(|entry| {
                entry.key.entries.capacity()
                    * std::mem::size_of::<crate::plock_variants::PlockVariantEntry>()
                    + entry.label.capacity()
                    + entry.name.as_ref().map(String::capacity).unwrap_or(0)
            })
            .sum::<usize>()
        + registry.previous_step_keys.capacity()
            * std::mem::size_of::<Option<crate::plock_variants::PlockVariantKey>>()
        + registry
            .previous_step_keys
            .iter()
            .flatten()
            .map(|key| {
                key.entries.capacity()
                    * std::mem::size_of::<crate::plock_variants::PlockVariantEntry>()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn manager(max_entries: usize, max_bytes: usize) -> UndoManager<i32> {
        UndoManager::new(HistoryBudget {
            max_entries,
            max_bytes,
        })
    }

    #[test]
    fn stack_transitions_preserve_failed_replay_and_clear_redo_on_new_edit() {
        let mut history = manager(8, 1024);
        history.commit("one", None, 1, 10);
        history.commit("two", None, 2, 20);

        assert_eq!(history.undo(|_| Err("rejected")), HistoryReplay::Failed("rejected"));
        assert_eq!((history.undo_len(), history.redo_len()), (2, 0));
        assert_eq!(history.current_revision(), 2);

        assert!(matches!(history.undo(|_| Ok::<_, ()>(())), HistoryReplay::Applied(_)));
        assert_eq!((history.undo_len(), history.redo_len()), (1, 1));
        assert_eq!(history.current_revision(), 1);

        history.commit("three", None, 3, 30);
        assert_eq!((history.undo_len(), history.redo_len()), (2, 0));
        assert_eq!(history.retained_bytes(), 40);
        assert_eq!(history.redo(|_| Ok::<_, ()>(())), HistoryReplay::Unavailable);
    }

    #[test]
    fn entry_and_byte_budgets_evict_oldest_but_keep_oversized_newest_entry() {
        let mut history = manager(2, 50);
        history.commit("one", None, 1, 20);
        history.commit("two", None, 2, 20);
        history.commit("three", None, 3, 20);
        assert_eq!(history.undo_len(), 2);
        assert_eq!(history.retained_bytes(), 40);

        history.commit("oversized", None, 4, 80);
        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.retained_bytes(), 80);
        assert!(history.newest_entry_exceeds_byte_budget());
    }

    #[test]
    fn budget_eviction_releases_only_evicted_structural_resources() {
        let first = Arc::new("compiled-first");
        let second = Arc::new("compiled-second");
        let third = Arc::new("compiled-third");
        let mut history = UndoManager::new(HistoryBudget {
            max_entries: 2,
            max_bytes: 1024,
        });
        history.commit("first", None, Arc::clone(&first), 16);
        history.commit("second", None, Arc::clone(&second), 16);
        history.commit("third", None, Arc::clone(&third), 16);

        assert_eq!(Arc::strong_count(&first), 1, "the evicted resource must be released");
        assert_eq!(Arc::strong_count(&second), 2);
        assert_eq!(Arc::strong_count(&third), 2);
        assert!(matches!(history.undo(|_| Ok::<_, ()>(())), HistoryReplay::Applied(_)));
        assert_eq!(Arc::strong_count(&third), 2, "redo must retain the structural resource");
        history.reset();
        assert_eq!(Arc::strong_count(&second), 1);
        assert_eq!(Arc::strong_count(&third), 1);
    }

    #[test]
    fn barrier_and_project_reset_advance_or_restart_revision_lifetime() {
        let mut history = manager(8, 1024);
        history.commit("edit", None, 1, 10);
        history.mark_saved();
        history
            .begin_gesture(ActiveGesture {
                id: GestureId(7),
                merge_key: MergeKey::new("step-drag"),
            })
            .expect("start gesture");
        assert!(history.is_at_saved_revision());

        history.barrier();
        assert_eq!((history.undo_len(), history.redo_len()), (0, 0));
        assert_eq!(history.current_revision(), 2);
        assert!(!history.is_at_saved_revision());
        assert_eq!(history.active_gesture(), None);

        history.reset();
        assert_eq!(history.current_revision(), 0);
        assert_eq!(history.saved_revision(), None);
    }

    #[test]
    fn fallback_idle_boundary_finishes_sources_without_end_events() {
        let mut history = manager(8, 1024);
        history
            .begin_gesture(ActiveGesture {
                id: GestureId(9),
                merge_key: MergeKey::new("wheel-volume"),
            })
            .expect("begin wheel gesture");
        assert!(history
            .finish_active_gesture_if_idle(Duration::ZERO)
            .is_some());
        assert_eq!(history.active_gesture(), None);
    }

    #[test]
    fn staged_gesture_commits_once_at_end_and_marks_pending_state_dirty() {
        let mut history = manager(8, 1024);
        let key = MergeKey::new("volume-drag");
        history
            .begin_gesture(ActiveGesture {
                id: GestureId(10),
                merge_key: key.clone(),
            })
            .expect("begin volume gesture");
        assert!(history
            .stage_active_gesture("Volume", &key, 1, 8)
            .is_some());
        assert!(history
            .stage_active_gesture("Volume", &key, 2, 8)
            .is_some());
        assert_eq!(history.undo_len(), 0);
        history.finish_active_gesture();
        assert_eq!(history.undo_len(), 1);
        history.mark_saved();
        assert!(history.is_at_saved_revision());

        history
            .begin_gesture(ActiveGesture {
                id: GestureId(11),
                merge_key: key.clone(),
            })
            .expect("begin second volume gesture");
        history.stage_active_gesture("Volume", &key, 3, 8);
        assert!(!history.is_at_saved_revision());
        history.finish_active_gesture();
        assert_eq!(history.undo_len(), 2);
    }

    #[test]
    fn float_equality_is_bit_exact_for_signed_zero_and_nan_payloads() {
        assert!(!bit_exact_f32_eq(0.0, -0.0));
        let first_nan = f32::from_bits(0x7fc0_0001);
        let same_nan = f32::from_bits(0x7fc0_0001);
        let other_nan = f32::from_bits(0x7fc0_0002);
        assert!(bit_exact_f32_eq(first_nan, same_nan));
        assert!(!bit_exact_f32_eq(first_nan, other_nan));
    }
}
