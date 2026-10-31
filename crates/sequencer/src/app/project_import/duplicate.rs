//! Duplicate a track (eseq-7pjq): a mirror import of one track from a
//! capture of this very project. The import is the live path (no restart:
//! new engines join, the playing scene relaunches onto the pinned lanes),
//! so the copy brings what an imported track brings — its Sound, every
//! scene's lane (steps, p-locks, mixer and track options), slot sets, lane
//! roster, mod cables — and, being a mirror, keeps its links to what stays
//! behind (output bus, sends, sidechains) instead of cutting them.

use super::{App, ImportRequest, ImportSource, ImportSummary, ImportTarget};

impl App {
    /// Append a copy of `track`, named `<name> copy`, in the plain group
    /// `track` is in, as ONE undo step. `finish` is the host's half, as in
    /// [`App::import_from_project_then`]. Returns the new track.
    pub fn duplicate_track_then(
        &mut self,
        track: usize,
        finish: impl FnOnce(&mut App, &ImportSummary) -> Vec<String>,
    ) -> Result<usize, String> {
        let Some(name) = self.tracks.get(track).cloned() else {
            return Err(format!("Track {} does not exist", track + 1));
        };
        if self.groups.iter().any(|group| group.is_rack() && group.members.contains(&track)) {
            return Err(format!("'{name}' is a drum rack pad; duplicate the rack's pad instead"));
        }
        if self.graph.track_instrument_types.get(track) == Some(&crate::sequencer::InstrumentType::Empty) {
            return Err(format!("'{name}' is an empty track: there is nothing to duplicate"));
        }
        let copy_name = super::super::first_free_name(&format!("{name} copy"), |candidate| {
            self.tracks.iter().any(|existing| existing == candidate)
        });
        let mut file = self.capture_project(&name)?;
        file.tracks[track].name = Some(copy_name);
        let source = ImportSource::from_file(&name, file)?;
        let request = ImportRequest {
            tracks: vec![track],
            target: ImportTarget::Mirror,
            ..Default::default()
        };
        let group = self
            .groups
            .iter()
            .position(|group| !group.is_rack() && group.members.contains(&track));
        let mut copy = None;
        self.import_from_project_then(&source, &request, |app, summary| {
            let mut notes = Vec::new();
            copy = summary.tracks.iter().find(|(old, _)| *old == track).map(|(_, new)| *new);
            if let (Some(new), Some(group)) = (copy, group) {
                if let Err(error) = app.move_track_to_group_recorded(new, group) {
                    notes.push(format!("The copy stayed out of the group: {error}"));
                }
            }
            notes.extend(finish(app, summary));
            notes
        })?;
        self.history.relabel_last("Duplicate track".to_string());
        copy.ok_or_else(|| format!("'{name}' was not duplicated"))
    }
}
