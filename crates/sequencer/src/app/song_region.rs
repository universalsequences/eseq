//! Arrangement region selection — a time x track rectangle over the song
//! (docs/arrangement-region-editing-spec.md 4).
//!
//! The region is Rust-owned for the same reasons the bound clip is
//! (`sound_binding::SongClipSelection`): it must survive a view switch and a
//! buffer reload, and the copy/paste/delete/move primitives read it directly
//! rather than being handed a rectangle by the UI script. The Lisp side owns
//! only the transient in-drag ghost.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::sequencer::{project_lanes, LaneSource, TakeId, TrackPatternData};

use super::edit::finish_active_gesture;
use super::history::{EditPatch, SceneStructurePatch, SongStructurePatch};
use super::App;

/// A committed region selection. Track indices are MODEL indices and both
/// ends are inclusive; the beat span is half-open `[start_beat, end_beat)`,
/// matching how the song's lane spans are addressed everywhere else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongRegionSelection {
    pub track_a: usize,
    pub track_b: usize,
    pub start_beat: f64,
    pub end_beat: f64,
}

impl SongRegionSelection {
    /// Normalizing constructor: callers pass the two ends of a drag in
    /// whatever order the pointer produced them.
    pub fn new(track_a: usize, track_b: usize, start_beat: f64, end_beat: f64) -> Self {
        Self {
            track_a: track_a.min(track_b),
            track_b: track_a.max(track_b),
            start_beat: start_beat.min(end_beat).max(0.0),
            end_beat: start_beat.max(end_beat).max(0.0),
        }
    }

    /// A region that selects no time selects nothing at all — the callers
    /// treat this as "clear" rather than storing an empty rectangle.
    pub fn is_empty(&self) -> bool {
        self.end_beat <= self.start_beat
    }

    pub fn contains_track(&self, track: usize) -> bool {
        track >= self.track_a && track <= self.track_b
    }
}

/// One copied clip span, in beats RELATIVE to the copied region's start
/// (region spec 5.1). Only sounding spans are stored: gaps are implicit and
/// paste silences them, because the clipboard is the whole rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipboardSpan {
    pub rel_start: f64,
    pub rel_end: f64,
    /// `Pattern` pastes as a reference; `Take` pastes as a fresh clone
    /// (region spec 5.1 locked decision). `Empty` never appears.
    pub source: LaneSource,
    /// The source offset AT `rel_start` — already advanced past the cut when
    /// the copy boundary sliced into the middle of a clip, so the pasted
    /// result plays the identical slice.
    pub offset_steps: f64,
}

/// The arrangement clipboard (region spec 5.1): one time × track rectangle,
/// lifted out of the committed song. Track indices are ABSOLUTE model
/// indices — paste is same-track, time-shift only, and validates that each
/// index still resolves.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrangementClipboard {
    pub len_beats: f64,
    /// Grid the copied rectangle sits on; paste floors its destination to it
    /// so a pasted bar lands on a bar. `0.0` means "paste exactly where told".
    pub snap_beats: f64,
    /// Absolute model track index → its spans, in beat order. A track that
    /// was silent throughout the region still appears, with no spans, so
    /// paste silences it.
    pub tracks: Vec<(usize, Vec<ClipboardSpan>)>,
}

impl ArrangementClipboard {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() || !(self.len_beats > 0.0)
    }

    /// Number of sounding spans, for the status line.
    pub fn span_count(&self) -> usize {
        self.tracks.iter().map(|(_, spans)| spans.len()).sum()
    }

    /// Floor a paste destination onto the grid the copied rectangle sat on,
    /// so an unsnapped cursor click still lands musically.
    pub fn floor_destination(&self, dest_beat: f64) -> f64 {
        if self.snap_beats > 0.0 {
            (dest_beat / self.snap_beats).floor() * self.snap_beats
        } else {
            dest_beat
        }
    }
}

/// Shared handle, held by the UI loop next to the piano-roll clipboard.
pub type ArrangementClipboardHandle = Arc<Mutex<Option<ArrangementClipboard>>>;

pub fn new_arrangement_clipboard() -> ArrangementClipboardHandle {
    Arc::new(Mutex::new(None))
}

/// Grid rungs a copied rectangle can sit on, coarsest first (beats). Capped
/// at one bar: a longer copy still snaps to the bar, never to its own length,
/// or pasting a 4-bar block would jump the destination four bars back.
const CLIPBOARD_SNAP_LADDER: [f64; 6] = [4.0, 2.0, 1.0, 0.5, 0.25, 0.125];

fn divides(value: f64, grid: f64) -> bool {
    let rem = (value / grid).fract().abs();
    rem < 1e-6 || rem > 1.0 - 1e-6
}

/// The coarsest ladder rung that divides both the region's start and its
/// length: copying bars 5–9 (start 16, length 16) yields a 4-beat snap, so
/// pasting at a click anywhere inside a bar lands on that bar's downbeat.
/// A region that fits no rung pastes unsnapped.
fn region_snap_beats(start_beat: f64, len_beats: f64) -> f64 {
    CLIPBOARD_SNAP_LADDER
        .iter()
        .copied()
        .find(|grid| divides(start_beat, *grid) && divides(len_beats, *grid))
        .unwrap_or(0.0)
}

impl App {
    /// Advance a lane source's offset by `delta_beats` of playback, in the
    /// source's own step domain: patterns wrap at the pattern length, takes
    /// advance linearly and never wrap (takes spec 6.1/7.4).
    fn advanced_source_offset(
        &self,
        track: usize,
        source: LaneSource,
        offset_steps: f64,
        delta_beats: f64,
    ) -> f64 {
        match source {
            LaneSource::Empty => 0.0,
            LaneSource::Pattern(pattern) => {
                self.advanced_offset(track, pattern.0, offset_steps, delta_beats)
            }
            LaneSource::Take(take) => match self.take_step_mapping(track, take.0) {
                Some((steps_per_beat, _)) => {
                    (offset_steps + delta_beats * steps_per_beat).max(0.0)
                }
                None => offset_steps,
            },
        }
    }

