#[path = "sequencer/clock.rs"]
mod clock;
#[path = "sequencer/data.rs"]
mod data;
#[path = "sequencer/snapshot.rs"]
mod snapshot;
#[path = "sequencer/state/mod.rs"]
mod state;

#[allow(unused_imports)]
pub use clock::{SequencerClock, TrackClockState};
#[allow(unused_imports)]
pub use data::{
    rack_slot_pool_index, sync_beats, BusId, ChordData, ChordSnapshot, CustomInstrumentRunMode,
    InstrumentType, KeyboardTrigger, MidiFxPosition, ModConnection, ModDestination, RackRouting,
    StepData, StepParam, SwingResolution, Timebase, TimebasePLockData, TrackOutput, TrackParams,
    TrackParamsSnapshot, TrackPattern, TrackSendSnapshot, TrackSoundState, Trigger, DEFAULT_BPM,
    DEFAULT_BUS_A_ID, DEFAULT_BUS_B_ID, DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_BANK_START,
    DRUM_RACK_LAST_PAD_NOTE, DRUM_RACK_PAD_BANK_STRIDE, DRUM_RACK_PAD_COUNT,
    DRUM_RACK_TOTAL_PAD_NOTES, EXT_MOD_INPUT_COUNT, MAX_INSTRUMENT_ENGINES, MAX_RACK_SLOTS,
    MAX_SAMPLER_POOLS, MAX_STEPS, MAX_TRACKS, MIX_BUS_ID, NUM_PARAMS, STEPS_PER_PAGE, SYNC_COUNT,
    SYNC_RESOLUTIONS, TRACK_PATTERN_WORDS,
};
#[allow(unused_imports)]
pub use snapshot::{
    SequencerSnapshot, SequencerStepSnapshot, SequencerTrackSnapshot, SequencerTransportSnapshot,
};
#[allow(unused_imports)]
pub use state::{
    default_empty_effect_chain, default_rack_macros, format_song_row_positions, project_lanes,
    remap_song_after_scene_delete, song_for_serialization, song_rows_referencing_scene,
    song_rows_referencing_track_pattern, state_at_beat, BusGateSequence, BusPatternSnapshot,
    LaneClip, ProjectSong, ProjectSongRow, ProjectSongTrackOverride, SerializedSongContext,
    SongProjectContext, SongRowId,
    EffectInstanceId, InstrumentDeviceValuesSnapshot, InstrumentSlotResetSummary,
    MidiFxInstanceId, PatternId, PatternSnapshot, ProjectScenes, PublishedSequencer, RackMacro, RackMacroCurve,
    RackMacroId, RackMacroMapping, RackMacroTarget, RackSlotId, RackSlotParam,
    RackSlotParamPlocks, RackSlotSnapshot, RackSlotValuesSnapshot, RackTrackSnapshot,
    RackMacroPatternStateSnapshot, RackSlotPatternStateSnapshot, RecordPosition, SequencerState,
    StepCellSnapshot,
    StepSlotPlocks, StepSnapshot, TrackId,
    TrackInstrumentPatternState, TrackInstrumentPatternStateSnapshot, TrackOutputEvent,
    Scene, SceneId, TrackPatternCellView, TrackPatternData, TrackPatternId, TrackPatternPool, TrackRegistry, TrackRegistryError,
    TrackPatternLaneState,
    NeuralInstrumentOverrideState, TrackEffectBindingStateSnapshot,
    RACK_MACRO_COUNT, RACK_SLOT_PARAM_COUNT,
};
