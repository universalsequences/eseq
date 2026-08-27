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
    ceil_to_grid, rack_slot_pool_index, sync_beats, BusId, ChordData, ChordSnapshot,
    CustomInstrumentRunMode, PatternStepGeometry,
    InstrumentType, KeyboardTrigger, MidiFxPosition, ModConnection, ModDestination,
    LiveTriggerStamp, LiveTriggerStampRing, RollCommand, RollHitRecorded,
    StepData, StepParam, SwingResolution, Timebase, TimebasePLockData, TrackOutput, TrackParams,
    TrackParamsSnapshot, TrackPattern, TrackSendBaseline, TrackSendPLockData,
    TrackSendRuntimeTarget, TrackSendSnapshot, TrackSoundState, Trigger, DEFAULT_BPM,
    DEFAULT_BUS_A_ID, DEFAULT_BUS_B_ID, DRUM_RACK_FIRST_PAD_NOTE, DRUM_RACK_LAST_PAD_NOTE,
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
    arrangement_for_serialization, arrangement_scene_spans,
    compile_arrangement, insert_clip_sorted, legacy_backdrop_spans, migrate_legacy_backdrops,
    lower_rows_to_arrangement, occlude_span, pattern_play_step, restamped_clip, stamp_scene_clips,
    stamped_clip_override, ArrClip, LegacyBackdropSpan, SceneSpan,
    ArrangementContext, ClipId, ProjectArrangement, SceneEvent, SongCompileContext,
    DEFAULT_ARRANGEMENT_END,
    default_empty_effect_chain, default_rack_macros, format_song_row_positions, project_lanes,
    remap_scene_index_after_move, remap_song_after_scene_delete, remap_song_after_scene_move,
    song_for_serialization, song_rows_referencing_scene,
    song_rows_referencing_track_pattern, state_at_beat, BusPatternSnapshot,
    validate_track_take_pool, LaneClip, LaneSource, ProjectSong, ProjectSongRow,
    ProjectSongTrackOverride, SerializedSongContext, SongProjectContext, SongRowId, TakeId,
    TrackTake, TrackTakePool,
    ActiveNoteActivity, AudibleSongRowApplied, RuntimeSong, RuntimeSongRow, SongChunkPlan,
    SongPlaybackCommand, SongPlaybackMailbox, SongPlaybackNotice, SongPlaybackRuntime,
    SongPositionShared,
    EffectInstanceId, InstrumentDeviceValuesSnapshot, InstrumentSlotResetSummary,
    GeneratorTickErrorNotice, MidiFxInstanceId, PatternId, PatternSnapshot, ProjectScenes, PublishedSequencer, RackMacro, RackMacroCurve,
    ResolvedSceneSlot, SceneSlotStore, SCENE_SLOT_SOFT_SERIALIZED_BYTES,
    rack_choke_key,
    RackMacroId, RackMacroMapping, RackMacroTarget, RackSlotId, RackSlotParam,
    RackSlotParamPlocks, RackSlotSnapshot, RackSlotValuesSnapshot, RackTrackSnapshot,
    RackMacroPatternStateSnapshot, RackSlotPatternStateSnapshot, RecordPosition, SequencerState,
    StepCellSnapshot,
    StepSlotPlocks, StepSnapshot, TrackId,
    TrackInstrumentPatternState, TrackInstrumentPatternStateSnapshot, TrackOutputEvent,
    Mix, MixId, Patch, PatchId, SoundEntityMeta, SoundRefs, StoredPattern, TrackPatternSeq,
    TrackSoundPool, SOUND_COLOR_SET,
    Scene, SceneBank, SceneBankId, SceneId, TrackPatternCellView, TrackPatternData, TrackPatternId, TrackPatternPool, TrackRegistry, TrackRegistryError,
    TrackPatternLaneState, MAX_SCENES_PER_BANK,
    NeuralInstrumentOverrideState, TrackEffectBindingStateSnapshot,
    RACK_MACRO_COUNT, RACK_SLOT_PARAM_COUNT,
};