    /// One track's lane spans clipped to `[start_beat, end_beat)`, in ABSOLUTE
    /// beats, as `(start, end, source, offset_at_start)`.
    ///
    /// A clip the window cuts into gets its offset ADVANCED to the cut, so the
    /// span plays the identical slice wherever it is re-stamped — the property
    /// both copy (region spec 5.1) and the per-track ripple depend on.
    /// `keep_empty` decides whether silence travels as explicit spans (the
    /// ripple needs it; the clipboard treats gaps as implicit).
    fn lane_spans_in(
        &self,
        lanes: &[Vec<crate::sequencer::LaneClip>],
        track: usize,
        start_beat: f64,
        end_beat: f64,
        keep_empty: bool,
    ) -> Vec<(f64, f64, LaneSource, f64)> {
        let Some(lane) = lanes.get(track) else {
            return Vec::new();
        };
        lane.iter()
            .filter_map(|clip| {
                let span_start = clip.start_beat.max(start_beat);
                let span_end = clip.end_beat.min(end_beat);
                if span_end - span_start <= 1e-9 {
                    return None;
                }
                if !keep_empty && matches!(clip.source, LaneSource::Empty) {
                    return None;
                }
                let cut_beats = span_start - clip.start_beat;
                let offset_steps = if cut_beats <= 1e-9 {
                    clip.offset_steps
                } else {
                    self.advanced_source_offset(track, clip.source, clip.offset_steps, cut_beats)
                };
                Some((span_start, span_end, clip.source, offset_steps))
            })
            .collect()
    }

    /// Lift the selected region out of the COMMITTED song into a clipboard
    /// (region spec 5.1). Read-only: no mutation, no history entry.
    ///
    /// Reads `song_region_selection` directly and does not care which gesture
    /// produced it — a free marquee and a clip click both set it (spec 4.1 as
    /// amended), so a selected clip copies as a one-clip rectangle.
    pub fn song_region_copy(&self) -> Result<ArrangementClipboard, String> {
        let region = self
            .song_region_selection
            .ok_or_else(|| "No arrangement region is selected".to_string())?;
        let song = self
            .state
            .committed_song()
            .ok_or_else(|| "The project has no song".to_string())?;
        let start_beat = region.start_beat;
        let end_beat = region.end_beat.min(song.end_beat);
        if end_beat <= start_beat {
            return Err("The selected region has no length".to_string());
        }
        let scenes = self.state.capture_project_scenes();
        let lanes = project_lanes(&song, &scenes);

        let mut tracks = Vec::new();
        for track in region.track_a..=region.track_b {
            if track >= lanes.len() {
                continue;
            }
            // Silent tracks stay in the rectangle (with no spans) so paste
            // silences them: the clipboard is the whole rectangle.
            let spans = self
                .lane_spans_in(&lanes, track, start_beat, end_beat, false)
                .into_iter()
                .map(|(span_start, span_end, source, offset_steps)| ClipboardSpan {
                    rel_start: span_start - start_beat,
                    rel_end: span_end - start_beat,
                    source,
                    offset_steps,
                })
                .collect();
            tracks.push((track, spans));
        }
        if tracks.is_empty() {
            return Err("The selected region covers no existing track".to_string());
        }
        let len_beats = end_beat - start_beat;
        Ok(ArrangementClipboard {
            len_beats,
            snap_beats: region_snap_beats(start_beat, len_beats),
            tracks,
        })
    }

    /// Paste the clipboard rectangle at `dest_beat` (region spec 5.2): one
    /// clone of the committed song, one undo entry.
    ///
    /// Per clipboard track: explicit-empty over the whole destination span
    /// first (gaps are silence), then each stored span with its offset,
    /// anchored at its pasted start. Pattern sources paste as REFERENCES;
    /// take sources are CLONED once per source take (new `TakeId`, chunk
    /// patterns deep-copied) so later per-clip editing of a pasted take never
    /// rewrites the original. A source take that has since been deleted is
    /// skipped rather than failing the paste.
    pub fn song_region_paste(
        &mut self,
        clipboard: &ArrangementClipboard,
        dest_beat: f64,
    ) -> Result<String, String> {
        self.require_song_edit_unlocked()?;
        if clipboard.is_empty() {
            return Err("The arrangement clipboard is empty".to_string());
        }
        if !dest_beat.is_finite() || dest_beat < 0.0 {
            return Err(format!(
                "Paste destination must be a finite, non-negative beat (got {dest_beat})"
            ));
        }
        let dest_beat = clipboard.floor_destination(dest_beat);
        let song_before = self.state.committed_song();
        let Some(existing) = song_before.clone() else {
            return Err("The project has no song".to_string());
        };

        let (scenes_before, cloned_takes) = self.clone_clipboard_takes(clipboard)?;
        let build = (|| -> Result<crate::sequencer::ProjectSong, String> {
            let mut song = existing.clone();
            // The pasted rectangle may run past the song end; extending it
            // rides inside this same commit (region spec 5.2).
            song.end_beat = song.end_beat.max(dest_beat + clipboard.len_beats);
            self.paint_clipboard(&mut song, clipboard, dest_beat, &cloned_takes)?;
            song.normalize();
            self.collapse_phase_continuation_rows(&mut song);
            Ok(song)
        })();
        self.commit_region_edit("Paste region", song_before, build, scenes_before)?;

        let span_count = clipboard.span_count();
        let track_count = clipboard.tracks.len();
        Ok(format!(
            "Pasted {span_count} clip{} across {track_count} track{} at beat {dest_beat}",
            if span_count == 1 { "" } else { "s" },
            if track_count == 1 { "" } else { "s" },
        ))
    }

