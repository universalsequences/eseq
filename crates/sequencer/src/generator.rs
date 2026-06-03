//! Lisp-defined generator runtime.
//!
//! A *generator* is the open, lisp-authored counterpart to the native neural
//! sequencer: a self-clocked unit that the scheduler ticks on its own timebase
//! grid each block (even with no incoming events) and that emits musical events.
//!
//! This module owns only the timing/ordering concerns — one [`GridBoundaryClock`]
//! per generator, a per-generator tick counter, and a per-generator RNG seed — and
//! is deliberately lisp-agnostic so it can be driven by a plain Rust callback in
//! tests. The actual lisp `:tick` closure invocation lives in `lisp_effect.rs`; the
//! scheduler wires the two together by passing a `tick_fn` that calls into the
//! scheduler-side lisp VM.
//!
//! Emission timing is sample-accurate but expressed by the generator in *musical*
//! coordinates (boundary-relative `offset_beats` on [`EmittedAccumulatorEvent`]);
//! this runtime resolves those to absolute sample times the same way the neural
//! runtime and the accumulator emit path do.

use std::collections::HashMap;

use crate::accumulator::ResolvedStep;
use crate::lisp_effect::EmittedAccumulatorEvent;
use crate::neural::{process_grid_boundaries, GridBoundaryClock};

/// Reference subdivision count used when converting a `Timebase` to beats. Only
/// affects `Timebase::Polyrhythm`; matches the neural runtime's convention so the
/// two clocks agree.
pub const GENERATOR_RESOLUTION_REF_STEPS: usize = 16;

const GENERATOR_DEFAULT_RANDOM_STATE: u64 = 0xA076_1D64_78BD_642F;

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
    /// Musical position of this boundary in quarter-note beats (`gen-beat`).
    pub beat: f64,
    pub resolution_beats: f64,
    pub samples_per_quarter: f64,
    /// RNG state for `gen-rand`; the callback returns the advanced state.
    pub random_state: u64,
    /// Persistent per-generator scalar state cells (`state-get`/`state-set!`),
    /// carried in and returned (possibly mutated) by the callback.
    pub state: HashMap<String, f64>,
}

/// Result of a generator tick: emitted events (boundary-relative `offset_beats`)
/// plus the advanced RNG state and persistent state cells.
#[derive(Clone, Debug, Default)]
pub struct GeneratorTickResult {
    pub emitted: Vec<EmittedAccumulatorEvent>,
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

#[derive(Clone, Debug)]
struct GeneratorInstance {
    id: u64,
    name: String,
    clock: GridBoundaryClock,
    tick_count: u64,
    random_state: u64,
    state: HashMap<String, f64>,
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
        mut tick_fn: F,
        out: &mut Vec<GeneratorEmission>,
    ) where
        F: FnMut(GeneratorTickInput) -> GeneratorTickResult,
    {
        if self.instances.is_empty() || end_beats <= start_beats {
            return;
        }
        let appended_from = out.len();
        for generator_index in 0..self.instances.len() {
            let id = self.instances[generator_index].id;
            let mut clock = self.instances[generator_index].clock;
            let mut tick_count = self.instances[generator_index].tick_count;
            let mut random_state = self.instances[generator_index].random_state;
            let mut state = std::mem::take(&mut self.instances[generator_index].state);
            let resolution_beats = clock.resolution_beats;
            process_grid_boundaries(
                &mut clock,
                start_beats,
                end_beats,
                block_start_sample,
                samples_per_quarter,
                |beat, _grid_index, boundary_sample| {
                    let result = tick_fn(GeneratorTickInput {
                        id,
                        generator_index,
                        tick_index: tick_count,
                        beat,
                        resolution_beats,
                        samples_per_quarter,
                        random_state,
                        state: std::mem::take(&mut state),
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
                    tick_count = tick_count.saturating_add(1);
                },
            );
            self.instances[generator_index].clock = clock;
            self.instances[generator_index].tick_count = tick_count;
            self.instances[generator_index].random_state = random_state;
            self.instances[generator_index].state = state;
        }
        out[appended_from..].sort_by_key(|emission| (emission.sample_time, emission.generator_index));
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
                offset_beats: 0.0,
                track: Some(0),
                resolved: default_resolved(),
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            }],
            random_state: 0,
            state: HashMap::new(),
        }
    }

    #[test]
    fn generator_emits_each_resolution_tick_over_a_bar() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Sixteenth)], 0.0);

        let mut out = Vec::new();
        runtime.process_block(0.0, 4.0, 0, 48_000.0, |input| {
            let mut result = emit_one();
            result.random_state = input.random_state;
            result
        }, &mut out);

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
        runtime.process_block(0.0, 4.0, 0, 48_000.0, |input| {
            seen.push(input.tick_index);
            GeneratorTickResult { emitted: Vec::new(), random_state: input.random_state, state: input.state }
        }, &mut out);

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
        runtime.process_block(0.0, 1.0, 0, 48_000.0, |input| {
            let mut result = emit_one();
            result.random_state = input.random_state;
            result
        }, &mut out);

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
        runtime.process_block(0.0, 1.0, 0, 48_000.0, |input| {
            let mut event = emit_one().emitted.remove(0);
            // emit a sixteenth (0.25 beat) after the boundary
            event.offset_beats = 0.25;
            GeneratorTickResult { emitted: vec![event], random_state: input.random_state, state: input.state }
        }, &mut out);

        // boundary at beat 1.0 (48000) + 0.25 beat (12000) = 60000
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].sample_time, 60_000);
    }

    #[test]
    fn sync_definitions_preserves_state_on_compatible_reload() {
        let mut runtime = GeneratorRuntime::default();
        runtime.sync_definitions(&[def(1, Timebase::Quarter)], 0.0);

        let mut out = Vec::new();
        runtime.process_block(0.0, 2.0, 0, 48_000.0, |input| {
            GeneratorTickResult { emitted: Vec::new(), random_state: input.random_state, state: input.state }
        }, &mut out);
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
        runtime.process_block(0.0, 4.0, 0, 48_000.0, |input| {
            GeneratorTickResult { emitted: vec![], random_state: input.random_state, state: input.state }
        }, &mut out);
        assert!(out.is_empty());
    }
}
