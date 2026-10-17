//! Run a sequencer instance's tick headlessly for a few bars and return what
//! it played, as data (bead eseq-maxc.3): the check an agent needs after
//! writing a tick, which `eseq sequencer check` and a panel capture cannot
//! give it.
//!
//! The run is the scheduler's own path, minus audio: the instance's kind
//! publishes its tick (`instance_published_sequencer`), a scheduler-built
//! Lisp runtime loads it with its `:requires`, a copy of the selected
//! scene's slots stands in for the document `self` reads, a fresh
//! `GeneratorRuntime` drives the boundaries, and the hits go through the
//! scheduler's accent merge and rack member routing. The instance's RNG,
//! `state-get` cells and clock start fresh, as on Play from the top.
//!
//! Nothing live is touched: the runtime is bound to a throwaway
//! `SequencerState` carrying the project's descriptors and rack
//! memberships, so whatever a tick writes (`gen-mark`, step edits) lands
//! there. The price: a tick that reads other tracks' patterns sees an empty
//! project. Also not modelled (the caller is told): processes downstream of
//! the generator, groove and swing, `chan-get` values and track harmony.

use std::collections::HashMap;
use std::sync::Arc;

use crate::generator::{GeneratorRuntime, GeneratorTickResult};
use crate::lisp_host;
use crate::sequencer::{SequencerState, Timebase};

/// Any positive rate works: results are reported in beats.
const SAMPLES_PER_QUARTER: f64 = 24_000.0;
/// The scheduler resets generators to beat 0 on Play and at a loop wrap;
/// `gen-tick` 0 falls on that downbeat.
const START_BEATS: f64 = 0.0;
pub const MAX_BARS: u32 = 64;
/// Hit lines a report lists; the summary still counts them all.
const MAX_REPORTED_HITS: usize = 512;

/// The tick that threw: the run stops there, as the scheduler parks a
/// failing generator.
#[derive(Debug, Clone, PartialEq)]
pub struct TickFailure {
    pub tick: u64,
    pub beat: f64,
    pub error: String,
}

/// One emitted hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedHit {
    /// Quarter-note beats from the start.
    pub beat: f64,
    pub track: usize,
    /// Semitones from the track's base note (a drum pad's note offset).
    pub transpose: f32,
    pub velocity: f32,
    /// Beats.
    pub duration: f32,
    pub chord: Vec<f32>,
    /// How many per-hit parameter locks it carried.
    pub params: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Simulation {
    /// Bars run, after clamping to 1..=[`MAX_BARS`].
    pub bars: u32,
    pub ticks: u64,
    pub hits: Vec<SimulatedHit>,
    pub failure: Option<TickFailure>,
}