    /// Duplicate the selected region immediately after itself, RIPPLING what
    /// follows right by the region's length (Ableton's Duplicate Time). One
    /// undo entry, and the selection follows the copy so repeated Cmd-D
    /// chains.
    ///
    /// Only the SELECTED tracks move; every other lane keeps playing at the
    /// beats it always did. Two mechanisms, same audible contract:
    ///
    /// - **Region covers every track** — shift the song ROWS right. The rows
    ///   are the shared time boundaries, so moving them moves everything at
    ///   once and the song stays scene-resolved (no override churn). This is
    ///   the "insert 4 bars into my song" gesture.
    /// - **Region covers some tracks** — re-paint just those lanes' tails
    ///   `len` beats later. The rows stay put; untouched lanes split
    ///   phase-transparently underneath (`split_row_state`) and sound
    ///   identical. The cost is that a rippled lane's tail becomes explicit
    ///   overrides, so it stops following its scene cells — unavoidable,
    ///   since its content no longer lines up with the rows' scenes.
    ///
    /// Either way the song grows by `len`; the appended tail is governed by
    /// whatever each lane was playing at the boundary. Take sources duplicate
    /// as fresh clones exactly as paste's do.
    pub fn song_region_duplicate(&mut self) -> Result<String, String> {
        self.require_song_edit_unlocked()?;
        let region = self
            .song_region_selection
            .ok_or_else(|| "No arrangement region is selected".to_string())?;
        let clipboard = self.song_region_copy()?;
        let song_before = self.state.committed_song();
        let Some(existing) = song_before.clone() else {
            return Err("The project has no song".to_string());
        };
        let len_beats = clipboard.len_beats;
        // The duplicate lands where the region ends; the copy already clamped
        // that to the song end, so it is always inside the song.
        let insert_beat = region.end_beat.min(existing.end_beat);

        // Whole-timeline shift only when the region really covers everything;
        // otherwise the untouched lanes must not move.
        let covers_every_track =
            region.track_a == 0 && region.track_b + 1 >= self.tracks.len().max(1);

        // Read the rippled lanes' tails BEFORE any mutation. Silence travels
        // too: a gap that slides right must leave silence behind it, not the
        // clip it used to sit next to.
        let tails: Vec<(usize, Vec<(f64, f64, LaneSource, f64)>)> = if covers_every_track {
            Vec::new()
        } else {
            let scenes = self.state.capture_project_scenes();
            let lanes = project_lanes(&existing, &scenes);
            clipboard
                .tracks
                .iter()
                .map(|(track, _)| {
                    (
                        *track,
                        self.lane_spans_in(&lanes, *track, insert_beat, existing.end_beat, true),
                    )
                })
                .collect()
        };

        let (scenes_before, cloned_takes) = self.clone_clipboard_takes(&clipboard)?;
        let build = (|| -> Result<crate::sequencer::ProjectSong, String> {
            let mut song = existing.clone();
            if covers_every_track {
                // Split first so the shifted suffix starts on a real row whose
                // offsets already describe the music at the boundary; the
                // shift is then phase-rigid (takes spec 7.4).
                if insert_beat < song.end_beat {
                    self.split_song_row_at(&mut song, insert_beat)?;
                }
                for row in &mut song.rows {
                    if row.start_beat >= insert_beat - 1e-9 {
                        row.start_beat += len_beats;
                    }
                }
                song.end_beat += len_beats;
            } else {
                song.end_beat += len_beats;
                let track_count = self.tracks.len();
                for (track, tail) in &tails {
                    if *track >= track_count {
                        continue;
                    }
                    // Re-stamp each tail span `len` beats later with the
                    // offset it had at its own start: same music, later.
                    for (span_start, span_end, source, offset_steps) in tail {
                        self.paint_source_region(
                            &mut song,
                            *track,
                            span_start + len_beats,
                            span_end + len_beats,
                            *source,
                            span_start + len_beats,
                            *offset_steps,
                        )?;
                    }
                }
            }
            // Both paths leave exactly [insert, insert + len) vacated on the
            // rippled lanes; the duplicate fills it.
            self.paint_clipboard(&mut song, &clipboard, insert_beat, &cloned_takes)?;
            song.normalize();
            self.collapse_phase_continuation_rows(&mut song);
            Ok(song)
        })();
        self.commit_region_edit("Duplicate region", song_before, build, scenes_before)?;

        // The highlight follows the copy, so Cmd-D again duplicates THAT.
        // It names no single clip any more, so it goes through the marquee
        // door (spec 4.1) and hands the sound binding back.
        self.set_song_region(SongRegionSelection::new(
            region.track_a,
            region.track_b,
            insert_beat,
            insert_beat + len_beats,
        ));
        let scope = if covers_every_track {
            "pushing the song right"
        } else {
            "pushing those tracks right"
        };
        Ok(format!(
            "Duplicated beats {}-{insert_beat} to {insert_beat}, {scope}",
            region.start_beat
        ))
    }

    /// Mint the paste-time clones of every take source in the clipboard, once
    /// per `(track, take)` — a take clip split across several rows must land
    /// as ONE take, not one per row. Returns the pre-mutation scene state when
    /// anything was cloned, so the caller can roll back and knows to commit a
    /// composite patch. Sources deleted since the copy are simply absent.
    fn clone_clipboard_takes(
        &mut self,
        clipboard: &ArrangementClipboard,
    ) -> Result<
        (
            Option<crate::sequencer::ProjectScenes>,
            HashMap<(usize, u64), TakeId>,
        ),
        String,
    > {
        let mut cloned_takes: HashMap<(usize, u64), TakeId> = HashMap::new();
        let needs_clones = clipboard.tracks.iter().any(|(_, spans)| {
            spans
                .iter()
                .any(|span| matches!(span.source, LaneSource::Take(_)))
        });
        if !needs_clones {
            return Ok((None, cloned_takes));
        }
        let scenes_before = self.capture_synchronized_scene_structure_state()?;
        for (track, spans) in &clipboard.tracks {
            for span in spans {
                let LaneSource::Take(source_take) = span.source else {
                    continue;
                };
                if cloned_takes.contains_key(&(*track, source_take.0)) {
                    continue;
                }
                match self.clone_take_for_paste(*track, source_take) {
                    Ok(Some(clone)) => {
                        cloned_takes.insert((*track, source_take.0), clone);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.restore_scene_structure_state(&scenes_before)?;
                        return Err(error);
                    }
                }
            }
        }
        Ok((Some(scenes_before), cloned_takes))
    }

    /// Stamp the clipboard rectangle onto `song` at `dest_beat`: silence the
    /// whole destination span per track (gaps are silence), then paint each
    /// stored span anchored at its own pasted start. Take spans paint their
    /// clone, or nothing when the source was deleted.
    fn paint_clipboard(
        &self,
        song: &mut crate::sequencer::ProjectSong,
        clipboard: &ArrangementClipboard,
        dest_beat: f64,
        cloned_takes: &HashMap<(usize, u64), TakeId>,
    ) -> Result<(), String> {
        let dest_end = dest_beat + clipboard.len_beats;
        let track_count = self.tracks.len();
        for (track, spans) in &clipboard.tracks {
            // The clipboard stores absolute track indices; one that no longer
            // resolves is skipped (region spec 5.1 locked decision).
            if *track >= track_count {
                continue;
            }
            self.paint_source_region(
                song,
                *track,
                dest_beat,
                dest_end,
                LaneSource::Empty,
                dest_beat,
                0.0,
            )?;
            for span in spans {
                let source = match span.source {
                    LaneSource::Take(source_take) => {
                        match cloned_takes.get(&(*track, source_take.0)) {
                            Some(clone) => LaneSource::Take(*clone),
                            None => continue,
                        }
                    }
                    other => other,
                };
                let start = dest_beat + span.rel_start;
                let end = dest_beat + span.rel_end;
                self.paint_source_region(
                    song,
                    *track,
                    start,
                    end,
                    source,
                    start,
                    span.offset_steps,
                )?;
            }
        }
        Ok(())
    }

