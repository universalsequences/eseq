//! Lisp-defined generator runtime.
//!
//! A *generator* is the open, lisp-authored counterpart to the native neural
//! sequencer: a self-clocked unit that the scheduler ticks on its own timebase
//! grid each block (even with no incoming events) and that emits musical events.
//!
//! This module owns only the timing/ordering concerns — one [`GridBoundaryClock`]
//! per generator, a per-generator tick counter, and a per-generator RNG seed — and
//! is deliberately lisp-agnostic so it can be driven by a plain Rust callback in
//! tests. The actual lisp `:tick` closure invocation lives in `lisp_host.rs`; the
//! scheduler wires the two together by passing a `tick_fn` that calls into the
//! scheduler-side lisp VM.
//!
//! Emission timing is sample-accurate but expressed by the generator in *musical*
//! coordinates (boundary-relative `offset_beats` on [`EmittedAccumulatorEvent`]);
//! this runtime resolves those to absolute sample times the same way the neural
//! runtime and the accumulator emit path do.

use std::collections::HashMap;

use crate::accumulator::ResolvedStep;
use crate::lisp_host::EmittedAccumulatorEvent;
use super::grid_clock::{process_grid_boundaries, GridBoundaryClock};

/// Reference subdivision count used when converting a `Timebase` to beats. Only
/// affects `Timebase::Polyrhythm`; matches the neural runtime's convention so the
/// two clocks agree.
pub const GENERATOR_RESOLUTION_REF_STEPS: usize = 16;

const GENERATOR_DEFAULT_RANDOM_STATE: u64 = 0xA076_1D64_78BD_642F;

/// Beat slack when matching a gate trigger to a boundary: emitters round beats
/// to samples, so a fire "on" a boundary may land a hair either side of it.
const GATE_EPS_BEATS: f64 = 1e-6;

/// Undelivered gate triggers one generator holds; the oldest drop past this,
/// so a parked (never-ticking) generator cannot grow without bound.
const GATE_PENDING_CAP: usize = 64;

/// What a gate trigger does to its generator (docs/jaki-trig-modes-spec.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateTriggerKind {
    /// Open the gate for `duration_beats` and latch `note`/`velocity` (§4).
    Play,
    /// Bump only the restart epoch: no gate, no payload (§5).
    Restart,
}

/// A trigger aimed at a generator by another sequencer (a graph node routed to
/// it). `beat` is the fire's straight grid beat; it is delivered at the
/// generator's first boundary at or after it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateTrigger {
    pub kind: GateTriggerKind,
    pub beat: f64,
    pub duration_beats: f64,
    pub note: f32,
    pub velocity: f32,
}

/// The gate as one tick sees it (`gen-gate`). `epoch` counts boundaries that
/// delivered a play trigger, `restart_epoch` those that delivered a restart;
/// a tick compares them with what it last saw. Never triggered: epoch 0,
/// closed, note 0, velocity 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneratorGateView {
    pub epoch: u64,
    pub open: bool,
    pub note: f32,
    pub velocity: f32,
    pub restart_epoch: u64,
}

impl Default for GeneratorGateView {
    fn default() -> Self {
        Self {
            epoch: 0,
            open: false,
            note: 0.0,
            velocity: 1.0,
            restart_epoch: 0,
        }
    }
}

#[derive(Clone, Debug)]
struct GateState {
    epoch: u64,
    restart_epoch: u64,
    open_until_beats: f64,
    note: f32,
    velocity: f32,
    pending: Vec<GateTrigger>,
}

impl Default for GateState {
    fn default() -> Self {
        Self {
            epoch: 0,
            restart_epoch: 0,
            open_until_beats: f64::NEG_INFINITY,
            note: 0.0,
            velocity: 1.0,
            pending: Vec::new(),
        }
    }
}

impl GateState {
    fn push(&mut self, trigger: GateTrigger) {
        if self.pending.len() >= GATE_PENDING_CAP {
            self.pending.remove(0);
        }
        self.pending.push(trigger);
    }

