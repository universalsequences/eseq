//! Groove application (docs/rack-groove-spec.md §Application, bead
//! eseq-groove.2): the scheduler's pre-resolved per-track groove table and the
//! one timing function every trig site calls.
//!
//! The app resolves each rack member's row once, when rack config or rack
//! membership changes ([`track_groove_snapshots`]); the scheduler then only
//! indexes `SequencerSnapshot::track_grooves` by track and calls
//! [`grooved_sample_time`] with the trig's straight boundary beat. A track
//! with no entry plays exactly as before.
//!
//! Early hits (eseq-groove.3): offsets are signed. A negative offset moves a
//! trig BEFORE its straight boundary, so the scheduler discovers trigs
//! [`TrackGrooveSnapshot::max_early_beats`] ahead of where they must sound
//! (see [`max_early_lead_beats`] and the lookahead horizon), and every site
//! floors the grooved sample at the audio frontier ([`GrooveFloor`]), so an
//! early trig is never enqueued at a passed sample, and drops (rather than
//! re-plays) an early hit that already sounded before a mid-play resync.
//! Applied offsets are bounded below by [`MAX_EARLY_SLOTS`] slots, which
//! bounds the lead: `E <= 0.75 * resolution_beats`.
//!
//! Velocity and random amounts (eseq-groove.5): `velocity_amount` lerps the
//! slot's `velocity_scale` into the trig's resolved velocity
//! ([`TrackGrooveSnapshot::apply_velocity`]); `random_amount` adds
//! `spread * noise` to the offset, where the noise is a pure hash of the
//! absolute transport slot index and the member's pad note, so an offline
//! render or bounce is reproducible while every bar still varies.

use std::sync::Arc;

use super::{GrooveRow, ProjectGroove};
use crate::project::ProjectRackConfig;

/// A slot position this close (in slot units) to a slot boundary IS that
/// boundary. Straight boundary beats reach the groove in slightly different
/// float forms depending on the source (grid index × step, transport clock
/// minus an in-step remainder, a sample converted back to beats), so the
/// snap keeps coincident trigs from different sources on the same slot.
/// 1e-4 of a 16th is about 0.3 samples at 120 BPM / 48 kHz.
const SLOT_SNAP: f64 = 1.0e-4;

/// The earliest an applied offset may land, in slots before the straight
/// boundary. Extraction keeps `|offset| < 0.5` and `timing_amount <= 1.5`, so
/// a measured pocket never reaches it; the cap only bounds Random jitter and
/// hand-built rows, which is what keeps the scheduler's discovery lead
/// `E = max early offset` at most `0.75 * resolution_beats` (rack groove spec
/// §Early hits).
pub const MAX_EARLY_SLOTS: f64 = 0.75;

/// One member track's resolved groove: its pad row (or the shared row when
/// the pad has none) plus the rack's amounts. Parallel to the snapshot's
/// tracks; built off the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackGrooveSnapshot {
    pub period_beats: f64,
    pub resolution_beats: f64,
    pub row: Arc<GrooveRow>,
    pub timing_amount: f32,
    /// How much of the slot's `velocity_scale` reaches the trig: 0 leaves
    /// velocity untouched, 1 applies the measured accent.
    pub velocity_amount: f32,
    /// How much of the slot's `spread` (MAD, in slots) jitters the offset.
    pub random_amount: f32,
    /// The Random seed's pad half: the member's pad note, or
    /// [`padless_seed_key`] for a member with no pad. Two members on the
    /// same pad note jitter together, like one drummer's hand.
    pub pad_note: i32,
}

/// The seed key of a rack member without a pad: below the pad-note domain,
/// so it never collides with a real pad, and distinct per member.
pub fn padless_seed_key(member: usize) -> i32 {
    i32::MIN.saturating_add(member.min(i32::MAX as usize) as i32)
}

/// Where a transport beat falls in the groove: the slot `k` (and the one
/// after it, wrapping) with the fraction `t` between them, plus the ABSOLUTE
/// transport slot index (not wrapped by the period) that seeds Random.
struct SlotPosition {
    k: usize,
    next: usize,
    t: f64,
    absolute: i64,
}

