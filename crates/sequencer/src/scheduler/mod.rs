/*!
Timeline scheduler and the event contract consumed by the real-time audio callback.

The scheduler thread snapshots sequencer state, advances musical time, resolves
parameter locks and generated events, applies process and MIDI-FX transformations,
and publishes `ScheduledEvent`s. `scheduled_event` owns that queue vocabulary;
the audio module remains its consumer.
*/

pub mod scheduled_event;

mod clock;
mod enqueue;
mod geometry;
mod lookahead;
mod midi_fx;
mod params;
mod process;
mod roll;
mod runtime;
mod worker;

#[allow(unused_imports)]
use {
    clock::*, enqueue::*, geometry::*, lookahead::*, midi_fx::*, params::*, process::*, roll::*,
    runtime::*, worker::{reconcile_playing_topology_change, topology_edit_frontier_drained},
};

pub use worker::spawn_scheduler_thread;

use std::cell::RefCell;
use std::collections::{hash_map::DefaultHasher, BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::accumulator::{
    apply_limit_mode, AccumMode, AccumulatorRuntimeState, ResolvedStep, StepAction,
    ACCUMULATOR_REGISTRY,
};
use crate::effects::EffectDescriptor;
use crate::lisp_host::{self, AccumulatorNoteSpan};
use crate::neural::{NeuralOutput, NeuralRuntime, ParamNodeId};
use crate::process::ProcessMidiFxParamOverride;
use self::scheduled_event::{
    resolved_chord_transpose, EventSource, ScheduledChordData, ScheduledEffectParam,
    ScheduledEvent, ScheduledEventKind, ScheduledEventQueue, ScheduledInstrumentParam,
    ScheduledInstrumentParamTarget, ScheduledInstrumentParams, ScheduledInstrumentTensorParam,
    ScheduledInstrumentTensorParams, ScheduledSamplerParams, StepEvent,
};
use crate::sequencer::{
    sync_beats, InstrumentType, KeyboardTrigger, MidiFxPosition, SequencerSnapshot, SequencerState,
    StepParam, SwingResolution, TrackOutputEvent, MAX_STEPS, MAX_TRACKS,
};
use crate::audio::MAX_VOICES;

// The scheduler lookahead pass carries sizeable event values in debug builds, and
// Tests call the same extracted production pass. Use the same documented stack
// budget as Lisp UI compilation instead of depending on platform defaults.
const SCHEDULER_THREAD_STACK_SIZE: usize = crate::REQUIRED_THREAD_STACK_SIZE;
const PROCESS_EVENT_CASCADE_LIMIT: usize = 1024;

#[cfg(test)]
mod tests;
