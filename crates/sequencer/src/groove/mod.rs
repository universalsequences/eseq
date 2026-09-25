//! Rack grooves (docs/rack-groove-spec.md, bead eseq-groove.1): a timing and
//! accent map extracted from a played rack pattern and owned by the drum rack,
//! so every trig source aimed at the rack's pads can later play through it.
//!
//! This module is the data model plus the PURE extraction half:
//!
//! - [`heard_hits`] reads one member pattern and returns where each hit was
//!   actually heard (step start + step Delay or per-note chord delay + the
//!   pattern's own swing), in the pattern's cycle beats.
//! - [`extract_groove`] snaps those hits to the nearest groove slot, folds them
//!   modulo the period, aggregates per pad (median offset, MAD spread,
//!   velocity accent vs the pad median, louder-wins collisions), builds the
//!   pooled all-pads row and fills every unmeasured slot.
//! - [`quantize_groove_source`] straightens the source pattern so it sounds
//!   the same through the groove as it did before extraction.
//!
//! Application (eseq-groove.2) lives in [`apply`]: the scheduler's
//! pre-resolved per-track table, the shared timing function and the built-in
//! MPC swing grooves. Moving grooves between racks (kit presets, another
//! rack's groove) lives in [`transfer`] (eseq-groove.7).

use serde::{Deserialize, Serialize};

use crate::effects::EffectSlotSnapshot;
use crate::sequencer::{
    bar_of_step, StepParam, SwingResolution, Timebase, TrackPatternData, MAX_STEPS,
};

mod apply;
#[cfg(test)]
mod tests;
mod transfer;

pub use apply::{
    builtin_groove, builtin_grooves, groove_hash_noise, GrooveFloor, groove_offset_samples, grooved_sample_time,
    max_early_lead_beats, mpc_swing_groove, padless_seed_key, track_groove_snapshots,
    BuiltinGroove, TrackGrooveSnapshot, BUILTIN_MPC_SWING_PERCENTS, MAX_EARLY_SLOTS,
};
pub use transfer::{import_grooves, install_groove_settings, GrooveRowChoice};

/// Stable identity of one groove within its rack's list.
pub type GrooveId = u64;

pub const GROOVE_TIMING_AMOUNT_MAX: f32 = 1.5;
pub const GROOVE_VELOCITY_AMOUNT_MAX: f32 = 1.5;
pub const GROOVE_RANDOM_AMOUNT_MAX: f32 = 1.0;
/// Upper bound on `period / resolution`: two bars of 32nds is 64 slots, so
/// this only rejects nonsense grids from a hand-edited file.
pub const GROOVE_MAX_SLOTS: usize = 256;

/// One bar of 4/4 and two bars, the periods the Extract Groove modal offers.
pub const GROOVE_PERIOD_ONE_BAR: f64 = 4.0;
pub const GROOVE_PERIOD_TWO_BARS: f64 = 8.0;
pub const GROOVE_RESOLUTION_SIXTEENTH: f64 = 0.25;
pub const GROOVE_RESOLUTION_THIRTY_SECOND: f64 = 0.125;

const EPS: f64 = 1e-9;

/// One extracted or built-in feel. Positions are in BEATS within the period,
/// never in steps: member tracks can have different timebases, and graph
/// sequencers fire at their own `:resolution`/`:quantize`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectGroove {
    pub id: GrooveId,
    pub name: String,
    /// 4.0 = one bar of 4/4, 8.0 = two bars.
    pub period_beats: f64,
    /// Slot spacing; 0.25 = 16ths.
    pub resolution_beats: f64,
    /// Per-pad rows keyed by `pad_note` (stable across member reorder), not
    /// by member index. Only pads that were heard at least once have a row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pad_rows: Vec<GroovePadRow>,
    /// All-pads row: every pad's hits pooled. The fallback for pads without a
    /// row and the only row a generic groove has.
    pub shared_row: GrooveRow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroovePadRow {
    pub pad_note: i32,
    pub row: GrooveRow,
}

/// `slots.len() == round(period_beats / resolution_beats)`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GrooveRow {
    pub slots: Vec<GrooveSlot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrooveSlot {
    /// Offset from the slot's straight position, in units of
    /// `resolution_beats`. Signed: negative = early. Extraction keeps it in
    /// [-0.5, 0.5).
    pub offset: f32,
    /// Accent relative to the row's median velocity (1.0 = neutral).
    #[serde(default = "neutral_velocity_scale")]
    pub velocity_scale: f32,
    /// Robust spread of `offset` across repeats (median absolute deviation),
    /// for the Random amount.
    #[serde(default)]
    pub spread: f32,
    /// How the slot got its value; unmeasured slots are filled, not zero.
    #[serde(default)]
    pub source: GrooveSlotSource,
}

