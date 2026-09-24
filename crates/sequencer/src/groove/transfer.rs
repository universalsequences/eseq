//! Moving grooves between racks (docs/rack-groove-spec.md §Data model, bead
//! eseq-groove.7): a saved kit carries its rack's grooves, and a groove
//! picked from ANOTHER rack is copied into the target rack's list.
//!
//! Both are the same operation: [`import_grooves`] merges incoming grooves
//! into a rack's list under fresh ids (reusing the id of an identical groove
//! already there, so auditioning a kit twice does not duplicate its pocket),
//! and [`install_groove_settings`] re-points an incoming selection through the
//! resulting id map.
//!
//! No per-pad remapping happens here, on purpose: pad rows key on
//! `pad_note`, and the scheduler table (`track_groove_snapshots`) resolves
//! each member through its OWN rack's pad note. So a copied groove plays a
//! target pad through the source row with the same note and every other pad
//! through the shared row; [`ProjectGroove::pad_row_mapping`] reports which.

use super::{GrooveId, GrooveRef, ProjectGroove, RackGrooveSettings};
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
    /// Same feel, whatever id either rack gave it: every field but `id`
    /// matches.
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
}

/// Merges `incoming` grooves into `rack.grooves` and returns
/// `(incoming id, id in the rack)` for every groove that landed.
///
/// - A malformed groove is skipped (and so absent from the map).
/// - A groove whose feel matches one already in the rack
///   ([`ProjectGroove::same_feel`]) maps to that one instead of duplicating.
/// - Anything else is appended under `rack.next_groove_id()`, so the rack's
///   own ids never collide with the source's.
pub fn import_grooves(
    rack: &mut ProjectRackConfig,
    incoming: &[ProjectGroove],
) -> Vec<(GrooveId, GrooveId)> {
    let mut map = Vec::with_capacity(incoming.len());
    for groove in incoming {
        if !groove.is_well_formed() || map.iter().any(|(from, _)| *from == groove.id) {
            continue;
        }
        let id = match rack.grooves.iter().find(|own| own.same_feel(groove)) {
            Some(own) => own.id,
            None => {
                let id = rack.next_groove_id();
                let mut copy = groove.clone();
                copy.id = id;
                rack.grooves.push(copy);
                id
            }
        };
        map.push((groove.id, id));
    }
    map
}

/// Installs an incoming groove selection and amounts on `rack`: a rack-groove
/// reference goes through `id_map` (from [`import_grooves`]); one that did not
/// land becomes `None`. Built-in references pass through unchanged. The
/// amounts are sanitized.
pub fn install_groove_settings(
    rack: &mut ProjectRackConfig,
    settings: &RackGrooveSettings,
    id_map: &[(GrooveId, GrooveId)],
) {
    let mut settings = settings.clone();
    if let Some(GrooveRef::Rack(from)) = settings.active {
        settings.active = id_map
            .iter()
            .find(|(source, _)| *source == from)
            .map(|(_, to)| GrooveRef::Rack(*to));
    }
    settings.sanitize();
    rack.groove = settings;
}