/// Run instance `instance` of kind `kind_id` (owned by rack `owner_rack`,
/// if any) for `bars` bars of 4/4.
pub fn simulate_instance(
    state: &Arc<SequencerState>,
    kind_id: &str,
    instance: u64,
    owner_rack: Option<u64>,
    bars: u32,
) -> Result<Simulation, String> {
    let bars = bars.clamp(1, MAX_BARS);
    let kind = lisp_host::registered_kind(kind_id)
        .ok_or_else(|| format!("no kind `{kind_id}` is loaded; import its module first"))?;
    let published = lisp_host::instance_published_sequencer(&kind, instance, owner_rack).ok_or_else(|| {
        format!("kind `{kind_id}` has no :generator tick to simulate (graph sequencers are not supported)")
    })?;
    // A throwaway state with the project's descriptors and racks, so nothing
    // the tick writes reaches the live one.
    let tracks = state.active_track_count().max(1);
    let sandbox = Arc::new(SequencerState::new(
        tracks,
        (0..tracks).map(|_| crate::sequencer::default_empty_effect_chain()).collect(),
    ));
    let (effects, instruments) = state.scratch_runtime_descriptors();
    sandbox.set_scratch_runtime_descriptors(effects, instruments);
    sandbox.set_rack_memberships(state.rack_memberships());
    // Built as the scheduler builds its own: descriptors, the builtin
    // MIDI-FX and process libraries, no process-chain writes.
    let mut scratch = crate::scheduler::build_scheduler_scratch_runtime(Arc::clone(&sandbox), "", false)
        .0
        .unwrap_or_else(|| lisp_host::scheduler_scratch_runtime_with_fallbacks(Arc::clone(&sandbox), 0, 0));
    scratch.register_published_sequencer(
        published.id,
        published.name.clone(),
        Timebase::from_index(published.resolution as u32),
        published.tick_source.clone(),
        &published.requires,
    )?;
    scratch.set_scene_slot_snapshot(Arc::new(state.current_scene_slots()));
    // A rack-owned tick's `:track` is a pad; the scheduler maps it to the
    // member track.
    let rack_members: Option<Vec<usize>> = owner_rack.map(|group| {
        crate::graph::rack_members(&state.rack_memberships(), group).unwrap_or(&[]).to_vec()
    });

    let mut generators = GeneratorRuntime::default();
    generators.sync_definitions(&scratch.sequencer_defs(), START_BEATS);
    let mut emissions = Vec::new();
    let mut ticks = 0;
    let mut failure = None;
    generators.process_block(
        START_BEATS,
        START_BEATS + f64::from(bars) * 4.0,
        0,
        SAMPLES_PER_QUARTER,
        |input| {
            // After a failure the run is over; what a skipped tick returns
            // no longer matters.
            let skipped = |random_state| GeneratorTickResult {
                emitted: Vec::new(),
                controls: Vec::new(),
                random_state,
                state: HashMap::new(),
            };
            if failure.is_some() {
                return skipped(input.random_state);
            }
            ticks += 1;
            let (index, beat, random_state) = (input.tick_index, input.beat, input.random_state);
            match scratch.invoke_sequencer_tick(input.generator_index, input) {
                Ok(mut result) => {
                    if let Some(members) = &rack_members {
                        crate::scheduler::map_rack_member_emissions(&mut result.emitted, members);
                    }
                    result
                }
                Err(error) => {
                    failure = Some(TickFailure { tick: index, beat: beat.max(0.0), error });
                    skipped(random_state)
                }
            }
        },
        &mut emissions,
    );
    // Coincident hits merge before they play, as in the scheduler.
    let hits = crate::scheduler::merge_generator_emission_accents(emissions)
        .into_iter()
        .map(|emission| {
            let event = emission.event;
            let beat = emission.sample_time as f64 / SAMPLES_PER_QUARTER;
            SimulatedHit {
                beat: (beat * 1e6).round() / 1e6,
                // A generator hit with no track plays on track 0.
                track: event.track.unwrap_or(0),
                transpose: event.resolved.transpose,
                velocity: event.resolved.velocity,
                duration: event.resolved.duration,
                chord: event.chord,
                params: event.named_params.len() + event.effect_params.len() + event.instrument_params.len(),
            }
        })
        .collect();
    Ok(Simulation { bars, ticks, hits, failure })
}

/// `bar.sixteenth` (1-based) for a beat, with the remainder when it is off
/// the sixteenth grid: `1.1`, `2.3`, `1.2+0.33`.
pub fn position(beat: f64) -> String {
    let sixteenths = beat * 4.0;
    let whole = (sixteenths + 1e-6).floor();
    let bar = (whole / 16.0).floor() as u64 + 1;
    let step = (whole as u64 % 16) + 1;
    let rest = sixteenths - whole;
    if rest > 1e-3 {
        format!("{bar}.{step}+{rest:.2}")
    } else {
        format!("{bar}.{step}")
    }
}