    /// Deliver every pending trigger due by boundary `beat` and report the gate
    /// there (spec §4): the window only extends, the epochs bump once per
    /// boundary, and among play triggers delivered together the loudest sets
    /// the payload.
    fn deliver(&mut self, beat: f64) -> GeneratorGateView {
        let mut played: Option<GateTrigger> = None;
        let mut restarted = false;
        let mut open_until = self.open_until_beats;
        self.pending.retain(|trigger| {
            if trigger.beat > beat + GATE_EPS_BEATS {
                return true;
            }
            match trigger.kind {
                GateTriggerKind::Restart => restarted = true,
                GateTriggerKind::Play => {
                    open_until = open_until.max(trigger.beat + trigger.duration_beats.max(0.0));
                    if played.is_none_or(|best| trigger.velocity > best.velocity) {
                        played = Some(*trigger);
                    }
                }
            }
            false
        });
        self.open_until_beats = open_until;
        if let Some(trigger) = played {
            self.epoch += 1;
            self.note = trigger.note;
            self.velocity = trigger.velocity;
        }
        if restarted {
            self.restart_epoch += 1;
        }
        GeneratorGateView {
            epoch: self.epoch,
            open: beat < self.open_until_beats - GATE_EPS_BEATS,
            note: self.note,
            velocity: self.velocity,
            restart_epoch: self.restart_epoch,
        }
    }
}

/// A neutral event payload for generators that emit without a seed step.
pub fn default_resolved() -> ResolvedStep {
    ResolvedStep {
        duration: 1.0,
        velocity: 1.0,
        speed: 1.0,
        aux_a: 0.0,
        aux_b: 0.0,
        transpose: 0.0,
        pan: 0.0,
        chop: 1.0,
        retrig: crate::sequencer::StepParam::Retrig.default_value(),
        retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
    }
}

/// Definition of a generator instance, as reconciled from the scheduler-side
/// registry. `resolution_beats` is the generator's `:resolution` already converted
/// to beats (see [`GENERATOR_RESOLUTION_REF_STEPS`]).
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorDef {
    pub id: u64,
    pub name: String,
    pub resolution_beats: f64,
}

/// Input handed to a generator's tick callback for one boundary crossing. Carries
/// only musical/symbolic coordinates — never samples.
#[derive(Clone, Debug, Default)]
pub struct GeneratorTickInput {
    pub id: u64,
    pub generator_index: usize,
    /// 0-based count of this generator's boundary crossings since reset (`gen-tick`).
    pub tick_index: u64,
    /// Absolute sample the boundary plays at (audio clock); `gen-mark` stamps
    /// its values here so the UI shows them when they sound, not when the
    /// lookahead computed them.
    pub boundary_sample: u64,
    /// Musical position of this boundary in quarter-note beats (`gen-beat`).
    pub beat: f64,
    pub resolution_beats: f64,
    pub samples_per_quarter: f64,
    /// RNG state for `gen-rand`; the callback returns the advanced state.
    pub random_state: u64,
    /// Persistent per-generator scalar state cells (`state-get`/`state-set!`),
    /// carried in and returned (possibly mutated) by the callback.
    pub state: HashMap<String, f64>,
    /// The gate at this boundary, after delivering due triggers (`gen-gate`).
    pub gate: GeneratorGateView,
}

/// Result of a generator tick: emitted events (boundary-relative `offset_beats`)
/// plus the advanced RNG state and persistent state cells.
#[derive(Clone, Debug, Default)]
pub struct GeneratorTickResult {
    pub emitted: Vec<EmittedAccumulatorEvent>,
    /// Mixer-control holds emitted via `seq-emit-control`
    /// (docs/jaki-mixer-control-routes-spec.md).
    pub controls: Vec<crate::mixer_control::EmittedMixerControl>,
    pub random_state: u64,
    pub state: HashMap<String, f64>,
}

/// One emitted event resolved to an absolute sample time, tagged with the
/// originating generator index for the deterministic sample-then-index ordering.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorEmission {
    pub sample_time: u64,
    pub generator_index: usize,
    pub event: EmittedAccumulatorEvent,
}

/// A mixer-control hold resolved to absolute engage/release samples, ready
/// for the scheduler to push into the mixer-control mailbox.
#[derive(Clone, Debug, PartialEq)]
pub struct MixerControlEmission {
    pub engage_sample: u64,
    pub release_sample: u64,
    pub generator_index: usize,
    pub control: crate::mixer_control::EmittedMixerControl,
}

