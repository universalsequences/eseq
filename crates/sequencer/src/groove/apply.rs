//! Groove application (docs/rack-groove-spec.md §Application, bead
//! eseq-groove.2): the scheduler's pre-resolved per-track groove table and the
//! one timing function every trig site calls.
//!
//! The app resolves each rack member's row once, when rack config or rack
//! membership changes ([`track_groove_snapshots`]); the scheduler then only
//! indexes `SequencerSnapshot::track_grooves` by track and calls
//! [`groove_delay_samples`] with the trig's straight boundary beat. A track
//! with no entry plays exactly as before.
//!
//! This slice is LATE-ONLY: applied offsets are clamped to `>= 0` until the
//! lookahead can discover trigs ahead of the chunk edge (eseq-groove.3).
//! Velocity and random amounts are carried on the snapshot but not applied
//! yet (eseq-groove.5).

use std::sync::Arc;

use super::{GrooveRef, GrooveRow, ProjectGroove};
use crate::project::ProjectRackConfig;

/// A slot position this close (in slot units) to a slot boundary IS that
/// boundary. Straight boundary beats reach the groove in slightly different
/// float forms depending on the source (grid index × step, transport clock
/// minus an in-step remainder, a sample converted back to beats), so the
/// snap keeps coincident trigs from different sources on the same slot.
/// 1e-4 of a 16th is about 0.3 samples at 120 BPM / 48 kHz.
const SLOT_SNAP: f64 = 1.0e-4;

/// One member track's resolved groove: its pad row (or the shared row when
/// the pad has none) plus the rack's amounts. Parallel to the snapshot's
/// tracks; built off the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackGrooveSnapshot {
    pub period_beats: f64,
    pub resolution_beats: f64,
    pub row: Arc<GrooveRow>,
    pub timing_amount: f32,
    /// Carried for eseq-groove.5; not applied in this slice.
    pub velocity_amount: f32,
    /// Carried for eseq-groove.5; not applied in this slice.
    pub random_amount: f32,
}

impl TrackGrooveSnapshot {
    /// The groove's offset at `boundary_beats` (transport beats, bar
    /// aligned), in beats, after the timing amount:
    ///
    /// ```text
    /// pos = boundary.rem_euclid(period) / resolution
    /// off = lerp(row[k].offset, row[k+1 mod n].offset, frac(pos)) * timing
    /// ```
    ///
    /// The interpolation is what lets a 16th groove shape 32nd hats or
    /// triplet-quantized neurons; two-slot swing is exactly this warp.
    /// Clamped to `>= 0` (late-only, eseq-groove.3 lifts the clamp).
    pub fn offset_beats(&self, boundary_beats: f64) -> f64 {
        let slots = &self.row.slots;
        let n = slots.len();
        if n == 0
            || !boundary_beats.is_finite()
            || !(self.period_beats > 0.0)
            || !(self.resolution_beats > 0.0)
        {
            return 0.0;
        }
        let mut pos = boundary_beats.rem_euclid(self.period_beats) / self.resolution_beats;
        let nearest = pos.round();
        if (pos - nearest).abs() < SLOT_SNAP {
            pos = nearest;
        }
        let k = pos.floor();
        let t = pos - k;
        let k = (k.max(0.0) as usize) % n;
        let next = (k + 1) % n;
        let a = slots[k].offset as f64;
        let b = slots[next].offset as f64;
        let offset_slots = (a + (b - a) * t) * self.timing_amount as f64;
        let beats = offset_slots * self.resolution_beats;
        if beats.is_finite() {
            beats.max(0.0)
        } else {
            0.0
        }
    }
}

/// The groove delay in samples for a trig whose straight boundary is
/// `boundary_beats`: the one timing function every trig site shares, so the
/// same pad at the same position moves by the same number of samples no
/// matter which source fired it.
pub fn groove_delay_samples(
    groove: &TrackGrooveSnapshot,
    boundary_beats: f64,
    samples_per_quarter: f64,
) -> u64 {
    if !(samples_per_quarter > 0.0) {
        return 0;
    }
    let samples = (groove.offset_beats(boundary_beats) * samples_per_quarter).round();
    if samples.is_finite() {
        samples.max(0.0) as u64
    } else {
        0
    }
}