impl Simulation {
    /// One line per hit and a summary: what the agent reads.
    pub fn report(&self) -> String {
        let mut lines = vec![format!(
            "{} tick(s) over {} bar(s), {} hit(s). Position is bar.sixteenth; gen-tick 0 is 1.1.",
            self.ticks,
            self.bars,
            self.hits.len()
        )];
        for hit in self.hits.iter().take(MAX_REPORTED_HITS) {
            let track = format!("track {}", hit.track);
            let mut line = format!(
                "{:>8}  {track:<9} {:+} st  vel {:.2}  dur {:.3}",
                position(hit.beat),
                hit.transpose,
                hit.velocity,
                hit.duration
            );
            if !hit.chord.is_empty() {
                line.push_str(&format!("  chord {:?}", hit.chord));
            }
            if hit.params > 0 {
                line.push_str(&format!("  {} p-lock(s)", hit.params));
            }
            lines.push(line);
        }
        if self.hits.len() > MAX_REPORTED_HITS {
            lines.push(format!("… {} more hit(s) not listed; simulate fewer bars", self.hits.len() - MAX_REPORTED_HITS));
        }
        if let Some(failure) = &self.failure {
            lines.push(format!(
                "Tick {} at {} threw, so the run stopped: {}",
                failure.tick,
                position(failure.beat),
                failure.error
            ));
        }
        lines.push(
            "Not modelled: processes after the generator, groove/swing, chan-get values, track harmony; \
             reads of other tracks' patterns see an empty project."
                .to_string(),
        );
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_bars_and_sixteenths_from_one() {
        assert_eq!(position(0.0), "1.1");
        assert_eq!(position(0.75), "1.4");
        assert_eq!(position(4.0), "2.1");
        assert_eq!(position(5.25), "2.6");
        assert_eq!(position(0.33), "1.2+0.32");
    }

    fn state() -> Arc<SequencerState> {
        Arc::new(SequencerState::new(
            2,
            (0..2).map(|_| crate::sequencer::default_empty_effect_chain()).collect(),
        ))
    }

    /// A kind whose tick fires on the document's `cells` (cycled per tick),
    /// registered as `test/sim:<name>`, with instance 7's cells set.
    fn kind_with_tick(name: &str, tick: &str, cells: &str, state: &SequencerState) -> String {
        let mut values = eseqlisp::Runtime::new();
        let mut literal = |source: &str| {
            let value = values.eval_str(source).expect("literal").expect("a value");
            crate::process::ProcessLiteral::from_value(&value).expect("portable")
        };
        let id = format!("test/sim:{name}");
        lisp_host::register_kind(lisp_host::KindDefinition {
            id: id.clone(),
            name: name.to_string(),
            package: Some("test/sim".to_string()),
            module: None,
            sequencer: None,
            generator: Some(crate::sequencer::PublishedSequencer {
                id: 0,
                name: name.to_string(),
                resolution: Timebase::Sixteenth as u8,
                tick_source: tick.to_string(),
                requires: Vec::new(),
                graph: None,
                owner_rack: None,
            }),
            document: vec![("cells".to_string(), literal("(list 0)"))],
            state_fields: Vec::new(),
            has_view: false,
            keymap: None,
        });
        state
            .write_current_scene_slot(lisp_host::instance_document_slot(7, "cells"), literal(cells))
            .expect("cells");
        id
    }

    #[test]
    fn a_tick_plays_the_instance_document_on_the_engines_boundaries() {
        let state = state();
        let kind = kind_with_tick(
            "cells",
            "(if (= (nth self.cells (mod (gen-tick) (len self.cells))) 1) \
               (seq-emit :track 1 :at :now :note 3 :vel 0.5 :dur 0.25) nil)",
            "(list 1 0 0 1)",
            &state,
        );
        let run = simulate_instance(&state, &kind, 7, None, 1).expect("simulates");
        assert_eq!(run.failure, None);
        assert_eq!(run.ticks, 16);
        let positions: Vec<String> = run.hits.iter().map(|hit| position(hit.beat)).collect();
        // Ticks 0, 3, 4, 7, …: tick k on the k-th sixteenth.
        assert_eq!(positions, ["1.1", "1.4", "1.5", "1.8", "1.9", "1.12", "1.13", "1.16"]);
        let first = &run.hits[0];
        assert_eq!((first.track, first.transpose, first.velocity, first.duration), (1, 3.0, 0.5, 0.25));
        assert!(run.report().contains("1.4  track 1   +3 st  vel 0.50  dur 0.250"), "{}", run.report());
    }

    #[test]
    fn a_tick_that_throws_stops_the_run_and_says_where() {
        let state = state();
        let kind = kind_with_tick(
            "broken",
            "(if (> (gen-tick) 1) (no-such-function) (seq-emit :track 0 :at :now :note 0 :vel 1 :dur 0.25))",
            "(list 1)",
            &state,
        );
        let run = simulate_instance(&state, &kind, 7, None, 2).expect("simulates");
        assert_eq!(run.hits.len(), 2, "ticks 0 and 1 played");
        let failure = run.failure.clone().expect("a failure");
        assert_eq!((failure.tick, position(failure.beat)), (2, "1.3".to_string()));
        assert!(failure.error.contains("no-such-function"), "{}", failure.error);
        assert!(run.report().contains("Tick 2 at 1.3 threw"), "{}", run.report());
    }

    #[test]
    fn coincident_hits_merge_and_a_tick_writes_nothing_live() {
        let state = state();
        let kind = kind_with_tick(
            "merged",
            "(do (gen-mark 1) \
               (seq-emit :track 0 :at :now :note 0 :vel 0.4 :dur 0.25) \
               (seq-emit :track 0 :at :now :note 0 :vel 0.4 :dur 0.25))",
            "(list 1)",
            &state,
        );
        let run = simulate_instance(&state, &kind, 7, None, 1).expect("simulates");
        assert_eq!(run.hits.len(), 16, "two hits per tick merge into one");
        assert!((run.hits[0].velocity - 0.8).abs() < 1e-6, "{:?}", run.hits[0]);
        assert!(state.generator_mark_keys().is_empty(), "the live state saw no gen-mark");
    }

    #[test]
    fn a_kind_without_a_generator_or_registration_is_an_error() {
        let state = state();
        let error = simulate_instance(&state, "nobody/missing:kind", 1, None, 1).unwrap_err();
        assert!(error.contains("no kind `nobody/missing:kind`"), "{error}");
    }
}
