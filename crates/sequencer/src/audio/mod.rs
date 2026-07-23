/*!
The real-time audio engine: everything between the sequencer's shared state
and the samples that leave the speakers.

The module is organized around one producer/consumer seam. A scheduler thread
(see `crate::scheduler`) resolves the timeline into `ScheduledEvent`s; the
CPAL callback in this module drains them, fires voices into the C audiograph,
and renders blocks. `engine` builds the graph and owns the stream;
`audiograph` is the raw FFI layer everything sits on.

Submodule map (each file's header has details):

- `engine` / `stream` / `device` — construction: graph + bus setup, CPAL
  device selection, output-stream build, helper-thread spawning.
- `state` — `AudioCallbackData`, the hub struct threaded through the whole
  callback, plus live keyboard-note bookkeeping.
- `callback` — the per-block CPAL callback orchestrating everything below.
- `events` — callback-side scheduled-event queues (countdown/block/gate-off/
  chop) and their dispatch.
- `fire` / `rack` — note firing: resolving a trigger into voice allocations,
  param pushes, and graph events (rack variants handle slot routing, macros,
  and choke groups).
- `params` — plock/key-lock/default parameter resolution for instruments,
  samplers, and rack slots.
- `voices` — sampler/graph-node allocation plus custom-engine allocation,
  stealing, release tails, and topology sync when tracks change.
- `graph_dispatch` — unsafe wrappers that push params, block events, and
  triggers into the live graph.
- `render` — block rendering, metronome mix, peak metering, and bus-gate/
  transport-clock sync.

Every submodule starts with `use super::*;` and shares this module's flat
namespace, mirroring the single-file `audio.rs` this was split from.
*/

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
mod voices;

pub use voices::MAX_VOICES;

// Flat namespace across the audio submodules: every file starts with
// `use super::*;`, so items resolve exactly as they did in the old
// single-file audio.rs.
#[allow(unused_imports)]
use {
    callback::*, device::*, events::*, fire::*, graph_dispatch::*, params::*, rack::*, render::*,
    state::*, stream::*, voices::*,
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
use crate::instruments::sampler::{
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

const FALLBACK_SAMPLE_RATE: u32 = 44_100;
const CUSTOM_ENGINE_RELEASE_TAIL_SECONDS: f64 = 20.0;
const SCHEDULED_EVENT_QUEUE_CAPACITY: usize = 4096;
const SCHEDULED_COUNTDOWN_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;
const SCHEDULED_BLOCK_SCRATCH_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;

#[cfg(test)]
mod tests;