fn neutral_velocity_scale() -> f32 {
    1.0
}

impl Default for GrooveSlot {
    fn default() -> Self {
        Self {
            offset: 0.0,
            velocity_scale: 1.0,
            spread: 0.0,
            source: GrooveSlotSource::Zero,
        }
    }
}

/// Which extraction rule produced a slot, so the UI can dim guessed cells.
/// The fill rules are tried in declaration order after `Measured`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrooveSlotSource {
    /// At least one hit landed in this slot.
    Measured,
    /// Fill rule 1: the same pad's measured slot at the same position in the
    /// other half of a two-bar period.
    FilledFromOtherHalf,
    /// Fill rule 2: interpolated between the row's nearest measured slots of
    /// the same metric class (on-beat, &, e/a).
    FilledFromNeighbors,
    /// Fill rule 3: the shared all-pads row's value at this slot.
    FilledFromShared,
    /// Fill rule 4: nothing to go on; straight, neutral accent.
    #[default]
    Zero,
}

impl GrooveSlotSource {
    pub fn is_measured(self) -> bool {
        self == Self::Measured
    }
}

/// Which groove a rack plays through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrooveRef {
    /// One of the rack's own grooves, by id.
    Rack(GrooveId),
    /// A generic groove shipped with the app (MPC swings), by its stable id.
    Builtin(String),
}

impl GrooveRef {
    /// The UI's stable picker key: `rack:<id>` or `builtin:<id>`.
    pub fn picker_key(&self) -> String {
        match self {
            Self::Rack(id) => format!("rack:{id}"),
            Self::Builtin(id) => format!("builtin:{id}"),
        }
    }

    /// Parses a picker key back; `off` (or an empty key) is `Ok(None)`.
    pub fn from_picker_key(key: &str) -> Result<Option<Self>, String> {
        let key = key.trim();
        if key.is_empty() || key == "off" {
            return Ok(None);
        }
        if let Some(id) = key.strip_prefix("rack:") {
            return id
                .parse::<GrooveId>()
                .map(|id| Some(Self::Rack(id)))
                .map_err(|_| format!("Bad rack groove key {key:?}"));
        }
        if let Some(id) = key.strip_prefix("builtin:") {
            if apply::builtin_groove(id).is_none() {
                return Err(format!("Unknown built-in groove {id:?}"));
            }
            return Ok(Some(Self::Builtin(id.to_string())));
        }
        Err(format!("Bad groove key {key:?}"))
    }
}

/// The rack's groove selection and amounts. Every field defaults, so racks
/// saved before grooves existed load with no active groove.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RackGrooveSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<GrooveRef>,
    /// 0..1.5, default 1.0.
    #[serde(default = "default_timing_amount")]
    pub timing_amount: f32,
    /// 0..1.5, default 0.0 (timing-only by default).
    #[serde(default)]
    pub velocity_amount: f32,
    /// 0..1.0, default 0.0.
    #[serde(default)]
    pub random_amount: f32,
}

fn default_timing_amount() -> f32 {
    1.0
}

impl Default for RackGrooveSettings {
    fn default() -> Self {
        Self {
            active: None,
            timing_amount: default_timing_amount(),
            velocity_amount: 0.0,
            random_amount: 0.0,
        }
    }
}

impl RackGrooveSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Clamps the amounts into their documented ranges (a non-finite value
    /// takes the default), so a hand-edited file cannot drive the scheduler
    /// out of its early-hit bound.
    pub fn sanitize(&mut self) {
        fn clamp(value: f32, max: f32, default: f32) -> f32 {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                default
            }
        }
        self.timing_amount = clamp(
            self.timing_amount,
            GROOVE_TIMING_AMOUNT_MAX,
            default_timing_amount(),
        );
        self.velocity_amount = clamp(self.velocity_amount, GROOVE_VELOCITY_AMOUNT_MAX, 0.0);
        self.random_amount = clamp(self.random_amount, GROOVE_RANDOM_AMOUNT_MAX, 0.0);
    }
}

