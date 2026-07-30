use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::effects::{
    EffectDescriptor, EffectSlotSnapshot, EffectSlotState, EffectSlotValuesSnapshot, HostControl,
    MAX_SLOT_PARAMS,
};
use crate::graph::{GraphVisualizationSnapshot, ProjectGraphOverrides};
use crate::neural::{
    remap_neural_network_routes_after_track_delete, NeuralVisualizationSnapshot,
    ProjectNeuralNetwork,
};
use crate::plock_variants::{
    live_track_key_lock_variant_key, live_track_key_lock_variant_keys, live_track_variant_key,
    live_track_variant_keys, PlockVariantAssignment, PlockVariantDomain, PlockVariantKey,
    PlockVariantRegistry,
};
use crate::audio::MAX_VOICES;

use super::data::{
    sync_beats, ChordData, ChordSnapshot, CustomInstrumentRunMode, InstrumentType, ModConnection,
    RackRouting, StepData, StepParam, SwingPLockData, SwingResolution, SwingResolutionPLockData,
    PatternStepGeometry, Timebase, TimebasePLockData, TrackParams, TrackParamsSnapshot,
    TrackPattern, TrackSoundState,
    DEFAULT_BPM, EXT_MOD_INPUT_COUNT, MAX_INSTRUMENT_ENGINES, MAX_RACK_SLOTS, MAX_SAMPLER_POOLS,
    MAX_STEPS, MAX_TRACKS, NUM_PARAMS, TRACK_PATTERN_WORDS,
};
use super::snapshot::{SequencerSnapshot, SequencerTransportSnapshot};
use super::{BusId, TrackOutput};













mod ids;
pub use ids::*;
mod track_registry;
pub use track_registry::*;
mod rack_macro;
pub use rack_macro::*;
mod rack_slot;
pub use rack_slot::*;
mod bus_pattern;
pub use bus_pattern::*;
mod step_snapshot;
pub use step_snapshot::*;
mod pattern_snapshot;
pub use pattern_snapshot::*;
mod track_pattern_data;
pub use track_pattern_data::*;
mod sound_entities;
pub use sound_entities::*;
mod scenes;
pub use scenes::*;
mod takes;
pub use takes::*;
mod song;
pub use song::*;
mod song_runtime;
pub use song_runtime::*;
mod arrangement;
pub use arrangement::*;
mod track_delete_remap;
use track_delete_remap::*;
mod core;
pub use core::*;
mod variant_lock_helpers;
use variant_lock_helpers::*;
mod sequencer_state;

#[cfg(test)]
mod tests;