#[derive(Clone, Debug)]
struct GeneratorInstance {
    id: u64,
    name: String,
    clock: GridBoundaryClock,
    tick_count: u64,
    random_state: u64,
    state: HashMap<String, f64>,
    gate: GateState,
}

#[derive(Clone, Debug, Default)]
pub struct GeneratorRuntime {
    instances: Vec<GeneratorInstance>,
}

impl GeneratorRuntime {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Reconcile the runtime to a new set of definitions, **by id**. An instance
    /// whose id and resolution are unchanged keeps its clock / tick counter / RNG
    /// (so hot-reloading the `:tick` body or unrelated params does not interrupt a
    /// running generator); otherwise it is (re)created fresh, realigned to the
    /// current transport position. Order follows `defs`.
    pub fn sync_definitions(&mut self, defs: &[GeneratorDef], total_beats: f64) {
        let mut next = Vec::with_capacity(defs.len());
        for def in defs {
            let resolution_beats = def.resolution_beats.max(1e-9);
            let reused = self
                .instances
                .iter()
                .find(|inst| inst.id == def.id)
                .filter(|inst| (inst.clock.resolution_beats - resolution_beats).abs() <= 1e-9)
                .cloned();
            match reused {
                Some(mut inst) => {
                    inst.name = def.name.clone();
                    next.push(inst);
                }
                None => {
                    let mut clock = GridBoundaryClock::new(resolution_beats);
                    clock.realign(total_beats);
                    next.push(GeneratorInstance {
                        id: def.id,
                        name: def.name.clone(),
                        clock,
                        tick_count: 0,
                        random_state: generator_random_seed(def.id),
                        state: HashMap::new(),
                        gate: GateState::default(),
                    });
                }
            }
        }
        self.instances = next;
    }

    /// Reset all runtime state (clocks realigned, tick counters zeroed, RNG
    /// reseeded) — used on transport reset / pattern switch.
    pub fn reset(&mut self, total_beats: f64) {
        for inst in &mut self.instances {
            inst.clock.realign(total_beats);
            inst.tick_count = 0;
            inst.random_state = generator_random_seed(inst.id);
            inst.state.clear();
            inst.gate = GateState::default();
        }
    }

    /// Queue a gate trigger for generator `id` (docs/jaki-trig-modes-spec.md
    /// §4); false when no generator has that id.
    pub fn push_gate_trigger(&mut self, id: u64, trigger: GateTrigger) -> bool {
        match self.instances.iter_mut().find(|inst| inst.id == id) {
            Some(inst) => {
                inst.gate.push(trigger);
                true
            }
            None => false,
        }
    }

    /// Drive every generator's clock across `(start_beats, end_beats]`, invoking
    /// `tick_fn` once per boundary crossing and resolving each emitted event to an
    /// absolute sample time. Newly produced emissions are appended to `out` and
    /// sorted by `(sample_time, generator_index)` — the determinism contract shared
    /// with the neural runtime. Emissions already in `out` are left untouched.
    pub fn process_block<F>(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        tick_fn: F,
        out: &mut Vec<GeneratorEmission>,
    ) where
        F: FnMut(GeneratorTickInput) -> GeneratorTickResult,
    {
        let mut discarded_controls = Vec::new();
        self.process_block_with_controls(
            start_beats,
            end_beats,
            block_start_sample,
            samples_per_quarter,
            tick_fn,
            out,
            &mut discarded_controls,
        );
    }

    /// [`Self::process_block`] plus mixer-control resolution: control holds
    /// emitted by ticks land in `control_out` with absolute engage/release
    /// samples (docs/jaki-mixer-control-routes-spec.md).
    #[allow(clippy::too_many_arguments)]
    pub fn process_block_with_controls<F>(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        tick_fn: F,
        out: &mut Vec<GeneratorEmission>,
        control_out: &mut Vec<MixerControlEmission>,
    ) where
        F: FnMut(GeneratorTickInput) -> GeneratorTickResult,
    {
        self.process_block_selected(
            start_beats,
            end_beats,
            block_start_sample,
            samples_per_quarter,
            |_| true,
            tick_fn,
            out,
            control_out,
        );
    }