/// Slot count of a `period / resolution` grid, or `None` when the grid is not
/// a whole, bounded number of positive slots.
pub fn groove_slot_count(period_beats: f64, resolution_beats: f64) -> Option<usize> {
    if !period_beats.is_finite()
        || !resolution_beats.is_finite()
        || period_beats <= 0.0
        || resolution_beats <= 0.0
    {
        return None;
    }
    let slots = period_beats / resolution_beats;
    let rounded = slots.round();
    ((slots - rounded).abs() < 1e-6 && rounded >= 1.0 && rounded <= GROOVE_MAX_SLOTS as f64)
        .then_some(rounded as usize)
}

impl ProjectGroove {
    pub fn slot_count(&self) -> usize {
        self.shared_row.slots.len()
    }

    pub fn pad_row(&self, pad_note: i32) -> Option<&GrooveRow> {
        self.pad_rows
            .iter()
            .find(|row| row.pad_note == pad_note)
            .map(|row| &row.row)
    }

    /// The row a pad plays through: its own, else the shared row.
    pub fn row_for_pad(&self, pad_note: i32) -> &GrooveRow {
        self.pad_row(pad_note).unwrap_or(&self.shared_row)
    }

    /// Grid is valid, every row has exactly the grid's slot count, pad notes
    /// are unique and every value is finite. A groove failing this is dropped
    /// on load rather than trusted by the scheduler.
    pub fn is_well_formed(&self) -> bool {
        let Some(slots) = groove_slot_count(self.period_beats, self.resolution_beats) else {
            return false;
        };
        let row_ok = |row: &GrooveRow| {
            row.slots.len() == slots
                && row.slots.iter().all(|slot| {
                    slot.offset.is_finite()
                        && slot.velocity_scale.is_finite()
                        && slot.spread.is_finite()
                })
        };
        let mut notes = self
            .pad_rows
            .iter()
            .map(|row| row.pad_note)
            .collect::<Vec<_>>();
        notes.sort_unstable();
        let unique = notes.windows(2).all(|pair| pair[0] != pair[1]);
        unique && row_ok(&self.shared_row) && self.pad_rows.iter().all(|row| row_ok(&row.row))
    }
}

// ---------------------------------------------------------------------------
// Heard positions
// ---------------------------------------------------------------------------

/// One hit as the user heard it: cycle-beat position from the pattern start
/// and the step's velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeardHit {
    pub beat: f64,
    pub velocity: f32,
}

/// Every hit of one rack pad's source pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct GroovePadSource {
    pub pad_note: i32,
    pub hits: Vec<HeardHit>,
}

fn step_active(pattern: &TrackPatternData, step: usize) -> bool {
    pattern.track_bits[step / 64] >> (step % 64) & 1 == 1
}

fn step_timebase(pattern: &TrackPatternData, step: usize) -> Timebase {
    pattern.timebase_plock_snapshot[step]
        .map(Timebase::from_index)
        .unwrap_or(pattern.track_params.timebase)
}

fn clamp_delay(delay: f32) -> f32 {
    if delay.is_finite() {
        delay.clamp(StepParam::Delay.min(), StepParam::Delay.max())
    } else {
        StepParam::Delay.default_value()
    }
}

/// The pattern's own swing for one step, in beats: the same rule the step
/// scheduler applies (`scheduler::lookahead`): a per-step swing / swing
/// resolution p-lock overrides the track value, and odd buckets of the
/// resolution grid, keyed to the step's cycle-start beat, are delayed.
fn step_swing_beats(pattern: &TrackPatternData, step: usize, cycle_start_beats: f64) -> f64 {
    let swing_pct = pattern.swing_plock_snapshot[step]
        .map(f32::from_bits)
        .unwrap_or(pattern.track_params.swing);
    if !(swing_pct > 50.0) {
        return 0.0;
    }
    let resolution = pattern.swing_resolution_plock_snapshot[step]
        .map(SwingResolution::from_index)
        .unwrap_or(pattern.track_params.swing_resolution);
    let bucket = ((cycle_start_beats + EPS) / resolution.step_beats()).floor() as u64;
    if bucket % 2 == 0 {
        return 0.0;
    }
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution.step_beats()
}