/// The straight sample moved by the groove (spec's `grooved_sample_time`,
/// timing half).
pub fn grooved_sample_time(
    groove: &TrackGrooveSnapshot,
    boundary_beats: f64,
    straight_sample: u64,
    samples_per_quarter: f64,
) -> u64 {
    straight_sample.saturating_add(groove_delay_samples(
        groove,
        boundary_beats,
        samples_per_quarter,
    ))
}

// ---------------------------------------------------------------------------
// Built-in generic grooves
// ---------------------------------------------------------------------------

/// A generic groove shipped with the app: MPC-style swing as a two-slot,
/// shared-row-only groove.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinGroove {
    /// Stable id, the payload of `GrooveRef::Builtin`: `mpc-16-58`,
    /// `mpc-8-66`, ...
    pub id: String,
    pub groove: ProjectGroove,
}

/// The MPC swing amounts (the classic 50/54/58/62/66/71/75 ladder).
pub const BUILTIN_MPC_SWING_PERCENTS: [u32; 7] = [50, 54, 58, 62, 66, 71, 75];

/// One MPC swing as a groove: period = two slots at `resolution_beats`, the
/// off slot late by the same amount track swing would delay it,
/// `(pct/100 - 0.5) * 2` slots.
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

/// Every built-in generic groove, in picker order: MPC 16th swings, then 8th
/// swings.
pub fn builtin_grooves() -> &'static [BuiltinGroove] {
    static BUILTINS: std::sync::OnceLock<Vec<BuiltinGroove>> = std::sync::OnceLock::new();
    BUILTINS.get_or_init(|| {
        let mut out = Vec::new();
        for (label, resolution) in [("16", 0.25), ("8", 0.5)] {
            for percent in BUILTIN_MPC_SWING_PERCENTS {
                out.push(BuiltinGroove {
                    id: format!("mpc-{label}-{percent}"),
                    groove: mpc_swing_groove(percent, resolution),
                });
            }
        }
        out
    })
}

pub fn builtin_groove(id: &str) -> Option<&'static ProjectGroove> {
    builtin_grooves()
        .iter()
        .find(|builtin| builtin.id == id)
        .map(|builtin| &builtin.groove)
}

impl ProjectRackConfig {
    /// The groove the rack plays through: one of its own, or a built-in.
    /// `None` when nothing is active or the reference does not resolve.
    pub fn resolved_active_groove(&self) -> Option<&ProjectGroove> {
        match self.groove.active.as_ref()? {
            GrooveRef::Rack(id) => self.groove_by_id(*id),
            GrooveRef::Builtin(id) => builtin_groove(id),
        }
    }
}

/// The scheduler's per-track groove table for `num_tracks` tracks, from each
/// drum rack's `(members, config)`. Every member of a rack with an active,
/// well-formed groove gets an entry: its pad's own row when the groove has
/// one for the pad's note, else the shared row (a member without a pad, too).
/// Everything else is `None`, which the scheduler treats as "no groove".
pub fn track_groove_snapshots<'a>(
    racks: impl IntoIterator<Item = (&'a [usize], &'a ProjectRackConfig)>,
    num_tracks: usize,
) -> Vec<Option<TrackGrooveSnapshot>> {
    let mut out = vec![None; num_tracks];
    for (members, rack) in racks {
        let Some(groove) = rack.resolved_active_groove() else {
            continue;
        };
        if !groove.is_well_formed() {
            continue;
        }
        let shared = Arc::new(groove.shared_row.clone());
        let entry = |row: Arc<GrooveRow>| TrackGrooveSnapshot {
            period_beats: groove.period_beats,
            resolution_beats: groove.resolution_beats,
            row,
            timing_amount: rack.groove.timing_amount,
            velocity_amount: rack.groove.velocity_amount,
            random_amount: rack.groove.random_amount,
        };
        for (member, &track) in members.iter().enumerate() {
            let Some(slot) = out.get_mut(track) else {
                continue;
            };
            let pad_row = rack
                .pads
                .iter()
                .find(|pad| pad.member == member)
                .and_then(|pad| groove.pad_row(pad.pad_note));
            let row = match pad_row {
                Some(row) => Arc::new(row.clone()),
                None => Arc::clone(&shared),
            };
            *slot = Some(entry(row));
        }
    }
    out
}