    /// [`Self::process_block_with_controls`] over only the generators whose id
    /// `include` accepts. The scheduler runs generators that graph nodes gate in
    /// a second pass, after the graph stage has queued its triggers
    /// (docs/jaki-trig-modes-spec.md §4.1).
    #[allow(clippy::too_many_arguments)]
    pub fn process_block_selected<I, F>(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
        include: I,
        mut tick_fn: F,
        out: &mut Vec<GeneratorEmission>,
        control_out: &mut Vec<MixerControlEmission>,
    ) where
        I: Fn(u64) -> bool,
        F: FnMut(GeneratorTickInput) -> GeneratorTickResult,
    {
        if self.instances.is_empty() || end_beats <= start_beats {
            return;
        }
        let appended_from = out.len();
        let controls_appended_from = control_out.len();
        for generator_index in 0..self.instances.len() {
            let id = self.instances[generator_index].id;
            if !include(id) {
                continue;
            }
            let mut clock = self.instances[generator_index].clock;
            let mut tick_count = self.instances[generator_index].tick_count;
            let mut random_state = self.instances[generator_index].random_state;
            let mut state = std::mem::take(&mut self.instances[generator_index].state);
            let mut gate = std::mem::take(&mut self.instances[generator_index].gate);
            let resolution_beats = clock.resolution_beats;
            process_grid_boundaries(
                &mut clock,
                start_beats,
                end_beats,
                block_start_sample,
                samples_per_quarter,
                |beat, _grid_index, boundary_sample| {
                    let gate_view = gate.deliver(beat);
                    let result = tick_fn(GeneratorTickInput {
                        id,
                        generator_index,
                        tick_index: tick_count,
                        boundary_sample,
                        beat,
                        resolution_beats,
                        samples_per_quarter,
                        random_state,
                        state: std::mem::take(&mut state),
                        gate: gate_view,
                    });
                    random_state = result.random_state;
                    state = result.state;
                    for event in result.emitted {
                        let offset_samples = (event.offset_beats as f64 * samples_per_quarter)
                            .round()
                            .max(0.0) as u64;
                        out.push(GeneratorEmission {
                            sample_time: boundary_sample.saturating_add(offset_samples),
                            generator_index,
                            event,
                        });
                    }
                    for control in result.controls {
                        let offset_samples = (control.offset_beats as f64 * samples_per_quarter)
                            .round()
                            .max(0.0) as u64;
                        let duration_samples = (control.duration_beats as f64
                            * samples_per_quarter)
                            .round()
                            .max(0.0) as u64;
                        let engage_sample = boundary_sample.saturating_add(offset_samples);
                        control_out.push(MixerControlEmission {
                            engage_sample,
                            release_sample: engage_sample.saturating_add(duration_samples),
                            generator_index,
                            control,
                        });
                    }
                    tick_count = tick_count.saturating_add(1);
                },
            );
            self.instances[generator_index].clock = clock;
            self.instances[generator_index].tick_count = tick_count;
            self.instances[generator_index].random_state = random_state;
            self.instances[generator_index].state = state;
            self.instances[generator_index].gate = gate;
        }
        out[appended_from..]
            .sort_by_key(|emission| (emission.sample_time, emission.generator_index));
        control_out[controls_appended_from..]
            .sort_by_key(|emission| (emission.engage_sample, emission.generator_index));
    }

    #[cfg(test)]
    fn tick_count(&self, id: u64) -> Option<u64> {
        self.instances
            .iter()
            .find(|inst| inst.id == id)
            .map(|inst| inst.tick_count)
    }
}