/// Where every hit of `pattern` was heard, in the pattern's cycle beats (one
/// pass over the pattern, step 0 at beat 0):
///
/// ```text
/// beat = step start (the pattern's real geometry: timebase p-locks, sync waits)
///      + delay * step beats     // StepParam::Delay, or each chord note's delay
///      + the pattern's swing    // what the user heard is part of the feel
/// ```
///
/// A chord step (Capture MIDI stores every captured hit that way,
/// `app/retrospective.rs`) yields one hit per chord note at that note's own
/// delay, and the step scheduler ignores `StepParam::Delay` on chord steps, so
/// this does too.
pub fn heard_hits(pattern: &TrackPatternData) -> Vec<HeardHit> {
    let geometry = pattern.step_geometry();
    let num_steps = geometry.num_steps().min(MAX_STEPS);
    let mut hits = Vec::new();
    for step in 0..num_steps {
        if !step_active(pattern, step) {
            continue;
        }
        let Some(params) = pattern.step_data.get(step) else {
            continue;
        };
        let start = geometry.beats_at_steps(step as f64);
        let step_beats = step_timebase(pattern, step).step_beats(num_steps);
        let swing = step_swing_beats(pattern, step, start);
        let velocity = params[StepParam::Velocity.index()];
        let chord_delays = pattern.chord_snapshot.delays.get(step).filter(|_| {
            pattern
                .chord_snapshot
                .steps
                .get(step)
                .is_some_and(|notes| !notes.is_empty())
        });
        match chord_delays {
            Some(delays) => {
                let notes = pattern.chord_snapshot.steps[step].len();
                for voice in 0..notes {
                    let delay = clamp_delay(delays.get(voice).copied().unwrap_or(0.0));
                    hits.push(HeardHit {
                        beat: start + delay as f64 * step_beats + swing,
                        velocity,
                    });
                }
            }
            None => {
                let delay = clamp_delay(params[StepParam::Delay.index()]);
                hits.push(HeardHit {
                    beat: start + delay as f64 * step_beats + swing,
                    velocity,
                });
            }
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GrooveExtractOptions {
    pub name: String,
    pub period_beats: f64,
    pub resolution_beats: f64,
}

impl Default for GrooveExtractOptions {
    fn default() -> Self {
        Self {
            name: "Groove".to_string(),
            period_beats: GROOVE_PERIOD_ONE_BAR,
            resolution_beats: GROOVE_RESOLUTION_SIXTEENTH,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrooveExtractError {
    /// `period / resolution` is not a whole, bounded slot count.
    InvalidGrid,
    /// No pad had a single active step.
    NoHits,
}

impl std::fmt::Display for GrooveExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGrid => f.write_str("The groove period must be a whole number of slots"),
            Self::NoHits => {
                f.write_str("The rack's patterns have no trigs to extract a groove from")
            }
        }
    }
}

/// Nearest-slot snap: the global slot index (unbounded, before folding) and
/// the signed offset in slot units, in [-0.5, 0.5). A hit the recorder stored
/// as "very late on the previous step" reads as "slightly early on the right
/// step", which is the correct reading of a pushed kick.
pub fn snap_to_slot(beat: f64, resolution_beats: f64) -> (i64, f32) {
    let position = beat / resolution_beats;
    let slot = (position + 0.5 + EPS).floor();
    let offset = (position - slot).clamp(-0.5, 0.5 - 1e-6);
    (slot as i64, offset as f32)
}

/// One kept hit after the per-repeat collision rule.
#[derive(Clone, Copy, Debug)]
struct SlotSample {
    offset: f32,
    velocity: f32,
}

/// Per pad: snap every hit and keep ONE per global slot (per repeat). Two
/// hits in the same slot of the same repeat (flam, ghost pickup) keep the
/// louder one for timing and velocity; on a velocity tie the one nearer the
/// grid wins, then the earlier one, so the result never depends on input
/// order.
fn resolve_collisions(hits: &[HeardHit], resolution_beats: f64) -> Vec<(i64, SlotSample)> {
    let mut kept = std::collections::BTreeMap::<i64, SlotSample>::new();
    for hit in hits {
        if !hit.beat.is_finite() {
            continue;
        }
        let (slot, offset) = snap_to_slot(hit.beat, resolution_beats);
        let velocity = if hit.velocity.is_finite() {
            hit.velocity.max(0.0)
        } else {
            0.0
        };
        let candidate = SlotSample { offset, velocity };
        match kept.get_mut(&slot) {
            None => {
                kept.insert(slot, candidate);
            }
            Some(existing) => {
                let replace = candidate.velocity > existing.velocity
                    || (candidate.velocity == existing.velocity
                        && (candidate.offset.abs() < existing.offset.abs()
                            || (candidate.offset.abs() == existing.offset.abs()
                                && candidate.offset < existing.offset)));
                if replace {
                    *existing = candidate;
                }
            }
        }
    }
    kept.into_iter().collect()
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) * 0.5
    }
}

/// Median absolute deviation around `center`.
fn mad(values: &[f32], center: f32) -> f32 {
    let mut deviations = values
        .iter()
        .map(|value| (value - center).abs())
        .collect::<Vec<_>>();
    median(&mut deviations)
}

/// Aggregates per-slot samples into a measured row (`None` = unmeasured).
/// `velocity_ref` is the row's median velocity; accents are relative to it.
fn measured_row(per_slot: &[Vec<SlotSample>], velocity_ref: f32) -> Vec<Option<GrooveSlot>> {
    per_slot
        .iter()
        .map(|samples| {
            if samples.is_empty() {
                return None;
            }
            let mut offsets = samples
                .iter()
                .map(|sample| sample.offset)
                .collect::<Vec<_>>();
            let offset = median(&mut offsets);
            let spread = mad(&offsets, offset);
            let mut velocities = samples
                .iter()
                .map(|sample| sample.velocity)
                .collect::<Vec<_>>();
            let velocity = median(&mut velocities);
            let velocity_scale = if velocity_ref > 1e-6 {
                velocity / velocity_ref
            } else {
                1.0
            };
            Some(GrooveSlot {
                offset,
                velocity_scale,
                spread,
                source: GrooveSlotSource::Measured,
            })
        })
        .collect()
}

/// Metric class of a beat position: on-beat 0, "&" 1, "e"/"a" 2, 32nd
/// off-positions 3, and so on by power-of-two subdivision; positions on no
/// power-of-two grid (triplets) share one class.
pub fn metric_class(beat: f64) -> u32 {
    let fraction = beat.rem_euclid(1.0);
    let mut denominator = 1.0_f64;
    for class in 0..7 {
        let scaled = fraction * denominator;
        if (scaled - scaled.round()).abs() < 1e-6 {
            return class;
        }
        denominator *= 2.0;
    }
    u32::MAX
}

/// Fills a measured row in the spec's order: other half of a two-bar period,
/// same-metric-class interpolation, the shared row (pad rows only), zero.
fn fill_row(
    measured: &[Option<GrooveSlot>],
    period_beats: f64,
    resolution_beats: f64,
    shared: Option<&GrooveRow>,
) -> GrooveRow {
    let slots = measured.len();
    let two_bar = slots % 2 == 0 && period_beats >= GROOVE_PERIOD_TWO_BARS - EPS;
    let classes = (0..slots)
        .map(|slot| metric_class(slot as f64 * resolution_beats))
        .collect::<Vec<_>>();
    let filled = (0..slots)
        .map(|slot| {
            if let Some(value) = measured[slot] {
                return value;
            }
            // 1. The same position in the other half of the period.
            if two_bar {
                if let Some(other) = measured[(slot + slots / 2) % slots] {
                    return GrooveSlot {
                        source: GrooveSlotSource::FilledFromOtherHalf,
                        ..other
                    };
                }
            }
            // 2. Interpolate between the nearest measured slots of the same
            //    metric class, circularly (a groove loops).
            let class = classes[slot];
            let find = |forward: bool| {
                (1..slots).find_map(|distance| {
                    let index = if forward {
                        (slot + distance) % slots
                    } else {
                        (slot + slots - distance) % slots
                    };
                    (classes[index] == class)
                        .then_some(measured[index])
                        .flatten()
                        .map(|value| (distance, value))
                })
            };
            if let (Some((back, before)), Some((ahead, after))) = (find(false), find(true)) {
                let t = back as f32 / (back + ahead) as f32;
                let lerp = |a: f32, b: f32| a + (b - a) * t;
                return GrooveSlot {
                    offset: lerp(before.offset, after.offset),
                    velocity_scale: lerp(before.velocity_scale, after.velocity_scale),
                    spread: lerp(before.spread, after.spread),
                    source: GrooveSlotSource::FilledFromNeighbors,
                };
            }
            // 3. The shared row's value at this slot, when it has one.
            if let Some(shared_slot) = shared.and_then(|row| row.slots.get(slot)) {
                if shared_slot.source != GrooveSlotSource::Zero {
                    return GrooveSlot {
                        source: GrooveSlotSource::FilledFromShared,
                        ..*shared_slot
                    };
                }
            }
            // 4. Straight.
            GrooveSlot::default()
        })
        .collect();
    GrooveRow { slots: filled }
}

/// Extracts a groove from every pad's heard hits (see [`heard_hits`]).
///
/// Hits are snapped to the NEAREST slot, folded modulo the period, and per
/// `(pad, slot)` aggregate to the median offset, its MAD spread, and the
/// median velocity over the pad's median velocity. The shared row pools every
/// pad, each hit's velocity first normalized by its own pad's median so a loud
/// kick does not read as an accent against a quiet hat. Pads that were never
/// heard get no row (they fall back to the shared row). Pad rows keep the
/// input order.
pub fn extract_groove(
    id: GrooveId,
    options: &GrooveExtractOptions,
    pads: &[GroovePadSource],
) -> Result<ProjectGroove, GrooveExtractError> {
    let slots = groove_slot_count(options.period_beats, options.resolution_beats)
        .ok_or(GrooveExtractError::InvalidGrid)?;
    let resolution = options.resolution_beats;

    struct PadSamples {
        pad_note: i32,
        per_slot: Vec<Vec<SlotSample>>,
        velocity_ref: f32,
    }

    let mut pad_samples = Vec::<PadSamples>::new();
    let mut shared_per_slot = vec![Vec::<SlotSample>::new(); slots];
    for pad in pads {
        let kept = resolve_collisions(&pad.hits, resolution);
        if kept.is_empty() {
            continue;
        }
        let mut velocities = kept
            .iter()
            .map(|(_, sample)| sample.velocity)
            .collect::<Vec<_>>();
        let velocity_ref = median(&mut velocities);
        let mut per_slot = vec![Vec::new(); slots];
        for (global, sample) in kept {
            let slot = global.rem_euclid(slots as i64) as usize;
            per_slot[slot].push(sample);
            let normalized = if velocity_ref > 1e-6 {
                sample.velocity / velocity_ref
            } else {
                1.0
            };
            shared_per_slot[slot].push(SlotSample {
                offset: sample.offset,
                velocity: normalized,
            });
        }
        pad_samples.push(PadSamples {
            pad_note: pad.pad_note,
            per_slot,
            velocity_ref,
        });
    }
    if pad_samples.is_empty() {
        return Err(GrooveExtractError::NoHits);
    }

    let mut shared_velocities = shared_per_slot
        .iter()
        .flatten()
        .map(|sample| sample.velocity)
        .collect::<Vec<_>>();
    let shared_ref = median(&mut shared_velocities);
    let shared_row = fill_row(
        &measured_row(&shared_per_slot, shared_ref),
        options.period_beats,
        resolution,
        None,
    );
    let mut pad_rows = Vec::<GroovePadRow>::with_capacity(pad_samples.len());
    for pad in pad_samples {
        let measured = measured_row(&pad.per_slot, pad.velocity_ref);
        let row = fill_row(
            &measured,
            options.period_beats,
            resolution,
            Some(&shared_row),
        );
        // Two sources for one pad note (never from a well-formed rack) merge
        // by keeping the first: rows are keyed by pad note.
        if pad_rows
            .iter()
            .all(|existing| existing.pad_note != pad.pad_note)
        {
            pad_rows.push(GroovePadRow {
                pad_note: pad.pad_note,
                row,
            });
        }
    }
    Ok(ProjectGroove {
        id,
        name: options.name.clone(),
        period_beats: options.period_beats,
        resolution_beats: resolution,
        pad_rows,
        shared_row,
    })
}

// ---------------------------------------------------------------------------
// Quantize source
// ---------------------------------------------------------------------------

fn slot_has_step_locks(slot: &EffectSlotSnapshot, step: usize) -> bool {
    slot.plocks
        .get(step)
        .is_some_and(|row| row.iter().any(Option::is_some))
        || slot
            .tensor_params
            .iter()
            .any(|param| param.plocks.get(step).is_some_and(Option::is_some))
}

/// Whether a step's content can move one step without anything that is not
/// carried by `TrackPatternData::copy_step_content_from` (device p-locks,
/// instrument-rack locks, process lanes) or that changes the pattern's
/// geometry (timebase p-locks, sync waits) being left behind.
fn step_can_move(pattern: &TrackPatternData, step: usize) -> bool {
    pattern.timebase_plock_snapshot[step].is_none()
        && pattern.step_data.get(step).is_some_and(|params| {
            params[StepParam::Sync.index()] == StepParam::Sync.default_value()
        })
        && !pattern
            .effect_slots
            .iter()
            .chain(pattern.midi_fx_slots.iter())
            .chain(std::iter::once(&pattern.instrument_slot))
            .any(|slot| slot_has_step_locks(slot, step))
}

/// How late each of one step's hits was HEARD, in units of the step: its
/// chord-note delay or step Delay plus the pattern's swing for that step. The
/// move rule compares this against half a step, the same line extraction's
/// nearest snap draws.
fn step_heard_lateness(
    pattern: &TrackPatternData,
    geometry: &crate::sequencer::PatternStepGeometry,
    step: usize,
) -> Vec<f64> {
    let num_steps = geometry.num_steps();
    let step_beats = step_timebase(pattern, step).step_beats(num_steps).max(EPS);
    let swing = step_swing_beats(pattern, step, geometry.beats_at_steps(step as f64)) / step_beats;
    let delays = match pattern.chord_snapshot.steps.get(step) {
        Some(notes) if !notes.is_empty() => (0..notes.len())
            .map(|voice| {
                clamp_delay(
                    pattern
                        .chord_snapshot
                        .delays
                        .get(step)
                        .and_then(|delays| delays.get(voice))
                        .copied()
                        .unwrap_or(0.0),
                )
            })
            .collect::<Vec<_>>(),
        _ => vec![clamp_delay(
            pattern
                .step_data
                .get(step)
                .map(|params| params[StepParam::Delay.index()])
                .unwrap_or(0.0),
        )],
    };
    delays
        .into_iter()
        .map(|delay| delay as f64 + swing)
        .collect()
}

/// Straightens a groove's source pattern so that, played through the groove,
/// it sounds as it did before extraction ("Quantize source afterwards"):
///
/// - every hit is moved to its NEAREST step, the same reading extraction's
///   nearest-slot snap makes: a step whose hits were all heard at least half
///   a step late (delay plus swing) moves whole onto the next step, when that step is empty and neither
///   step carries content a move cannot take along (device p-locks,
///   timebase/sync, an instrument rack, process lanes, a different bar
///   transpose). A step that cannot move stays on its own step;
/// - every step Delay and chord-note delay is zeroed;
/// - track swing is set to 50 and per-step swing p-locks are cleared.
///
/// Returns whether anything changed.
pub fn quantize_groove_source(pattern: &mut TrackPatternData) -> bool {
    let source = pattern.clone();
    let geometry = source.step_geometry();
    let num_steps = geometry.num_steps().min(MAX_STEPS);
    let pattern_can_move = source.rack_track.is_none()
        && source.process_chain.slots.is_empty()
        && source.project_process_lane_overrides.is_empty();
    let mut changed = false;

    if pattern_can_move && num_steps > 1 {
        let mut targeted = [false; MAX_STEPS];
        for step in 0..num_steps {
            if !step_active(&source, step) {
                continue;
            }
            let target = (step + 1) % num_steps;
            let all_late = step_heard_lateness(&source, &geometry, step)
                .iter()
                .all(|lateness| *lateness >= 0.5 - EPS);
            if !all_late
                || step_active(&source, target)
                || targeted[target]
                || !step_can_move(&source, step)
                || !step_can_move(&source, target)
                || source.bar_transpose_snapshot[bar_of_step(step)]
                    != source.bar_transpose_snapshot[bar_of_step(target)]
            {
                continue;
            }
            targeted[target] = true;
            pattern.copy_step_content_from(target, &source, step);
            pattern.clear_step_content_at(step);
            changed = true;
        }
    }

    for params in pattern.step_data.iter_mut() {
        let delay = &mut params[StepParam::Delay.index()];
        if *delay != StepParam::Delay.default_value() {
            *delay = StepParam::Delay.default_value();
            changed = true;
        }
    }
    for delays in pattern.chord_snapshot.delays.iter_mut() {
        for delay in delays.iter_mut() {
            if *delay != 0.0 {
                *delay = 0.0;
                changed = true;
            }
        }
    }
    if pattern.track_params.swing != 50.0 {
        pattern.track_params.swing = 50.0;
        changed = true;
    }
    for swing in pattern.swing_plock_snapshot.iter_mut() {
        if swing.take().is_some() {
            changed = true;
        }
    }
    changed
}
