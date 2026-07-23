/*!
Step-accumulator machinery: transform each triggered step with a running
per-track value.

An *accumulator* rewrites a step as it fires using accumulated state — e.g.
ramping transpose, decaying velocity, echoing an octave up, or forwarding a
copy to another track. This module owns the value/limit semantics
([`AccumMode`]'s rtz/clip/rvtz/rvbp behaviors via [`apply_limit_mode`]), the
audio-thread-safe [`ActionBuffer`] of [`StepAction`]s an accumulator emits,
and the builtin [`ACCUMULATOR_REGISTRY`] of native `AccumulatorFn`s.
Lisp-defined accumulators (`def-accumulator`, hosted in `lisp_host`) build on
the same `ResolvedStep` payload and emit path.

Also defines [`ResolvedStep`] itself — a step's parameters resolved from
`StepData` atomics into plain `f32` fields — which has outgrown this file to
become the universal event payload shared by the generator, graph, and
neural runtimes.
*/

use crate::sequencer::{StepData, StepParam};

/// A step's parameters resolved from StepData atomics into plain f32 fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedStep {
    pub duration: f32,
    pub velocity: f32,
    pub speed: f32,
    #[allow(dead_code)]
    pub aux_a: f32,
    #[allow(dead_code)]
    pub aux_b: f32,
    pub transpose: f32,
    pub pan: f32,
    pub chop: f32,
}

impl ResolvedStep {
    pub fn from_step_data(sd: &StepData, step: usize) -> Self {
        Self {
            duration: sd.get(step, StepParam::Duration),
            velocity: sd.get(step, StepParam::Velocity),
            speed: sd.get(step, StepParam::Speed),
            aux_a: sd.get(step, StepParam::AuxA),
            aux_b: sd.get(step, StepParam::AuxB),
            transpose: sd.get(step, StepParam::Transpose),
            pan: sd.get(step, StepParam::Pan),
            chop: sd.get(step, StepParam::Chop),
        }
    }
}

/// An action the accumulator can emit. The audio loop interprets each one.
#[derive(Clone, Copy)]
pub enum StepAction {
    Play(ResolvedStep),
    SendToTrack {
        track: usize,
        resolved: ResolvedStep,
    },
    #[allow(dead_code)]
    Silence,
}

/// Fixed-size action list — no heap allocation on the audio thread.
#[derive(Clone, Copy)]
pub struct ActionBuffer {
    actions: [Option<StepAction>; 8],
    count: usize,
}

impl ActionBuffer {
    pub fn just(action: StepAction) -> Self {
        let mut buf = Self {
            actions: [None; 8],
            count: 1,
        };
        buf.actions[0] = Some(action);
        buf
    }

    pub fn push(&mut self, action: StepAction) {
        if self.count < 8 {
            self.actions[self.count] = Some(action);
            self.count += 1;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &StepAction> {
        self.actions[..self.count].iter().filter_map(|a| a.as_ref())
    }
}

// ── Limit / mode ──

/// What the accumulator does when it hits its limit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum AccumMode {
    /// Reset to zero.
    Rtz = 0,
    /// Clamp at the limit.
    Clip = 1,
    /// Flip direction and reset to zero.
    Rvtz = 2,
    /// Flip direction and bounce (ping-pong between −limit and +limit).
    Rvbp = 3,
}

impl AccumMode {
    pub const COUNT: usize = 4;
    pub const LABELS: [&'static str; 4] = ["rtz", "clip", "rvtz", "rvbp"];

    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => AccumMode::Clip,
            2 => AccumMode::Rvtz,
            3 => AccumMode::Rvbp,
            _ => AccumMode::Rtz,
        }
    }

    pub fn label(self) -> &'static str {
        Self::LABELS[self as usize]
    }
}

/// Apply the limit+mode rule to a raw new accumulator value.
/// Mutates `reversed` if the mode involves direction changes.
pub fn apply_limit_mode(raw: f32, limit: f32, mode: AccumMode, reversed: &mut bool) -> f32 {
    let limit = limit.max(0.0);
    if limit == 0.0 {
        return raw;
    }
    match mode {
        AccumMode::Rtz => {
            if raw.abs() > limit {
                0.0
            } else {
                raw
            }
        }
        AccumMode::Clip => raw.clamp(-limit, limit),
        AccumMode::Rvtz => {
            if raw.abs() > limit {
                *reversed = !*reversed;
                0.0
            } else {
                raw
            }
        }
        AccumMode::Rvbp => {
            if raw.abs() > limit {
                *reversed = !*reversed;
                raw.clamp(-limit, limit)
            } else {
                raw
            }
        }
    }
}

// ── Runtime state ──