    /// Shared tail for the clipboard primitives: validate the built song,
    /// install it, and commit ONE history entry — `EditPatch::Song` when no
    /// takes were cloned, else a composite pairing the scene patch with it
    /// (scenes first, ordering per `song_region_to_take`). Any failure rolls
    /// the take clones back and leaves the committed song untouched.
    fn commit_region_edit(
        &mut self,
        label: &'static str,
        song_before: Option<crate::sequencer::ProjectSong>,
        build: Result<crate::sequencer::ProjectSong, String>,
        scenes_before: Option<crate::sequencer::ProjectScenes>,
    ) -> Result<(), String> {
        let song_after = match build {
            Ok(song) => song,
            Err(error) => {
                if let Some(scenes) = &scenes_before {
                    self.restore_scene_structure_state(scenes)?;
                }
                return Err(error);
            }
        };
        {
            let scenes = self.state.capture_project_scenes();
            if let Err(error) = song_after.validate(&scenes) {
                if let Some(scenes) = &scenes_before {
                    self.restore_scene_structure_state(scenes)?;
                }
                return Err(format!("{label} produced an invalid song: {error}"));
            }
        }
        self.state.set_committed_song(Some(song_after.clone()));
        let song_patch = SongStructurePatch {
            before: song_before,
            after: Some(song_after),
        };
        match scenes_before {
            None => {
                let retained_bytes = song_patch.retained_bytes();
                finish_active_gesture(self);
                self.history
                    .commit(label, None, EditPatch::Song(song_patch), retained_bytes);
            }
            Some(scenes_before) => {
                let scenes_after = self.state.capture_project_scenes();
                finish_active_gesture(self);
                let scene_patch = SceneStructurePatch {
                    before: scenes_before,
                    after: scenes_after,
                };
                let retained_bytes = scene_patch.retained_bytes() + song_patch.retained_bytes();
                self.history.commit(
                    label,
                    None,
                    EditPatch::Composite(vec![
                        EditPatch::SceneStructure(scene_patch),
                        EditPatch::Song(song_patch),
                    ]),
                    retained_bytes,
                );
            }
        }
        Ok(())
    }

    /// Deep-copy a take into a fresh one on the same track (region spec 5.1):
    /// a new `TakeId` over freshly minted chunk patterns, named after the
    /// source. `Ok(None)` when the source take no longer exists.
    fn clone_take_for_paste(
        &mut self,
        track: usize,
        take_id: TakeId,
    ) -> Result<Option<TakeId>, String> {
        let source = self.state.with_project_scenes(|scenes| {
            let take = scenes.take_pools.get(track)?.get(take_id)?;
            let chunks: Option<Vec<TrackPatternData>> = take
                .chunks
                .iter()
                .map(|chunk| scenes.track_pools.get(track)?.get(*chunk).cloned())
                .collect();
            Some((take.name.clone(), chunks?, take.total_len_steps))
        });
        let Some((name, chunks, total_len_steps)) = source else {
            return Ok(None);
        };
        self.state
            .register_track_take(track, Some(format!("{name} copy")), chunks, total_len_steps)
            .map(Some)
    }

    /// Silence the selected region on every track it covers (region spec
    /// 5.2): explicit-empty overrides, one undo entry. This is what
    /// multi-track Backspace lowers to.
    pub fn song_region_delete(&mut self) -> Result<String, String> {
        self.require_song_edit_unlocked()?;
        let region = self
            .song_region_selection
            .ok_or_else(|| "No arrangement region is selected".to_string())?;
        let song_before = self.state.committed_song();
        let Some(existing) = song_before.clone() else {
            return Err("The project has no song".to_string());
        };
        let start_beat = region.start_beat;
        let end_beat = region.end_beat.min(existing.end_beat);
        if end_beat <= start_beat {
            return Err("The selected region has no length".to_string());
        }

        let mut song = existing.clone();
        let track_count = self.tracks.len();
        let mut painted = 0usize;
        for track in region.track_a..=region.track_b {
            if track >= track_count {
                continue;
            }
            self.paint_source_region(
                &mut song,
                track,
                start_beat,
                end_beat,
                LaneSource::Empty,
                start_beat,
                0.0,
            )?;
            painted += 1;
        }
        if painted == 0 {
            return Err("The selected region covers no existing track".to_string());
        }
        song.normalize();
        self.collapse_phase_continuation_rows(&mut song);
        if song.rows == existing.rows && song.end_beat == existing.end_beat {
            return Ok("The region is already empty".to_string());
        }
        self.commit_song_edit("Delete region", song_before, Some(song))?;
        Ok(format!(
            "Deleted beats {start_beat}-{end_beat} on {painted} track{}",
            if painted == 1 { "" } else { "s" }
        ))
    }

    /// Mirror of the arrangement edit cursor (region spec 5.3): the Lisp view
    /// owns the gesture, but Cmd-V is handled Rust-side and needs a paste
    /// target that survives the trip.
    pub fn set_arrangement_cursor(&mut self, beat: f64, track: isize) {
        if !beat.is_finite() || beat < 0.0 {
            return;
        }
        self.arrangement_cursor_beat = beat;
        self.arrangement_cursor_track = track;
    }

    /// Set the region (spec 4.1). Returns true when it actually changed.
    ///
    /// The region names no single clip, so it takes the selection channel
    /// away from the clip selection and RELEASES the sound binding — the same
    /// rule scene-lane selections follow (takes spec 16.6 cause 2). An empty
    /// or degenerate rectangle clears instead of committing nothing-selected.
    pub fn set_song_region(&mut self, region: SongRegionSelection) -> bool {
        if region.is_empty() {
            return self.clear_song_region();
        }
        // Order matters only for the binding resync: dropping the clip
        // selection re-resolves every track's bound source once.
        self.set_song_clip_selection(None);
        if self.song_region_selection == Some(region) {
            return false;
        }
        self.song_region_selection = Some(region);
        true
    }

    /// Set the region as the footprint of the clip that was just selected,
    /// WITHOUT releasing the clip selection.
    ///
    /// Clicking a clip's title bar selects the clip *and* selects its span as
    /// a one-clip region (Ableton: a selected clip is a selected region, which
    /// is what makes copy/delete on it mean anything). Only a free marquee —
    /// which names no single clip — takes the binding away, via
    /// `set_song_region`.
    pub fn set_song_region_for_clip(&mut self, region: SongRegionSelection) -> bool {
        if region.is_empty() {
            return self.clear_song_region();
        }
        if self.song_region_selection == Some(region) {
            return false;
        }
        self.song_region_selection = Some(region);
        true
    }