fn generator_random_seed(id: u64) -> u64 {
    let seed = splitmix64(id ^ GENERATOR_DEFAULT_RANDOM_STATE);
    if seed == 0 {
        GENERATOR_DEFAULT_RANDOM_STATE
    } else {
        seed
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::Timebase;

    fn def(id: u64, timebase: Timebase) -> GeneratorDef {
        GeneratorDef {
            id,
            name: format!("gen-{id}"),
            resolution_beats: timebase.step_beats(GENERATOR_RESOLUTION_REF_STEPS),
        }
    }

    fn emit_one() -> GeneratorTickResult {
        GeneratorTickResult {
            emitted: vec![EmittedAccumulatorEvent {
                origin_note: None,
                offset_beats: 0.0,
                track: Some(0),
                resolved: default_resolved(),
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            }],
            controls: Vec::new(),
            random_state: 0,
            state: HashMap::new(),
        }
    }

    #[test]
    fn generator_emits_each_resolution_tick_over_a_bar() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Sixteenth)], 0.0);

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| {
                let mut result = emit_one();
                result.random_state = input.random_state;
                result
            },
            &mut out,
        );

        assert_eq!(out.len(), 16);
        // First boundary at 0.25 beats -> 12000 samples; last at 4.0 -> 192000.
        assert_eq!(out[0].sample_time, 12_000);
        assert_eq!(out[15].sample_time, 192_000);
        assert!(out.iter().all(|e| e.generator_index == 0));
    }

    #[test]
    fn tick_index_is_zero_based_and_monotonic() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Quarter)], 0.0);

        let mut seen = Vec::new();
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| {
                seen.push(input.tick_index);
                GeneratorTickResult {
                    emitted: Vec::new(),
                    controls: Vec::new(),
                    random_state: input.random_state,
                    state: input.state,
                }
            },
            &mut out,
        );

        assert_eq!(seen, vec![0, 1, 2, 3]);
        assert_eq!(runtime.tick_count(1), Some(4));
    }

    #[test]
    fn coincident_emissions_order_by_generator_index() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(
            &[def(10, Timebase::Quarter), def(20, Timebase::Quarter)],
            0.0,
        );

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            |input| {
                let mut result = emit_one();
                result.random_state = input.random_state;
                result
            },
            &mut out,
        );

        // Both fire at beat 1.0 (sample 48000); ordering is by generator index.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].sample_time, 48_000);
        assert_eq!(out[1].sample_time, 48_000);
        assert_eq!(out[0].generator_index, 0);
        assert_eq!(out[1].generator_index, 1);
    }

    #[test]
    fn boundary_relative_offset_resolves_to_later_sample() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Quarter)], 0.0);

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            |input| {
                let mut event = emit_one().emitted.remove(0);
                // emit a sixteenth (0.25 beat) after the boundary
                event.offset_beats = 0.25;
                GeneratorTickResult {
                    emitted: vec![event],
                    controls: Vec::new(),
                    random_state: input.random_state,
                    state: input.state,
                }
            },
            &mut out,
        );

        // boundary at beat 1.0 (48000) + 0.25 beat (12000) = 60000
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].sample_time, 60_000);
    }

    #[test]
    fn sync_definitions_preserves_state_on_compatible_reload() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Quarter)], 0.0);

        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            2.0,
            0,
            48_000.0,
            |input| GeneratorTickResult {
                emitted: Vec::new(),
                controls: Vec::new(),
                random_state: input.random_state,
                state: input.state,
            },
            &mut out,
        );
        assert_eq!(runtime.tick_count(1), Some(2));

        // Same id + resolution: preserve the tick counter.
        runtime.sync_definitions(&[def(1, Timebase::Quarter)], 2.0);
        assert_eq!(runtime.tick_count(1), Some(2));

        // Resolution change: reset.
        runtime.sync_definitions(&[def(1, Timebase::Sixteenth)], 2.0);
        assert_eq!(runtime.tick_count(1), Some(0));
    }

    #[test]
    fn empty_runtime_emits_nothing() {
        let mut runtime = GeneratorRuntime::default();
        let mut out = Vec::new();
        runtime.process_block(
            0.0,
            4.0,
            0,
            48_000.0,
            |input| GeneratorTickResult {
                emitted: vec![],
                controls: Vec::new(),
                random_state: input.random_state,
                state: input.state,
            },
            &mut out,
        );
        assert!(out.is_empty());
    }

    fn play(beat: f64, duration_beats: f64, note: f32, velocity: f32) -> GateTrigger {
        GateTrigger {
            kind: GateTriggerKind::Play,
            beat,
            duration_beats,
            note,
            velocity,
        }
    }

    /// Run one generator over `(start, end]` and record (beat, gate) per tick.
    fn gates(runtime: &mut GeneratorRuntime, start: f64, end: f64) -> Vec<(f64, GeneratorGateView)> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        runtime.process_block(
            start,
            end,
            0,
            48_000.0,
            |input| {
                seen.push((input.beat, input.gate));
                GeneratorTickResult {
                    emitted: Vec::new(),
                    controls: Vec::new(),
                    random_state: input.random_state,
                    state: input.state,
                }
            },
            &mut out,
        );
        seen
    }

    #[test]
    fn gate_opens_at_the_fire_boundary_for_exactly_its_duration() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Sixteenth)], 0.0);
        // Three sixteenths from beat 1.0.
        assert!(runtime.push_gate_trigger(1, play(1.0, 0.75, 5.0, 0.5)));
        let seen = gates(&mut runtime, 0.0, 2.0);
        let open: Vec<f64> = seen.iter().filter(|(_, g)| g.open).map(|(b, _)| *b).collect();
        assert_eq!(open, vec![1.0, 1.25, 1.5]);
        let at_fire = seen.iter().find(|(b, _)| *b == 1.0).unwrap().1;
        assert_eq!((at_fire.epoch, at_fire.note, at_fire.velocity), (1, 5.0, 0.5));
        let before = seen.iter().find(|(b, _)| *b == 0.75).unwrap().1;
        assert_eq!(before, GeneratorGateView::default(), "nothing delivered early");
        assert!(!runtime.push_gate_trigger(99, play(0.0, 1.0, 0.0, 1.0)), "unknown id");
    }

    #[test]
    fn coincident_fires_bump_once_extend_the_window_and_the_loudest_sets_the_payload() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Sixteenth)], 0.0);
        runtime.push_gate_trigger(1, play(0.5, 0.25, 7.0, 0.4));
        runtime.push_gate_trigger(1, play(0.5, 0.5, -3.0, 0.9));
        // A later, quieter fire inside the window: newest wins, window keeps its end.
        runtime.push_gate_trigger(1, play(0.75, 0.0, 2.0, 0.1));
        let seen = gates(&mut runtime, 0.0, 1.5);
        let at = |beat: f64| seen.iter().find(|(b, _)| *b == beat).unwrap().1;
        assert_eq!((at(0.5).epoch, at(0.5).note, at(0.5).velocity), (1, -3.0, 0.9));
        assert!(at(0.75).open && !at(1.0).open, "window ends at 0.5 + 0.5");
        assert_eq!((at(0.75).epoch, at(0.75).note, at(0.75).velocity), (2, 2.0, 0.1));
    }

    #[test]
    fn restart_triggers_bump_only_the_restart_epoch() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Quarter)], 0.0);
        runtime.push_gate_trigger(
            1,
            GateTrigger { kind: GateTriggerKind::Restart, ..play(2.0, 4.0, 3.0, 0.2) },
        );
        let seen = gates(&mut runtime, 0.0, 3.0);
        let at2 = seen.iter().find(|(b, _)| *b == 2.0).unwrap().1;
        assert_eq!(at2, GeneratorGateView { restart_epoch: 1, ..GeneratorGateView::default() });
    }

    #[test]
    fn selected_pass_skips_excluded_generators_and_reset_clears_the_gate() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Quarter), def(2, Timebase::Quarter)], 0.0);
        runtime.push_gate_trigger(2, play(0.0, 8.0, 0.0, 1.0));
        let mut ids = Vec::new();
        runtime.process_block_selected(
            0.0,
            1.0,
            0,
            48_000.0,
            |id| id == 2,
            |input| {
                ids.push(input.id);
                GeneratorTickResult {
                    emitted: Vec::new(),
                    controls: Vec::new(),
                    random_state: input.random_state,
                    state: input.state,
                }
            },
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(ids, vec![2]);
        assert_eq!(runtime.tick_count(1), Some(0), "the excluded generator did not advance");
        runtime.reset(0.0);
        let seen = gates(&mut runtime, 0.0, 1.0);
        assert!(seen.iter().all(|(_, g)| *g == GeneratorGateView::default()));
    }
}