/// Per-track accumulator runtime state (lives on the audio thread).
#[derive(Clone, Copy, Default)]
pub struct AccumulatorRuntimeState {
    pub value: f32,
    pub reversed: bool,
}

// ── Function types ──

/// Transform a step using the current accumulator value. Returns (actions, raw_new_value).
/// The framework applies limit+mode to raw_new_value before storing it.
pub type AccumulatorFn =
    fn(step: ResolvedStep, aux_a: f32, state: f32, reversed: bool) -> (ActionBuffer, f32);

pub struct AccumulatorDef {
    pub name: &'static str,
    pub func: AccumulatorFn,
    /// Value the state resets to on play-start or pattern change.
    pub reset_value: f32,
    /// Suggested default limit for this accumulator.
    pub default_limit: f32,
    #[allow(dead_code)]
    pub aux_a_min: f32,
    #[allow(dead_code)]
    pub aux_a_max: f32,
    #[allow(dead_code)]
    pub aux_a_increment: f32,
}

// ── Built-in accumulators ──

fn accum_off(step: ResolvedStep, _aux_a: f32, state: f32, _rev: bool) -> (ActionBuffer, f32) {
    (ActionBuffer::just(StepAction::Play(step)), state)
}

/// Adds `aux_a` semitones to the running total each trigger, applied to the current step.
/// Direction flips when `reversed` is set (by rvtz/rvbp modes).
fn accum_transpose_ramp(
    step: ResolvedStep,
    aux_a: f32,
    state: f32,
    reversed: bool,
) -> (ActionBuffer, f32) {
    let delta = if reversed { -aux_a } else { aux_a };
    let new_raw = state + delta;
    let out = ResolvedStep {
        transpose: step.transpose + new_raw,
        ..step
    };
    (ActionBuffer::just(StepAction::Play(out)), new_raw)
}

/// Adds aux_a velocity units to the running total each trigger, applied to the current step.
/// aux_a=0.3 → +0.3/step. Use limit+mode to control the range.
fn accum_velocity_decay(
    step: ResolvedStep,
    aux_a: f32,
    state: f32,
    reversed: bool,
) -> (ActionBuffer, f32) {
    let delta = if reversed { -aux_a } else { aux_a };
    let new_raw = state + delta;
    let out = ResolvedStep {
        velocity: (step.velocity + new_raw).clamp(0.0, 1.0),
        ..step
    };
    (ActionBuffer::just(StepAction::Play(out)), new_raw)
}

fn accum_octave_echo(
    step: ResolvedStep,
    _aux_a: f32,
    state: f32,
    _rev: bool,
) -> (ActionBuffer, f32) {
    let mut buf = ActionBuffer::just(StepAction::Play(step));
    buf.push(StepAction::Play(ResolvedStep {
        transpose: step.transpose + 12.0,
        velocity: step.velocity * 0.5,
        ..step
    }));
    (buf, state)
}

fn accum_send_to_track(
    step: ResolvedStep,
    aux_a: f32,
    state: f32,
    _rev: bool,
) -> (ActionBuffer, f32) {
    let target = aux_a.round() as usize;
    let mut buf = ActionBuffer::just(StepAction::Play(step));
    buf.push(StepAction::SendToTrack {
        track: target,
        resolved: step,
    });
    (buf, state)
}

pub const ACCUMULATOR_REGISTRY: &[AccumulatorDef] = &[
    AccumulatorDef {
        name: "Off",
        func: accum_off,
        reset_value: 0.0,
        default_limit: 0.0,
        aux_a_min: 0.0,
        aux_a_max: 1.0,
        aux_a_increment: 0.05,
    },
    AccumulatorDef {
        name: "TransposeRamp",
        func: accum_transpose_ramp,
        reset_value: 0.0,
        default_limit: 48.0,
        aux_a_min: 0.0,
        aux_a_max: 16.0,
        aux_a_increment: 1.0,
    },
    AccumulatorDef {
        name: "VelocityDecay",
        func: accum_velocity_decay,
        reset_value: 0.0,
        default_limit: 1.0,
        aux_a_min: 0.0,
        aux_a_max: 1.0,
        aux_a_increment: 0.05,
    },
    AccumulatorDef {
        name: "OctaveEcho",
        func: accum_octave_echo,
        reset_value: 0.0,
        default_limit: 0.0,
        aux_a_min: 0.0,
        aux_a_max: 1.0,
        aux_a_increment: 0.05,
    },
    AccumulatorDef {
        name: "SendToTrack",
        func: accum_send_to_track,
        reset_value: 0.0,
        default_limit: 0.0,
        aux_a_min: 0.0,
        aux_a_max: 63.0,
        aux_a_increment: 1.0,
    },
];