    /// Clear the region (spec 4.1). Returns true when it actually changed.
    /// Does NOT touch the sound binding: clearing a region leaves the tracks
    /// on whatever the playback/scene rules resolve, which is where the
    /// region put them.
    pub fn clear_song_region(&mut self) -> bool {
        if self.song_region_selection.is_none() {
            return false;
        }
        self.song_region_selection = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::edit::{redo, undo};
    use crate::app::song_edit::SongRowSpec;
    use crate::app::sound_binding::tests::app_with_take;
    use crate::app::sound_binding::BoundSource;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, PatternId, PatternSnapshot, SequencerState, SongRowId,
        StepParam,
    };

    /// `tracks` tracks, three scenes; per-track pool ids are 1..=3 with scene
    /// j's cell holding `PatternId(j + 1)`. Every pattern is 16 sixteenth
    /// steps (4 beats) with transpose `track * 10 + pool id`, so a pasted
    /// clip's provenance is readable from the content.
    fn multi_track_app(tracks: usize) -> App {
        let state = SequencerState::new(
            tracks,
            (0..tracks).map(|_| default_empty_effect_chain()).collect(),
        );
        state.replace_pattern_repository(
            vec![
                PatternSnapshot::new_default(tracks, &[]),
                PatternSnapshot::new_default(tracks, &[]),
                PatternSnapshot::new_default(tracks, &[]),
            ],
            0,
        );
        state.with_scenes_mut(|scenes| {
            for track in 0..tracks {
                for id in 1..=3u64 {
                    let data = scenes.track_pools[track]
                        .get_mut(PatternId(id))
                        .expect("pool pattern");
                    data.track_params.num_steps = 16;
                    for step in 0..16 {
                        data.track_bits[step / 64] |= 1 << (step % 64);
                        data.step_data[step][StepParam::Transpose.index()] =
                            (track as u64 * 10 + id) as f32;
                    }
                }
            }
        });
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let mut app = App::new(
            Arc::new(state),
            LiveGraphPtr(std::ptr::null_mut()),
            44_100,
            AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_gate_runtime: Arc::new(Mutex::new(Vec::new())),
                bus_gate_playheads: Arc::new(Mutex::new(Vec::new())),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        app.tracks = (1..=tracks).map(|n| format!("Track {n}")).collect();
        app.track_registry =
            crate::sequencer::TrackRegistry::for_legacy_track_count(tracks).unwrap();
        app
    }

    fn app_with_song() -> App {
        app_with_song_tracks(2)
    }

    /// Three-row song: 0.0 scene 0, 4.0 scene 1, 8.0 scene 2, end 16. Each
    /// scene resolves every track to its own pattern, so every 4-beat span is
    /// one clip on each lane.
    fn app_with_song_tracks(tracks: usize) -> App {
        let mut app = multi_track_app(tracks);
        app.song_replace(
            vec![
                SongRowSpec {
                    start_beat: 0.0,
                    scene: 0,
                    overrides: Vec::new(),
                },
                SongRowSpec {
                    start_beat: 4.0,
                    scene: 1,
                    overrides: Vec::new(),
                },
                SongRowSpec {
                    start_beat: 8.0,
                    scene: 2,
                    overrides: Vec::new(),
                },
            ],
            16.0,
            false,
        )
        .expect("song_replace succeeds");
        app
    }

    /// The lane projection of `track` as (start, end, source, offset) tuples,
    /// which is what "plays identically" means for these tests.
    fn lane(app: &App, track: usize) -> Vec<(f64, f64, LaneSource, f64)> {
        let song = app.state.committed_song().expect("song");
        let scenes = app.state.capture_project_scenes();
        project_lanes(&song, &scenes)[track]
            .iter()
            .map(|clip| (clip.start_beat, clip.end_beat, clip.source, clip.offset_steps))
            .collect()
    }

    /// Merge adjacent spans that continue the same source without a phase
    /// jump, so a lane read before and after a paste compares structurally
    /// even when internal row splits differ.
    fn merged_lane(app: &App, track: usize) -> Vec<(f64, f64, LaneSource, f64)> {
        let mut merged: Vec<(f64, f64, LaneSource, f64)> = Vec::new();
        for span in lane(app, track) {
            match merged.last_mut() {
                Some(prev)
                    if prev.2 == span.2
                        && (prev.1 - span.0).abs() < 1e-9
                        && (app.advanced_source_offset(track, prev.2, prev.3, prev.1 - prev.0)
                            - span.3)
                            .abs()
                            < 1e-6 =>
                {
                    prev.1 = span.1;
                }
                _ => merged.push(span),
            }
        }
        merged
    }

    /// The merged lane inside `[start, end)`, with times made RELATIVE to
    /// `start`: the shape a copied rectangle and its pasted twin must share.
    fn window(app: &App, track: usize, start: f64, end: f64) -> Vec<(f64, f64, LaneSource, f64)> {
        merged_lane(app, track)
            .into_iter()
            .filter_map(|(clip_start, clip_end, source, offset)| {
                let span_start = clip_start.max(start);
                let span_end = clip_end.min(end);
                if span_end - span_start <= 1e-9 {
                    return None;
                }
                let cut = span_start - clip_start;
                let offset = if cut <= 1e-9 {
                    offset
                } else {
                    app.advanced_source_offset(track, source, offset, cut)
                };
                Some((span_start - start, span_end - start, source, offset))
            })
            .collect()
    }

    fn span(rel_start: f64, rel_end: f64, pattern: u64, offset_steps: f64) -> ClipboardSpan {
        ClipboardSpan {
            rel_start,
            rel_end,
            source: LaneSource::Pattern(PatternId(pattern)),
            offset_steps,
        }
    }

    /// 5.1: the clipboard is the whole rectangle — one entry per covered
    /// track, sounding spans only, relative beats, and a grid derived from
    /// the copied rectangle. Copy never touches history.
    #[test]
    fn copy_lifts_the_rectangle_without_touching_history() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let depth = app.history.undo_len();

        let clipboard = app.song_region_copy().expect("copy succeeds");
        assert_eq!(clipboard.len_beats, 8.0);
        assert_eq!(clipboard.snap_beats, 4.0, "start 4 and length 8 sit on bars");
        assert_eq!(
            clipboard.tracks.iter().map(|(track, _)| *track).collect::<Vec<_>>(),
            vec![0, 1]
        );
        for (_, spans) in &clipboard.tracks {
            assert_eq!(
                spans,
                &vec![span(0.0, 4.0, 2, 0.0), span(4.0, 8.0, 3, 0.0)],
                "scene 1 then scene 2, anchored at their own starts"
            );
        }
        assert_eq!(app.history.undo_len(), depth, "copy is read-only");
    }

