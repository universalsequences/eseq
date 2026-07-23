pub mod audiograph;
pub mod engine;

mod callback;
mod device;
mod events;
mod fire;
mod graph_dispatch;
mod params;
mod rack;
mod render;
mod state;
mod stream;
mod voice_pool;

// Flat namespace across the audio submodules: every file starts with
// `use super::*;`, so items resolve exactly as they did in the old
// single-file audio.rs.
#[allow(unused_imports)]
use {
    callback::*, device::*, events::*, fire::*, graph_dispatch::*, params::*, rack::*, render::*,
    state::*, stream::*, voice_pool::*,
};

use arrayvec::ArrayVec;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::audiograph::*;
use crate::effects::gatepitch;
use crate::effects::{EffectSlotSnapshot, EffectSlotState, MAX_SLOT_PARAMS};
use crate::recorder::MasterRecorder;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_LOOP_XFADE_SAMPLES, PARAM_RELEASE_SAMPLES, PARAM_WARP_PROJECT_BPM,
    SAMPLER_EVENT_AUX_ATTACK_SAMPLES, SAMPLER_EVENT_AUX_ENABLED, SAMPLER_EVENT_AUX_END_POINT,
    SAMPLER_EVENT_AUX_GATE_MODE, SAMPLER_EVENT_AUX_GATE_SAMPLES, SAMPLER_EVENT_AUX_LOOP_MODE,
    SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES, SAMPLER_EVENT_AUX_NOTE_ON_COUNT,
    SAMPLER_EVENT_AUX_RELEASE_SAMPLES, SAMPLER_EVENT_AUX_REVERSE, SAMPLER_EVENT_AUX_SCRUB_OFFSET,
    SAMPLER_EVENT_AUX_SPEED, SAMPLER_EVENT_AUX_SR_HZ, SAMPLER_EVENT_AUX_START_POINT,
    SAMPLER_EVENT_AUX_TRANSPOSE, SAMPLER_EVENT_AUX_VELOCITY, SAMPLER_EVENT_AUX_WARP_ENABLED,
    SAMPLER_EVENT_AUX_WARP_MODE, SAMPLER_EVENT_AUX_WARP_PRESERVE,
    SAMPLER_EVENT_AUX_WARP_PROJECT_BPM, SAMPLER_EVENT_AUX_WARP_PTR_HI,
    SAMPLER_EVENT_AUX_WARP_PTR_LO, SAMPLER_EVENT_AUX_WARP_RATIO, SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM,
    SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE, SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE,
};
use crate::scheduled_event::{
    resolved_chord_transpose, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
    ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    ScheduledInstrumentParams, ScheduledInstrumentTensorParam, ScheduledInstrumentTensorParams,
    ScheduledSamplerParams,
};
use crate::sequencer::{
    rack_slot_pool_index, sync_beats, BusId, CustomInstrumentRunMode, InstrumentType,
    KeyboardTrigger, RackRouting, RackSlotParam, RackSlotSnapshot, RackTrackSnapshot,
    SequencerSnapshot, SequencerState, StepParam, SwingResolution, MAX_INSTRUMENT_ENGINES,
    MAX_RACK_SLOTS, MAX_SAMPLER_POOLS, MAX_TRACKS,
};
use crate::app::BusGateRuntimeState;
use crate::voice::{VoicePool, MAX_VOICES};

pub const FALLBACK_SAMPLE_RATE: u32 = 44_100;
const CUSTOM_ENGINE_RELEASE_TAIL_SECONDS: f64 = 20.0;
const SCHEDULED_EVENT_QUEUE_CAPACITY: usize = 4096;
const SCHEDULED_COUNTDOWN_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;
const SCHEDULED_BLOCK_SCRATCH_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;

#[cfg(test)]
mod tests;
