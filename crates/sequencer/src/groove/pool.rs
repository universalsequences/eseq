//! The project groove pool (docs/rack-groove-spec.md §Groove pool, library
//! and pad roles, bead eseq-groove.9): every groove a project plays lives in
//! `ProjectFile::grooves` / `App::grooves`, and a rack only points into it
//! (`RackGrooveSettings::active`).
//!
//! Grooves enter the pool by extraction, from a library file (copy-on-apply)
//! or from a kit preset. The last two go through [`import_groove`], which
//! reuses a pool groove with the same feel ([`ProjectGroove::same_feel`])
//! instead of adding a duplicate, so applying a library groove twice, or
//! auditioning the same kit twice, does not pile up copies.
//!
//! No per-pad remapping happens on import, on purpose: pad rows key on
//! `pad_note`, and the scheduler table (`track_groove_snapshots`) resolves
//! each member through its OWN rack's pad note. So a groove extracted on one
//! kit plays a pad of another kit through the row with the same note and
//! every other pad through the shared row; [`ProjectGroove::pad_row_mapping`]
//! reports which.

use super::{GrooveId, ProjectGroove};
use crate::project::ProjectRackConfig;

/// Which row a pad plays through when a groove is applied to a rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrooveRowChoice {
    /// The groove has a row for this pad note.
    Pad,
    /// No row for this pad note: the all-pads shared row.
    Shared,
}

impl ProjectGroove {
    /// Same feel, whatever id either copy has: every field but `id` matches.
    pub fn same_feel(&self, other: &ProjectGroove) -> bool {
        self.name == other.name
            && self.period_beats == other.period_beats
            && self.resolution_beats == other.resolution_beats
            && self.pad_rows == other.pad_rows
            && self.shared_row == other.shared_row
    }

    /// For each of `pad_notes` (in order), the row it plays through.
    pub fn pad_row_mapping(&self, pad_notes: &[i32]) -> Vec<GrooveRowChoice> {
        pad_notes
            .iter()
            .map(|&note| {
                if self.pad_row(note).is_some() {
                    GrooveRowChoice::Pad
                } else {
                    GrooveRowChoice::Shared
                }
            })
            .collect()
    }
}

impl ProjectRackConfig {
    /// For each of the rack's pads (in pad order), the row `groove` plays it
    /// through.
    pub fn groove_row_mapping(&self, groove: &ProjectGroove) -> Vec<GrooveRowChoice> {
        let notes = self.pads.iter().map(|pad| pad.pad_note).collect::<Vec<_>>();
        groove.pad_row_mapping(&notes)
    }

    /// Load-time repair of the rack's groove selection against the project
    /// `pool`: an active id that is not in the pool turns the groove off, and
    /// the amounts are clamped. Returns whether anything changed.
    pub fn repair_groove_selection(&mut self, pool: &[ProjectGroove]) -> bool {
        let before = self.groove.clone();
        if self
            .groove
            .active
            .is_some_and(|id| pool_groove(pool, id).is_none())
        {
            self.groove.active = None;
        }
        self.groove.sanitize();
        before != self.groove
    }
}

pub fn pool_groove(pool: &[ProjectGroove], id: GrooveId) -> Option<&ProjectGroove> {
    pool.iter().find(|groove| groove.id == id)
}

/// The id the next pool groove takes: one past the largest in use, never 0.
pub fn next_pool_groove_id(pool: &[ProjectGroove]) -> GrooveId {
    pool.iter().map(|groove| groove.id).max().unwrap_or(0) + 1
}

/// Copy-on-apply: `groove` (a library file, a kit's copy) enters the pool,
/// and the pool id to point a rack at comes back. A pool groove with the same
/// feel is reused instead of duplicated; anything else is appended under
/// [`next_pool_groove_id`] (the incoming id means nothing in this project).
/// `None` for a malformed groove, which is never trusted by the scheduler.
pub fn import_groove(pool: &mut Vec<ProjectGroove>, groove: &ProjectGroove) -> Option<GrooveId> {
    if !groove.is_well_formed() {
        return None;
    }
    if let Some(existing) = pool.iter().find(|own| own.same_feel(groove)) {
        return Some(existing.id);
    }
    let id = next_pool_groove_id(pool);
    let mut copy = groove.clone();
    copy.id = id;
    pool.push(copy);
    Some(id)
}

/// [`import_groove`] for several grooves; returns `(incoming id, pool id)`
/// for every groove that landed (a repeated incoming id is imported once).
pub fn import_grooves(
    pool: &mut Vec<ProjectGroove>,
    incoming: &[ProjectGroove],
) -> Vec<(GrooveId, GrooveId)> {
    let mut map: Vec<(GrooveId, GrooveId)> = Vec::with_capacity(incoming.len());
    for groove in incoming {
        if map.iter().any(|(from, _)| *from == groove.id) {
            continue;
        }
        if let Some(id) = import_groove(pool, groove) {
            map.push((groove.id, id));
        }
    }
    map
}

/// Load-time repair of the pool: drops malformed grooves, id 0 and duplicate
/// ids (keeping the first). Returns whether anything was dropped.
pub fn repair_groove_pool(pool: &mut Vec<ProjectGroove>) -> bool {
    let before = pool.len();
    let mut seen = Vec::with_capacity(pool.len());
    pool.retain(|groove| {
        let keep = groove.id != 0 && groove.is_well_formed() && !seen.contains(&groove.id);
        seen.push(groove.id);
        keep
    });
    pool.len() != before
}