    /// 5.1: a clip the copy boundary cuts into stores its offset ADVANCED to
    /// the cut, so the pasted result plays the identical slice rather than
    /// restarting the pattern.
    #[test]
    fn copy_advances_the_offset_of_a_boundary_cut_clip() {
        let mut app = app_with_song();
        // [6, 10) cuts into scene 1's clip 2 beats in (8 sixteenth steps) and
        // out of scene 2's clip 2 beats before its end.
        app.set_song_region(SongRegionSelection::new(0, 0, 6.0, 10.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        assert_eq!(clipboard.len_beats, 4.0);
        assert_eq!(clipboard.snap_beats, 2.0, "start 6 fits the half-bar rung");
        assert_eq!(
            clipboard.tracks[0].1,
            vec![span(0.0, 2.0, 2, 8.0), span(2.0, 4.0, 3, 0.0)]
        );
    }

    /// 5.1: silence inside the rectangle is a gap — omitted from the spans,
    /// but the track still travels so paste can silence the destination.
    #[test]
    fn copy_omits_gaps_but_keeps_the_silent_track() {
        let mut app = app_with_song();
        app.song_track_paint(1, 4.0, 8.0, None).expect("silence");
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));

        let clipboard = app.song_region_copy().expect("copy succeeds");
        assert_eq!(clipboard.tracks[1].0, 1);
        assert_eq!(
            clipboard.tracks[1].1,
            vec![span(4.0, 8.0, 3, 0.0)],
            "the silenced first half is a gap, not a span"
        );
    }