/// Deterministic noise in `[-1, 1)` from `(absolute slot index, pad note)`:
/// a splitmix64 finalizer over both, so neighbouring slots and pads are
/// uncorrelated and the same inputs always give the same value.
pub fn groove_hash_noise(absolute_slot: i64, pad_note: i32) -> f64 {
    let mut z = (absolute_slot as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pad_note as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ 0x6772_6f6f_7665_5f35; // "groove_5"
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Top 53 bits -> [0, 1) -> [-1, 1).
    ((z >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
}

impl TrackGrooveSnapshot {
    fn slot_position(&self, boundary_beats: f64) -> Option<SlotPosition> {
        let n = self.row.slots.len();
        if n == 0
            || !boundary_beats.is_finite()
            || !(self.period_beats > 0.0)
            || !(self.resolution_beats > 0.0)
        {
            return None;
        }
        let snap = |mut pos: f64| {
            let nearest = pos.round();
            if (pos - nearest).abs() < SLOT_SNAP {
                pos = nearest;
            }
            pos
        };
        let pos = snap(boundary_beats.rem_euclid(self.period_beats) / self.resolution_beats);
        let k = pos.floor();
        let t = pos - k;
        let k = (k.max(0.0) as usize) % n;
        let absolute = snap(boundary_beats / self.resolution_beats).floor();
        let absolute = if absolute.is_finite() {
            absolute.clamp(i64::MIN as f64, i64::MAX as f64) as i64
        } else {
            0
        };
        Some(SlotPosition {
            k,
            next: (k + 1) % n,
            t,
            absolute,
        })
    }

    /// The groove's offset at `boundary_beats` (transport beats, bar
    /// aligned), in beats, after the random and timing amounts:
    ///
    /// ```text
    /// pos  = boundary.rem_euclid(period) / resolution
    /// off  = lerp(row[k].offset, row[k+1 mod n].offset, frac(pos))
    /// off += random * row[k].spread * noise(absolute slot, pad note)
    /// off *= timing
    /// ```
    ///
    /// The interpolation is what lets a 16th groove shape 32nd hats or
    /// triplet-quantized neurons; two-slot swing is exactly this warp.
    /// Signed (negative = early), never earlier than [`MAX_EARLY_SLOTS`].
    pub fn offset_beats(&self, boundary_beats: f64) -> f64 {
        self.offset_beats_with_random(boundary_beats, true)
    }

    /// [`offset_beats`](Self::offset_beats), with the Random jitter only
    /// when `with_random` (the record unwind reads the bare pocket, see
    /// `pocket_offset_beats`).
    pub(super) fn offset_beats_with_random(&self, boundary_beats: f64, with_random: bool) -> f64 {
        let Some(at) = self.slot_position(boundary_beats) else {
            return 0.0;
        };
        let slots = &self.row.slots;
        let a = slots[at.k].offset as f64;
        let b = slots[at.next].offset as f64;
        let mut offset_slots = a + (b - a) * at.t;
        let jitter = self.random_amount as f64 * slots[at.k].spread as f64;
        if with_random && jitter != 0.0 && jitter.is_finite() {
            offset_slots += jitter * groove_hash_noise(at.absolute, self.pad_note);
        }
        let offset_slots = offset_slots * self.timing_amount as f64;
        let beats = offset_slots * self.resolution_beats;
        if beats.is_finite() {
            // `max` after the finiteness check: `f64::max` would turn a NaN
            // into the cap instead of "no move".
            offset_slots.max(-MAX_EARLY_SLOTS) * self.resolution_beats
        } else {
            0.0
        }
    }

    /// How far before its straight boundary this groove can move a trig, in
    /// beats: the most negative [`offset_beats`](Self::offset_beats) over
    /// every position, as a non-negative lead. Interpolation stays between
    /// neighbouring slot offsets and Random adds at most
    /// `random * spread[k]` slots, so the scan is exact per slot pair (and
    /// conservative only in assuming the noise reaches its bound). Zero for a
    /// late-only (or degenerate) groove, so it changes nothing there.
    pub fn max_early_beats(&self) -> f64 {
        let slots = &self.row.slots;
        let n = slots.len();
        if n == 0 || !(self.period_beats > 0.0) || !(self.resolution_beats > 0.0) {
            return 0.0;
        }
        let timing = self.timing_amount as f64;
        let random = self.random_amount as f64;
        let mut earliest_slots = 0.0_f64;
        for k in 0..n {
            let a = slots[k].offset as f64;
            let b = slots[(k + 1) % n].offset as f64;
            let jitter = (random * slots[k].spread as f64).abs();
            // The lowest offset this slot span can produce after timing: the
            // low end of `[min - jitter, max + jitter]` for a positive
            // amount, the high end mirrored for a negative one.
            let low = if timing >= 0.0 {
                (a.min(b) - jitter) * timing
            } else {
                (a.max(b) + jitter) * timing
            };
            if low.is_finite() {
                earliest_slots = earliest_slots.min(low);
            }
        }
        (-earliest_slots).min(MAX_EARLY_SLOTS) * self.resolution_beats
    }

    /// How far AFTER its straight boundary this groove can move a trig, in
    /// beats: the largest [`offset_beats`](Self::offset_beats) over every
    /// position (the mirror of [`max_early_beats`](Self::max_early_beats),
    /// exact per slot pair, conservative only in assuming Random reaches
    /// its bound). Zero for an early-only (or degenerate) groove. A mid-play
    /// resync looks this far back for late hits whose straight boundary it
    /// already passed but which have not sounded yet (rack groove spec
    /// §Early hits, eseq-groove.8).
    pub fn max_late_beats(&self) -> f64 {
        let slots = &self.row.slots;
        let n = slots.len();
        if n == 0 || !(self.period_beats > 0.0) || !(self.resolution_beats > 0.0) {
            return 0.0;
        }
        let timing = self.timing_amount as f64;
        let random = self.random_amount as f64;
        let mut latest_slots = 0.0_f64;
        for k in 0..n {
            let a = slots[k].offset as f64;
            let b = slots[(k + 1) % n].offset as f64;
            let jitter = (random * slots[k].spread as f64).abs();
            let high = if timing >= 0.0 {
                (a.max(b) + jitter) * timing
            } else {
                (a.min(b) - jitter) * timing
            };
            if high.is_finite() {
                latest_slots = latest_slots.max(high);
            }
        }
        latest_slots * self.resolution_beats
    }

    /// The velocity multiplier at `boundary_beats`:
    /// `lerp(1, lerp(row[k].velocity_scale, row[k+1].velocity_scale, t),
    /// velocity_amount)`. Exactly `1.0` at a zero amount (and for any
    /// degenerate input), so a timing-only groove leaves velocity untouched.
    pub fn velocity_scale(&self, boundary_beats: f64) -> f32 {
        if self.velocity_amount == 0.0 {
            return 1.0;
        }
        let Some(at) = self.slot_position(boundary_beats) else {
            return 1.0;
        };
        let slots = &self.row.slots;
        let a = slots[at.k].velocity_scale as f64;
        let b = slots[at.next].velocity_scale as f64;
        let slot_scale = a + (b - a) * at.t;
        let scale = 1.0 + (slot_scale - 1.0) * self.velocity_amount as f64;
        if scale.is_finite() {
            scale.max(0.0) as f32
        } else {
            1.0
        }
    }

    /// The trig's resolved `velocity` through the groove's accent, clamped to
    /// the Velocity step param's range. A neutral scale returns `velocity`
    /// bit for bit (even out of range), so velocity amount 0 changes nothing.
    pub fn apply_velocity(&self, velocity: f32, boundary_beats: f64) -> f32 {
        let scale = self.velocity_scale(boundary_beats);
        if scale == 1.0 {
            return velocity;
        }
        let param = crate::sequencer::StepParam::Velocity;
        (velocity * scale).clamp(param.min(), param.max())
    }
}

/// The groove offset in samples for a trig whose straight boundary is
/// `boundary_beats` (negative = early): the one timing function every trig
/// site shares, so the same pad at the same position moves by the same number
/// of samples no matter which source fired it.
pub fn groove_offset_samples(
    groove: &TrackGrooveSnapshot,
    boundary_beats: f64,
    samples_per_quarter: f64,
) -> i64 {
    if !(samples_per_quarter > 0.0) {
        return 0;
    }
    let samples = (groove.offset_beats(boundary_beats) * samples_per_quarter).round();
    if samples.is_finite() {
        samples.clamp(i64::MIN as f64, i64::MAX as f64) as i64
    } else {
        0
    }
}

/// Where an early groove offset may land relative to the audio frontier
/// (rack groove spec §Early hits). Built per lookahead call by the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GrooveFloor {
    /// The audio frontier (`rendered`): an early trig is never enqueued at a
    /// sample the audio thread has already passed.
    pub not_before: u64,
    /// The previous scheduling frontier when playback last resynced mid-play
    /// (queue cleared, clock rewound to `rendered`); zero after a transport
    /// start or seek. A trig whose straight sample lies below it was already
    /// discovered and enqueued before the resync, so an early move that lands
    /// before `not_before` already SOUNDED: it is dropped, not clamped, or the
    /// resync would play it a second time.
    pub replayed_until: u64,
}

impl GrooveFloor {
    /// A floor with no resync history: early trigs clamp at `not_before`.
    pub fn at(not_before: u64) -> Self {
        Self {
            not_before,
            replayed_until: 0,
        }
    }
}

/// The straight sample moved by the groove (spec's `grooved_sample_time`,
/// timing half).
///
/// `floor.not_before` is the audio frontier the scheduler is extending from:
/// an early offset never lands before it, so a trig is never enqueued at a
/// sample the audio thread has already passed. The floor never delays a
/// trig whose straight sample is itself earlier (that trig was already late
/// before the groove touched it), and a late offset ignores it.
///
/// `None` means "already played": the early move lands before the frontier
/// and the straight sample was discovered before a mid-play resync
/// ([`GrooveFloor::replayed_until`]), so the hit sounded before the queue was
/// cleared. Callers skip the trig. A late or zero offset is never `None`.
pub fn grooved_sample_time(
    groove: &TrackGrooveSnapshot,
    boundary_beats: f64,
    straight_sample: u64,
    samples_per_quarter: f64,
    floor: GrooveFloor,
) -> Option<u64> {
    let offset = groove_offset_samples(groove, boundary_beats, samples_per_quarter);
    let moved = straight_sample.saturating_add_signed(offset);
    if offset >= 0 || moved >= floor.not_before {
        return Some(moved);
    }
    if straight_sample < floor.replayed_until {
        return None;
    }
    Some(moved.max(floor.not_before.min(straight_sample)))
}

/// The scheduler's discovery lead for a groove table: the largest
/// [`TrackGrooveSnapshot::max_early_beats`] over every grooved track. The
/// lookahead schedules this far past its horizon, so a trig whose straight
/// boundary lies up to this far beyond the audio's next block is already
/// enqueued when its early offset makes it due. Zero when no groove can move
/// a trig early, which keeps late-only (and ungrooved) scheduling unchanged.
pub fn max_early_lead_beats(grooves: &[Option<TrackGrooveSnapshot>]) -> f64 {
    grooves
        .iter()
        .flatten()
        .map(TrackGrooveSnapshot::max_early_beats)
        .fold(0.0, f64::max)
}

/// The resync look-back for a groove table: the largest
/// [`TrackGrooveSnapshot::max_late_beats`] over every grooved track. Zero
/// when no groove can move a trig late.
pub fn max_late_lead_beats(grooves: &[Option<TrackGrooveSnapshot>]) -> f64 {
    grooves
        .iter()
        .flatten()
        .map(TrackGrooveSnapshot::max_late_beats)
        .fold(0.0, f64::max)
}

// ---------------------------------------------------------------------------
// MPC swing (factory library content)
// ---------------------------------------------------------------------------

/// The MPC swing amounts (the classic 50/54/58/62/66/71/75 ladder) the
/// factory library ships as `.groove` files.
pub const MPC_SWING_PERCENTS: [u32; 7] = [50, 54, 58, 62, 66, 71, 75];

/// The resolutions the factory MPC swings ship at: 16ths, then 8ths.
pub const MPC_SWING_RESOLUTIONS: [f64; 2] = [0.25, 0.5];

/// One MPC swing as a groove: period = two slots at `resolution_beats`, the
/// off slot late by the same amount track swing would delay it,
/// `(pct/100 - 0.5) * 2` slots. The factory `content/grooves/mpc-swing-*`
/// files are this function's output (a test keeps them in sync).
pub fn mpc_swing_groove(percent: u32, resolution_beats: f64) -> ProjectGroove {
    let offset = ((percent as f32 / 100.0) - 0.5) * 2.0;
    let slot = |offset: f32| super::GrooveSlot {
        offset,
        velocity_scale: 1.0,
        spread: 0.0,
        source: super::GrooveSlotSource::Measured,
    };
    let label = if resolution_beats < 0.375 { "16" } else { "8" };
    ProjectGroove {
        id: 0,
        name: format!("MPC {label} Swing {percent}%"),
        period_beats: resolution_beats * 2.0,
        resolution_beats,
        pad_rows: Vec::new(),
        shared_row: GrooveRow {
            slots: vec![slot(0.0), slot(offset)],
        },
    }
}

/// The factory file stem of one MPC swing: `mpc-swing-58-16th`,
/// `mpc-swing-66-8th`.
pub fn mpc_swing_file_stem(percent: u32, resolution_beats: f64) -> String {
    let label = if resolution_beats < 0.375 {
        "16th"
    } else {
        "8th"
    };
    format!("mpc-swing-{percent}-{label}")
}

impl ProjectRackConfig {
    /// The pool groove the rack plays through. `None` when nothing is active
    /// or the id is not in `pool`.
    pub fn active_groove<'a>(&self, pool: &'a [ProjectGroove]) -> Option<&'a ProjectGroove> {
        super::pool_groove(pool, self.groove.active?)
    }
}