    /// 5.1: a take span records the SOURCE take id; the clone is minted at
    /// paste time, not here.
    #[test]
    fn copy_records_the_source_take_id() {
        let mut app = app_with_song();
        let take = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("region converts");
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));

        let clipboard = app.song_region_copy().expect("copy succeeds");
        let spans = &clipboard.tracks[0].1;
        assert!(
            spans.iter().all(|span| span.source == LaneSource::Take(take)),
            "{spans:?}"
        );
        assert_eq!(spans.first().map(|span| span.offset_steps), Some(0.0));
    }

    /// 5.2: the pasted rectangle plays exactly the source rectangle, shifted.
    /// Pattern sources paste as REFERENCES (same pool ids), gaps as silence,
    /// and the whole thing is ONE undo entry.
    #[test]
    fn paste_reproduces_the_source_rectangle_shifted() {
        let mut app = app_with_song();
        app.song_set_end(32.0).expect("extend the song");
        app.song_track_paint(1, 4.0, 8.0, None).expect("silence");
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        let source: Vec<_> = (0..2).map(|track| window(&app, track, 4.0, 12.0)).collect();
        let before: Vec<_> = (0..2).map(|track| merged_lane(&app, track)).collect();
        let depth = app.history.undo_len();

        app.song_region_paste(&clipboard, 20.0).expect("paste succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");

        for track in 0..2 {
            assert_eq!(
                window(&app, track, 20.0, 28.0),
                source[track],
                "track {track} pastes identically shifted"
            );
        }
        assert_eq!(
            window(&app, 1, 20.0, 24.0),
            vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
            "the copied gap pastes as silence"
        );

        undo(&mut app);
        for track in 0..2 {
            assert_eq!(merged_lane(&app, track), before[track], "undo restores track {track}");
        }
        redo(&mut app);
        assert_eq!(window(&app, 0, 20.0, 28.0), source[0]);
    }

    /// 5.2: a paste running past the song end extends it INSIDE the same
    /// commit, so one undo takes both back.
    #[test]
    fn paste_extends_the_song_end_in_the_same_entry() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        let depth = app.history.undo_len();

        app.song_region_paste(&clipboard, 12.0).expect("paste succeeds");
        assert_eq!(app.history.undo_len(), depth + 1);
        let song = app.state.committed_song().expect("song");
        assert_eq!(song.end_beat, 20.0, "the song grew to hold the paste");
        assert_eq!(window(&app, 0, 12.0, 20.0), window(&app, 0, 4.0, 12.0));

        undo(&mut app);
        assert_eq!(
            app.state.committed_song().expect("song").end_beat,
            16.0,
            "one undo takes the extension back too"
        );
    }

    /// 5.2: the destination floors to the copied rectangle's own grid, so a
    /// cursor parked mid-bar still pastes on the bar.
    #[test]
    fn paste_floors_the_destination_to_the_clipboard_grid() {
        let mut app = app_with_song();
        app.song_set_end(32.0).expect("extend the song");
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");

        app.song_region_paste(&clipboard, 21.5).expect("paste succeeds");
        assert_eq!(
            window(&app, 0, 20.0, 28.0),
            window(&app, 0, 4.0, 12.0),
            "21.5 floors to the 4-beat grid the copy sat on"
        );
    }

    /// 5.1/5.2 locked: take sources paste as CLONES — a new id over
    /// deep-copied chunks, named after the source — so later per-clip editing
    /// of a pasted take never rewrites the original. One undo entry restores
    /// the song AND the take pool.
    #[test]
    fn paste_clones_take_sources_in_one_entry() {
        let mut app = app_with_song();
        let take = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("region converts");
        app.song_set_end(32.0).expect("extend the song");
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        let depth = app.history.undo_len();

        app.song_region_paste(&clipboard, 20.0).expect("paste succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");

        let pasted = window(&app, 0, 20.0, 28.0);
        let LaneSource::Take(clone) = pasted[0].2 else {
            panic!("the pasted span must be a take, got {pasted:?}");
        };
        assert_ne!(clone, take, "paste mints a NEW take");
        assert!(
            pasted.iter().all(|span| span.2 == LaneSource::Take(clone)),
            "a take clip split across rows clones ONCE: {pasted:?}"
        );
        assert_eq!(
            pasted,
            window(&app, 0, 4.0, 12.0)
                .into_iter()
                .map(|(start, end, _, offset)| (start, end, LaneSource::Take(clone), offset))
                .collect::<Vec<_>>(),
            "same span shape and phase as the source"
        );

        let source_take = app.state.track_take(0, take).expect("source take");
        let clone_take = app.state.track_take(0, clone).expect("cloned take");
        assert_eq!(clone_take.name, format!("{} copy", source_take.name));
        assert_eq!(clone_take.total_len_steps, source_take.total_len_steps);
        assert_ne!(clone_take.chunks, source_take.chunks, "fresh chunk patterns");
        app.state.with_project_scenes(|scenes| {
            for (src, dst) in source_take.chunks.iter().zip(&clone_take.chunks) {
                let src = scenes.track_pools[0].get(*src).expect("source chunk");
                let dst = scenes.track_pools[0].get(*dst).expect("cloned chunk");
                assert_eq!(src.track_bits, dst.track_bits, "chunk steps are deep-copied");
                assert_eq!(src.step_data, dst.step_data, "chunk content is deep-copied");
            }
        });

        undo(&mut app);
        assert!(
            app.state.track_take(0, clone).is_none(),
            "one undo drops the clone with the song edit"
        );
        assert!(app.state.track_take(0, take).is_some(), "the source survives");
        redo(&mut app);
        assert!(app.state.track_take(0, clone).is_some());
    }

    /// 5.1: the clipboard holds a take id, not the take. If it was deleted
    /// since the copy, that span pastes as nothing — the paste still applies.
    #[test]
    fn paste_skips_a_take_deleted_since_the_copy() {
        let mut app = app_with_song();
        let take = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("region converts");
        app.song_set_end(32.0).expect("extend the song");
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        app.song_take_delete(0, take.0).expect("take deletes");

        app.song_region_paste(&clipboard, 20.0)
            .expect("paste still applies");
        assert_eq!(
            window(&app, 0, 20.0, 28.0),
            vec![(0.0, 8.0, LaneSource::Empty, 0.0)],
            "the dead take's span pastes as the silence the rectangle promises"
        );
    }

    /// 5.2: delete writes explicit-empty overrides across the whole
    /// rectangle in one entry — the multi-track Backspace path.
    #[test]
    fn delete_silences_the_rectangle_in_one_entry() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let depth = app.history.undo_len();

        app.song_region_delete().expect("delete succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        for track in 0..2 {
            assert_eq!(
                window(&app, track, 4.0, 12.0),
                vec![(0.0, 8.0, LaneSource::Empty, 0.0)],
                "track {track} is silent across the region"
            );
            assert_eq!(
                window(&app, track, 0.0, 4.0),
                vec![(0.0, 4.0, LaneSource::Pattern(PatternId(1)), 0.0)],
                "track {track} outside the region is untouched"
            );
        }
        // Explicit-empty, not a fallback to the scene cell.
        let song = app.state.committed_song().expect("song");
        let row = song.rows.iter().find(|row| row.start_beat == 4.0).expect("row");
        for track in 0..2 {
            let over = row
                .overrides
                .iter()
                .find(|over| over.track == track)
                .expect("override");
            assert_eq!(over.pattern_id, None);
            assert_eq!(over.take_id, None);
        }

        undo(&mut app);
        assert_eq!(
            window(&app, 0, 4.0, 12.0),
            vec![
                (0.0, 4.0, LaneSource::Pattern(PatternId(2)), 0.0),
                (4.0, 8.0, LaneSource::Pattern(PatternId(3)), 0.0),
            ]
        );
    }

    /// Cmd-D: the region duplicates immediately after itself and everything
    /// downstream slides right by its length — phase-rigidly, so the pushed
    /// material plays exactly what it played before, just later. One undo
    /// entry, and the selection follows the copy so a second Cmd-D chains.
    #[test]
    fn duplicate_ripples_the_song_right_and_chains() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let source: Vec<_> = (0..2).map(|track| window(&app, track, 4.0, 12.0)).collect();
        // What currently sits after the region, and must reappear 8 beats on.
        let pushed: Vec<_> = (0..2).map(|track| window(&app, track, 12.0, 16.0)).collect();
        let before: Vec<_> = (0..2).map(|track| merged_lane(&app, track)).collect();
        let depth = app.history.undo_len();

        app.song_region_duplicate().expect("duplicate succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        let song = app.state.committed_song().expect("song");
        assert_eq!(song.end_beat, 24.0, "the song grew by the region length");

        for track in 0..2 {
            assert_eq!(
                window(&app, track, 12.0, 20.0),
                source[track],
                "track {track} gets the duplicate right after the region"
            );
            assert_eq!(
                window(&app, track, 4.0, 12.0),
                source[track],
                "track {track} keeps the original"
            );
            assert_eq!(
                window(&app, track, 20.0, 24.0),
                pushed[track],
                "track {track}'s downstream material moved right unchanged"
            );
        }
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 1, 12.0, 20.0)),
            "the selection follows the copy"
        );

        // Chaining: a second duplicate acts on the NEW copy.
        app.song_region_duplicate().expect("second duplicate succeeds");
        assert_eq!(
            app.state.committed_song().expect("song").end_beat,
            32.0
        );
        assert_eq!(window(&app, 0, 20.0, 28.0), source[0]);

        undo(&mut app);
        undo(&mut app);
        for track in 0..2 {
            assert_eq!(
                merged_lane(&app, track),
                before[track],
                "two undos restore track {track} exactly"
            );
        }
        assert_eq!(app.state.committed_song().expect("song").end_beat, 16.0);
    }

    /// A PARTIAL region ripples only its own lanes: the selected tracks slide
    /// right, every other track keeps playing at the beats it always did.
    /// The untouched lanes get new row boundaries underneath them (the rows
    /// are shared), which must be phase-transparent — same music, more rows.
    #[test]
    fn duplicate_of_a_partial_region_leaves_the_other_tracks_where_they_were() {
        let mut app = app_with_song_tracks(3);
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let source: Vec<_> = (0..2).map(|track| window(&app, track, 4.0, 12.0)).collect();
        let pushed: Vec<_> = (0..2).map(|track| window(&app, track, 12.0, 16.0)).collect();
        let untouched = merged_lane(&app, 2);
        let before: Vec<_> = (0..3).map(|track| merged_lane(&app, track)).collect();
        let depth = app.history.undo_len();

        app.song_region_duplicate().expect("duplicate succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        assert_eq!(
            app.state.committed_song().expect("song").end_beat,
            24.0,
            "the song still grows to hold the rippled tails"
        );

        for track in 0..2 {
            assert_eq!(window(&app, track, 4.0, 12.0), source[track], "original kept");
            assert_eq!(
                window(&app, track, 12.0, 20.0),
                source[track],
                "track {track} gets the duplicate"
            );
            assert_eq!(
                window(&app, track, 20.0, 24.0),
                pushed[track],
                "track {track}'s tail moved right unchanged"
            );
        }

        // The whole point: track 2 is untouched over the time it already
        // occupied. It only gains the newly created tail, governed by what it
        // was already playing there.
        assert_eq!(
            window(&app, 2, 0.0, 16.0),
            untouched
                .iter()
                .map(|(start, end, source, offset)| (*start, *end, *source, *offset))
                .collect::<Vec<_>>(),
            "the unselected track plays exactly what it did, at the same beats"
        );

        undo(&mut app);
        for track in 0..3 {
            assert_eq!(merged_lane(&app, track), before[track], "undo restores track {track}");
        }
    }

    /// Duplicate is a copy, so take sources clone exactly as paste's do — the
    /// duplicate is its own performance, editable without touching the
    /// original. Still one undo entry across song and take pool.
    #[test]
    fn duplicate_clones_take_sources_in_one_entry() {
        let mut app = app_with_song();
        let take = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("region converts");
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let depth = app.history.undo_len();

        app.song_region_duplicate().expect("duplicate succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");

        let duplicated = window(&app, 0, 12.0, 20.0);
        let LaneSource::Take(clone) = duplicated[0].2 else {
            panic!("the duplicate must be a take, got {duplicated:?}");
        };
        assert_ne!(clone, take, "the duplicate is its own take");
        assert_eq!(
            app.state.track_take(0, clone).expect("clone").name,
            format!("{} copy", app.state.track_take(0, take).expect("source").name)
        );
        assert_eq!(
            duplicated,
            window(&app, 0, 4.0, 12.0)
                .into_iter()
                .map(|(start, end, _, offset)| (start, end, LaneSource::Take(clone), offset))
                .collect::<Vec<_>>(),
            "same span shape and phase as the original"
        );

        undo(&mut app);
        assert!(app.state.track_take(0, clone).is_none());
        assert!(app.state.track_take(0, take).is_some());
    }

    /// The extracted helper paints TAKE sources too (region spec 5.2 — the
    /// latent gap `song-track-paint` had), re-anchoring each row it covers
    /// linearly from the anchor, and commits nothing on its own.
    #[test]
    fn paint_source_region_paints_takes_without_committing() {
        let mut app = app_with_song();
        let take = app
            .song_region_to_take(0, 0.0, 16.0)
            .expect("region converts");
        let mut song = app.state.committed_song().expect("song");
        let depth = app.history.undo_len();

        app.paint_source_region(
            &mut song,
            0,
            4.0,
            12.0,
            LaneSource::Take(take),
            4.0,
            8.0,
        )
        .expect("take paint succeeds");

        // 16th steps: the row at 8.0 is 4 beats (16 steps) past the anchor,
        // so it CONTINUES the take at 8 + 16 rather than restarting it.
        let offsets: Vec<(f64, Option<u64>, f64)> = song
            .rows
            .iter()
            .filter(|row| row.start_beat >= 4.0 && row.start_beat < 12.0)
            .map(|row| {
                let over = row
                    .overrides
                    .iter()
                    .find(|over| over.track == 0)
                    .expect("painted override");
                (row.start_beat, over.take_id, over.offset_steps)
            })
            .collect();
        assert_eq!(
            offsets,
            vec![(4.0, Some(take.0), 8.0), (8.0, Some(take.0), 24.0)]
        );
        assert_eq!(app.history.undo_len(), depth, "the helper never commits");
        assert_ne!(
            app.state.committed_song().expect("song").rows,
            song.rows,
            "and it never installs the song it edited"
        );
    }

    #[test]
    fn new_normalizes_reversed_ends() {
        let region = SongRegionSelection::new(5, 2, 16.0, 4.0);
        assert_eq!(region.track_a, 2);
        assert_eq!(region.track_b, 5);
        assert_eq!(region.start_beat, 4.0);
        assert_eq!(region.end_beat, 16.0);
        assert!(region.contains_track(3));
        assert!(!region.contains_track(6));
        assert!(!region.is_empty());
    }

    #[test]
    fn zero_width_region_is_empty() {
        assert!(SongRegionSelection::new(0, 0, 8.0, 8.0).is_empty());
    }

    #[test]
    fn negative_beats_clamp_to_zero() {
        let region = SongRegionSelection::new(0, 1, -4.0, 8.0);
        assert_eq!(region.start_beat, 0.0);
        assert_eq!(region.end_beat, 8.0);
    }

    /// 4.1 mutual exclusivity, region -> clip: a region names no single clip,
    /// so committing one drops the clip selection and hands the sound binding
    /// back to the playback/scene rules.
    #[test]
    fn setting_a_region_releases_the_bound_clip() {
        let (mut app, take, scene_pattern, _chunks) = app_with_take();
        app.select_song_clip(0, SongRowId(0))
            .expect("clip selects");
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take))
        );

        assert!(app.set_song_region(SongRegionSelection::new(0, 0, 0.0, 8.0)));
        assert_eq!(app.song_clip_selection, None, "the clip selection is dropped");
        assert!(
            app.track_sound_binding(0).is_scene(),
            "the binding falls back off rule 1"
        );
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Pattern(scene_pattern))
        );
    }

    /// Clip -> region: selecting a clip REPLACES the region with that clip's
    /// own span and keeps the binding — a selected clip is a one-clip region
    /// (Ableton), not the opposite of one. Both light the same body colour.
    #[test]
    fn selecting_a_clip_narrows_the_region_to_its_span() {
        let (mut app, take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(0, 0, 0.0, 64.0));
        assert!(app.song_region_selection.is_some());

        app.select_song_clip_span(0, SongRowId(0), Some((4.0, 12.0)))
            .expect("clip selects");
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 0, 4.0, 12.0)),
            "the region becomes the clip's footprint"
        );
        assert_eq!(
            app.track_sound_binding(0).source,
            Some(BoundSource::Take(take)),
            "and the clip still owns the sound binding"
        );
    }

    /// Selecting with no span (nothing under the pointer) clears the region
    /// rather than leaving it highlighting a clip that is not selected.
    #[test]
    fn selecting_a_clip_without_a_span_clears_the_region() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(0, 0, 0.0, 64.0));
        app.select_song_clip_span(0, SongRowId(0), None)
            .expect("clip selects");
        assert_eq!(app.song_region_selection, None);
    }

    /// Deselecting a clip must NOT clear the region: `set_song_region`
    /// deselects on its way in, and that internal call would otherwise wipe
    /// the region it is about to store.
    #[test]
    fn deselecting_a_clip_leaves_the_region_alone() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(1, 3, 4.0, 12.0));
        app.set_song_clip_selection(None);
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(1, 3, 4.0, 12.0))
        );

        assert!(app.clear_song_region());
        assert_eq!(app.song_region_selection, None);
        assert!(!app.clear_song_region(), "clearing twice reports no change");
    }

    /// A zero-width rectangle is not a selection: it clears instead.
    #[test]
    fn empty_region_clears_rather_than_committing() {
        let (mut app, _take, _scene_pattern, _chunks) = app_with_take();
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        app.set_song_region(SongRegionSelection::new(0, 0, 8.0, 8.0));
        assert_eq!(app.song_region_selection, None);
    }
}