/// The scheduler's per-track groove table for `num_tracks` tracks, from each
/// drum rack's `(members, config)` and the project groove `pool` the racks
/// reference. Every member of a rack with an active, well-formed groove gets
/// an entry: the pad row [`ProjectGroove::resolve_pad_row`] picks for the
/// pad's note and effective role (same note, then same role), else the
/// shared row (a member without a pad, too). Everything else is
/// `None`, which the scheduler treats as "no groove".
pub fn track_groove_snapshots<'a>(
    racks: impl IntoIterator<Item = (&'a [usize], &'a ProjectRackConfig)>,
    pool: &[ProjectGroove],
    num_tracks: usize,
) -> Vec<Option<TrackGrooveSnapshot>> {
    let mut out = vec![None; num_tracks];
    for (members, rack) in racks {
        let Some(groove) = rack.active_groove(pool) else {
            continue;
        };
        if !groove.is_well_formed() {
            continue;
        }
        let shared = Arc::new(groove.shared_row.clone());
        let entry = |row: Arc<GrooveRow>, pad_note: i32| TrackGrooveSnapshot {
            period_beats: groove.period_beats,
            resolution_beats: groove.resolution_beats,
            row,
            timing_amount: rack.groove.timing_amount,
            velocity_amount: rack.groove.velocity_amount,
            random_amount: rack.groove.random_amount,
            pad_note,
        };
        for (member, &track) in members.iter().enumerate() {
            let Some(slot) = out.get_mut(track) else {
                continue;
            };
            let pad = rack.pads.iter().find(|pad| pad.member == member);
            let pad_row = pad.and_then(|pad| {
                groove
                    .resolve_pad_row(pad.pad_note, pad.effective_role())
                    .map(|row| &row.row)
            });
            let row = match pad_row {
                Some(row) => Arc::new(row.clone()),
                None => Arc::clone(&shared),
            };
            let pad_note = pad.map_or_else(|| padless_seed_key(member), |pad| pad.pad_note);
            *slot = Some(entry(row, pad_note));
        }
    }
    out
}
