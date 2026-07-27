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

use crate::sequencer::{
    insert_clip_sorted, occlude_span, restamped_clip, ArrClip, ClipId, LaneSource,
    ProjectArrangement, ProjectScenes, TakeId, TrackPatternData,
};

use super::edit::finish_active_gesture;
use super::history::{ArrangementStructurePatch, EditPatch, SceneStructurePatch};
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
    /// True when the marquee was swept in the SCENE lane (lane spec 8: "copy
    /// … plus scene events inside a scene-lane region"). The rectangle is
    /// identical either way — a scene-lane sweep already spans every visible
    /// track — but this bit is what tells copy/paste/delete to carry the
    /// scene EVENTS as well as the clips. A track-lane marquee, even one that
    /// happens to cover every track, never touches the scene lane.
    pub scene_lane: bool,
}

impl SongRegionSelection {
    /// Normalizing constructor: callers pass the two ends of a drag in
    /// whatever order the pointer produced them. Track-lane marquee.
    pub fn new(track_a: usize, track_b: usize, start_beat: f64, end_beat: f64) -> Self {
        Self::new_in_lane(track_a, track_b, start_beat, end_beat, false)
    }

    /// `new` with the scene-lane bit set explicitly.
    pub fn new_in_lane(
        track_a: usize,
        track_b: usize,
        start_beat: f64,
        end_beat: f64,
        scene_lane: bool,
    ) -> Self {
        Self {
            track_a: track_a.min(track_b),
            track_b: track_a.max(track_b),
            start_beat: start_beat.min(end_beat).max(0.0),
            end_beat: start_beat.max(end_beat).max(0.0),
            scene_lane,
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
/// (region spec 5.1). One entry per CLIP the region intersects: a lane with
/// no clip over part of the region contributes nothing there, because a gap
/// is silence (arrangement-lane-model-spec 6.2) and pasting the rectangle
/// somewhere else must reproduce that silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipboardSpan {
    pub rel_start: f64,
    pub rel_end: f64,
    /// `Pattern` pastes as a reference; `Take` pastes as a fresh clone
    /// (region spec 5.1 locked decision). `Empty` cannot arise from a stored
    /// clip any more (clips always have a source); a paste that meets one
    /// stores nothing, leaving the cleared span silent.
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
    /// The scene lane's contribution, present only for a SCENE-LANE region
    /// (lane spec 8). `scene_events` are `(rel_beat, scene)` for every scene
    /// event inside the copied span, with a leading entry at `rel_beat 0.0`
    /// for the scene governing the region's start — so pasting reproduces
    /// what the scene lane sounded like, not just its change points.
    pub scene_lane: bool,
    pub scene_events: Vec<(f64, usize)>,
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

    /// One lane's CLIPS clipped to `[start_beat, end_beat)`, in ABSOLUTE
    /// beats, as `(start, end, source, offset_at_start)`.
    ///
    /// A clip the window cuts into gets its offset re-stamped to the cut by
    /// the compiler's own split rule (`restamped_clip`), so the fragment plays
    /// the identical slice wherever it is re-anchored — the property copy
    /// (region spec 5.1) and the duplicate ripple both depend on. Lane gaps
    /// produce nothing: they are silence, and silence has no content.
    fn arrangement_clip_spans_in(
        &self,
        arrangement: &ProjectArrangement,
        scenes: &ProjectScenes,
        track: usize,
        start_beat: f64,
        end_beat: f64,
    ) -> Vec<(f64, f64, LaneSource, f64)> {
        let Some(lane) = arrangement.track_lanes.get(track) else {
            return Vec::new();
        };
        lane.iter()
            .filter_map(|clip| {
                let span_start = clip.start_beat.max(start_beat);
                let span_end = clip.end_beat.min(end_beat);
                if span_end - span_start <= 1e-9 {
                    return None;
                }
                // A slice with nothing left to play carries no content.
                let cut = restamped_clip(scenes, track, clip, span_start)?;
                Some((span_start, span_end, cut.source(), cut.offset_steps))
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
        let arrangement = self
            .state
            .committed_arrangement()
            .ok_or_else(|| "The project has no song".to_string())?;
        let start_beat = region.start_beat;
        let end_beat = region.end_beat.min(arrangement.end_beat);
        if end_beat <= start_beat {
            return Err("The selected region has no length".to_string());
        }
        let scenes = self.state.capture_project_scenes();

        let mut tracks = Vec::new();
        for track in region.track_a..=region.track_b {
            if track >= arrangement.track_lanes.len() {
                continue;
            }
            // Clip-free tracks stay in the rectangle (with no spans) so paste
            // clears them: the clipboard is the whole rectangle.
            let spans = self
                .arrangement_clip_spans_in(&arrangement, &scenes, track, start_beat, end_beat)
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
        // Scene-lane regions carry the scene lane too (lane spec 8). The
        // leading entry restates the scene marked at the region's start, so
        // the pasted rectangle carries the scene markers it was copied with.
        let scene_events = if region.scene_lane {
            let mut events: Vec<(f64, usize)> = arrangement
                .scene_at_beat(start_beat)
                .map(|scene| vec![(0.0, scene)])
                .unwrap_or_default();
            events.extend(
                arrangement
                    .scene_lane
                    .iter()
                    .filter(|event| event.start_beat > start_beat && event.start_beat < end_beat)
                    .map(|event| (event.start_beat - start_beat, event.scene)),
            );
            events
        } else {
            Vec::new()
        };
        Ok(ArrangementClipboard {
            len_beats,
            snap_beats: region_snap_beats(start_beat, len_beats),
            tracks,
            scene_lane: region.scene_lane,
            scene_events,
        })
    }

    /// Paste the clipboard rectangle at `dest_beat` (region spec 5.2): one
    /// clone of the committed arrangement, one undo entry.
    ///
    /// Per clipboard track: clear the whole destination span first (paste is
    /// the op that truncates, spec 8), then insert each stored span as a clip
    /// with its offset, anchored at its pasted start. Pattern sources paste as
    /// REFERENCES;
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
        let arrangement_before = self.state.committed_arrangement();
        let Some(existing) = arrangement_before.clone() else {
            return Err("The project has no song".to_string());
        };

        let (scenes_before, cloned_takes) = self.clone_clipboard_takes(clipboard)?;
        let scenes = self.state.capture_project_scenes();
        let build = (|| -> Result<ProjectArrangement, String> {
            let mut arrangement = existing.clone();
            // The pasted rectangle may run past the song end; extending it
            // rides inside this same commit (region spec 5.2).
            arrangement.end_beat = arrangement.end_beat.max(dest_beat + clipboard.len_beats);
            self.paste_clipboard(
                &mut arrangement,
                &scenes,
                clipboard,
                dest_beat,
                &cloned_takes,
            )?;
            Ok(arrangement)
        })();
        self.commit_region_edit("Paste region", arrangement_before, build, scenes_before)?;

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
    /// beats it always did. Two scopes, same audible contract:
    ///
    /// - **Region covers every track** — every lane's clips AND the scene
    ///   lane slide right, so the whole timeline opens up. This is the
    ///   "insert 4 bars into my song" gesture.
    /// - **Region covers some tracks** — only those lanes' clips slide; the
    ///   scene lane and every other lane stay exactly where they were.
    ///
    /// A clip straddling the insert point is SPLIT there first, so the part
    /// that moves carries the phase it had at the boundary (takes spec 7.4)
    /// and the part that stays is untouched. Either way the song grows by
    /// `len`. Take sources duplicate as fresh clones exactly as paste's do.
    pub fn song_region_duplicate(&mut self) -> Result<String, String> {
        self.require_song_edit_unlocked()?;
        let region = self
            .song_region_selection
            .ok_or_else(|| "No arrangement region is selected".to_string())?;
        let clipboard = self.song_region_copy()?;
        let arrangement_before = self.state.committed_arrangement();
        let Some(existing) = arrangement_before.clone() else {
            return Err("The project has no song".to_string());
        };
        let len_beats = clipboard.len_beats;
        // The duplicate lands where the region ends; the copy already clamped
        // that to the song end, so it is always inside the song.
        let insert_beat = region.end_beat.min(existing.end_beat);

        // Whole-timeline shift only when the region really covers everything;
        // otherwise the untouched lanes must not move.
        // A scene-lane sweep is by definition a whole-timeline gesture: it
        // carries the scene lane, so the ripple has to move it too.
        let covers_every_track = region.scene_lane
            || (region.track_a == 0 && region.track_b + 1 >= self.tracks.len().max(1));

        let (scenes_before, cloned_takes) = self.clone_clipboard_takes(&clipboard)?;
        let scenes = self.state.capture_project_scenes();
        let build = (|| -> Result<ProjectArrangement, String> {
            let mut arrangement = existing.clone();
            arrangement.end_beat += len_beats;
            let rippled: Vec<usize> = if covers_every_track {
                (0..arrangement.track_lanes.len()).collect()
            } else {
                clipboard.tracks.iter().map(|(track, _)| *track).collect()
            };
            for track in rippled {
                if track >= arrangement.track_lanes.len() {
                    continue;
                }
                Self::ripple_lane_right(&mut arrangement, &scenes, track, insert_beat, len_beats)?;
            }
            if covers_every_track {
                for event in &mut arrangement.scene_lane {
                    if event.start_beat >= insert_beat && event.start_beat > 0.0 {
                        event.start_beat += len_beats;
                    }
                }
            }
            // Both paths leave exactly [insert, insert + len) vacated on the
            // rippled lanes; the duplicate fills it.
            self.paste_clipboard(
                &mut arrangement,
                &scenes,
                &clipboard,
                insert_beat,
                &cloned_takes,
            )?;
            Ok(arrangement)
        })();
        self.commit_region_edit("Duplicate region", arrangement_before, build, scenes_before)?;

        // The highlight follows the copy, so Cmd-D again duplicates THAT.
        // It names no single clip any more, so it goes through the marquee
        // door (spec 4.1) and hands the sound binding back.
        self.set_song_region(SongRegionSelection::new_in_lane(
            region.track_a,
            region.track_b,
            insert_beat,
            insert_beat + len_beats,
            region.scene_lane,
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

    /// Move the selected region rigidly by `delta_beats` (region spec 6.2):
    /// the rectangle's clips are lifted, the source rectangle goes silent, and
    /// they land `delta_beats` later or earlier with their `offset_steps`
    /// untouched — so the moved music sounds identical, just somewhere else
    /// (takes spec 7.4).
    ///
    /// The rectangle is lifted exactly the way copy lifts it: a clip the region
    /// only partially covers moves only its covered part, cut at the edge and
    /// re-anchored through `restamped_clip` (spec 8). Take sources are NOT
    /// cloned — a move keeps referencing the very takes it always did — so this
    /// is always a single `EditPatch::Arrangement` entry.
    ///
    /// Order matters: the source rectangle is cleared BEFORE the destination is
    /// written, or an overlapping move (a delta smaller than the region) would
    /// erase the clips it had just placed.
    pub fn song_region_move(&mut self, delta_beats: f64) -> Result<String, String> {
        self.require_song_edit_unlocked()?;
        if !delta_beats.is_finite() {
            return Err(format!(
                "Region move delta must be a finite beat count (got {delta_beats})"
            ));
        }
        let region = self
            .song_region_selection
            .ok_or_else(|| "No arrangement region is selected".to_string())?;
        if delta_beats == 0.0 {
            return Ok("The region did not move".to_string());
        }
        let dest_beat = region.start_beat + delta_beats;
        if dest_beat < 0.0 {
            // Never silently truncate the leading clips: the UI clamps the
            // drag, so reaching here means a stale or synthetic gesture.
            return Err(format!(
                "Moving the region by {delta_beats} beats would push it before beat 0"
            ));
        }
        let clipboard = self.song_region_copy()?;
        let arrangement_before = self.state.committed_arrangement();
        let Some(existing) = arrangement_before.clone() else {
            return Err("The project has no song".to_string());
        };
        let len_beats = clipboard.len_beats;
        let source_start = region.start_beat;
        let source_end = source_start + len_beats;
        let dest_end = dest_beat + len_beats;

        // `paste_clipboard` resolves take sources through a clone map; a move
        // clones nothing, so the map is the identity over the takes present.
        let mut takes: HashMap<(usize, u64), TakeId> = HashMap::new();
        for (track, spans) in &clipboard.tracks {
            for span in spans {
                if let LaneSource::Take(take) = span.source {
                    takes.insert((*track, take.0), take);
                }
            }
        }

        let scenes = self.state.capture_project_scenes();
        let build = (|| -> Result<ProjectArrangement, String> {
            let mut arrangement = existing.clone();
            // Like paste, a move that runs past the song end extends it inside
            // the same commit (spec 5.2).
            arrangement.end_beat = arrangement.end_beat.max(dest_end);
            for (track, _) in &clipboard.tracks {
                if *track >= arrangement.track_lanes.len() {
                    continue;
                }
                occlude_span(&mut arrangement, &scenes, *track, source_start, source_end)?;
            }
            // A scene-lane region moves its scene events too, restoring what
            // governed the vacated span's end so nothing after it changes.
            if clipboard.scene_lane {
                let tail_scene =
                    Self::clear_scene_lane_span(&mut arrangement, source_start, source_end);
                Self::restore_scene_tail(&mut arrangement, source_end, tail_scene);
            }
            // Destination second — it clears its own rectangle and stamps the
            // lifted clips (and, for a scene-lane region, the scene events).
            self.paste_clipboard(&mut arrangement, &scenes, &clipboard, dest_beat, &takes)?;
            Ok(arrangement)
        })();
        self.commit_region_edit("Move region", arrangement_before, build, None)?;

        // The highlight follows the move, so a repeated drag (or a Cmd-C right
        // after one) addresses where the music now is.
        self.set_song_region(SongRegionSelection::new_in_lane(
            region.track_a,
            region.track_b,
            dest_beat,
            dest_end,
            region.scene_lane,
        ));
        Ok(format!(
            "Moved beats {source_start}-{source_end} to {dest_beat}"
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

    /// Slide everything on `track`'s lane at or after `insert_beat` right by
    /// `len_beats`, splitting a clip that straddles the boundary so the moving
    /// half carries the phase it had there (takes spec 7.4).
    fn ripple_lane_right(
        arrangement: &mut ProjectArrangement,
        scenes: &ProjectScenes,
        track: usize,
        insert_beat: f64,
        len_beats: f64,
    ) -> Result<(), String> {
        // Split the straddling clip first, so the shift is a clean partition.
        let straddling = arrangement.track_lanes[track]
            .iter()
            .position(|clip| clip.start_beat < insert_beat && clip.end_beat > insert_beat);
        if let Some(index) = straddling {
            let clip = arrangement.track_lanes[track][index];
            // A tail with nothing left to play (a take past its end) is
            // dropped: silence is a gap, never an empty clip.
            let tail = restamped_clip(scenes, track, &clip, insert_beat);
            arrangement.track_lanes[track][index].end_beat = insert_beat;
            if let Some(mut tail) = tail {
                tail.id = arrangement.allocate_clip_id()?;
                arrangement.track_lanes[track].insert(index + 1, tail);
            }
        }
        for clip in &mut arrangement.track_lanes[track] {
            if clip.start_beat >= insert_beat {
                clip.start_beat += len_beats;
                clip.end_beat += len_beats;
            }
        }
        Ok(())
    }

    /// Stamp the clipboard rectangle onto `arrangement` at `dest_beat`: clear
    /// the whole destination span per track (a gap in the clipboard is a gap
    /// in the destination), then insert each stored span as a clip anchored at
    /// its own pasted start. Take spans paste their clone, or nothing when the
    /// source was deleted.
    fn paste_clipboard(
        &self,
        arrangement: &mut ProjectArrangement,
        scenes: &ProjectScenes,
        clipboard: &ArrangementClipboard,
        dest_beat: f64,
        cloned_takes: &HashMap<(usize, u64), TakeId>,
    ) -> Result<(), String> {
        let dest_end = dest_beat + clipboard.len_beats;
        for (track, spans) in &clipboard.tracks {
            // The clipboard stores absolute track indices; one that no longer
            // resolves is skipped (region spec 5.1 locked decision).
            if *track >= arrangement.track_lanes.len() {
                continue;
            }
            occlude_span(arrangement, scenes, *track, dest_beat, dest_end)?;
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
                let id = arrangement.allocate_clip_id()?;
                let mut clip = ArrClip::new(id, dest_beat + span.rel_start, dest_beat + span.rel_end, None);
                match source {
                    // A sourceless span is silence, and the destination was
                    // already cleared: store nothing (spec 6.1).
                    LaneSource::Empty => continue,
                    LaneSource::Pattern(pattern) => clip.pattern_id = Some(pattern.0),
                    LaneSource::Take(take) => clip.take_id = Some(take.0),
                }
                clip.offset_steps = span.offset_steps;
                insert_clip_sorted(arrangement, *track, clip);
            }
        }
        if clipboard.scene_lane {
            Self::paste_scene_events(arrangement, &clipboard.scene_events, dest_beat, dest_end);
        }
        Ok(())
    }

    /// Set the scene of the event at `beat`, inserting one if there is none.
    /// Keeps the lane sorted; `beat` is assumed inside the arrangement.
    fn set_scene_event(arrangement: &mut ProjectArrangement, beat: f64, scene: usize) {
        match arrangement
            .scene_lane
            .iter()
            .position(|event| event.start_beat == beat)
        {
            Some(index) => arrangement.scene_lane[index].scene = scene,
            None => {
                let position = arrangement
                    .scene_lane
                    .iter()
                    .position(|event| event.start_beat > beat)
                    .unwrap_or(arrangement.scene_lane.len());
                arrangement
                    .scene_lane
                    .insert(position, crate::sequencer::SceneEvent { start_beat: beat, scene });
            }
        }
    }

    /// Clear the scene lane over `[start, end)` — never removing the
    /// mandatory event at 0.0 — while preserving what governed `end`, so
    /// everything after the region keeps playing the scene it always did.
    /// Returns the scene that has to be restored at `end`, if any.
    fn clear_scene_lane_span(
        arrangement: &mut ProjectArrangement,
        start: f64,
        end: f64,
    ) -> Option<usize> {
        let tail_scene = arrangement.scene_at_beat(end);
        arrangement
            .scene_lane
            .retain(|event| event.start_beat == 0.0 || event.start_beat < start || event.start_beat >= end);
        tail_scene
    }

    /// Re-establish `tail_scene` at `end` when the span edit changed what
    /// governs it (and `end` is still inside the arrangement).
    fn restore_scene_tail(arrangement: &mut ProjectArrangement, end: f64, tail_scene: Option<usize>) {
        let Some(scene) = tail_scene else { return };
        if end >= arrangement.end_beat {
            return;
        }
        if arrangement.scene_at_beat(end) != Some(scene) {
            Self::set_scene_event(arrangement, end, scene);
        }
    }

    /// Stamp the copied scene events into `[dest, dest_end)` (lane spec 8):
    /// clear that span of the scene lane, drop the copied events in at their
    /// relative beats, then restore whatever governed `dest_end` so the paste
    /// is local to the rectangle.
    fn paste_scene_events(
        arrangement: &mut ProjectArrangement,
        events: &[(f64, usize)],
        dest_beat: f64,
        dest_end: f64,
    ) {
        let tail_scene = Self::clear_scene_lane_span(arrangement, dest_beat, dest_end);
        for (rel_beat, scene) in events {
            let beat = dest_beat + rel_beat;
            if beat >= arrangement.end_beat {
                continue;
            }
            Self::set_scene_event(arrangement, beat, *scene);
        }
        Self::restore_scene_tail(arrangement, dest_end, tail_scene);
    }

    /// Shared tail for the clipboard primitives: install the built
    /// arrangement (which validates and recompiles it) and commit ONE history
    /// entry — `EditPatch::Arrangement` when no takes were cloned, else a
    /// composite pairing the scene patch with it (scenes first, ordering per
    /// `song_region_to_take`). Any failure rolls the take clones back and
    /// leaves the committed arrangement untouched.
    fn commit_region_edit(
        &mut self,
        label: &'static str,
        arrangement_before: Option<ProjectArrangement>,
        build: Result<ProjectArrangement, String>,
        scenes_before: Option<ProjectScenes>,
    ) -> Result<(), String> {
        let arrangement_after = match build {
            Ok(arrangement) => arrangement,
            Err(error) => {
                if let Some(scenes) = &scenes_before {
                    self.restore_scene_structure_state(scenes)?;
                }
                return Err(error);
            }
        };
        if let Err(error) = self
            .state
            .set_committed_arrangement(Some(arrangement_after.clone()))
        {
            if let Some(scenes) = &scenes_before {
                self.restore_scene_structure_state(scenes)?;
            }
            return Err(format!("{label} produced an invalid arrangement: {error}"));
        }
        let arrangement_patch = ArrangementStructurePatch {
            before: arrangement_before,
            after: Some(arrangement_after),
        };
        match scenes_before {
            None => {
                let retained_bytes = arrangement_patch.retained_bytes();
                finish_active_gesture(self);
                self.history.commit(
                    label,
                    None,
                    EditPatch::Arrangement(arrangement_patch),
                    retained_bytes,
                );
            }
            Some(scenes_before) => {
                let scenes_after = self.state.capture_project_scenes();
                finish_active_gesture(self);
                let scene_patch = SceneStructurePatch {
                    before: scenes_before,
                    after: scenes_after,
                };
                let retained_bytes =
                    scene_patch.retained_bytes() + arrangement_patch.retained_bytes();
                self.history.commit(
                    label,
                    None,
                    EditPatch::Composite(vec![
                        EditPatch::SceneStructure(scene_patch),
                        EditPatch::Arrangement(arrangement_patch),
                    ]),
                    retained_bytes,
                );
            }
        }
        self.rebuild_active_song_after_arrangement_edit();
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

    /// Remove the clips the selected region covers, trimming the ones it only
    /// partially covers (spec 8), in one undo entry. This is what multi-track
    /// Backspace lowers to.
    ///
    /// The cleared span goes SILENT (spec 6.2): the clips are gone from the
    /// timeline and nothing plays there — which is the point of deleting.
    pub fn song_region_delete(&mut self) -> Result<String, String> {
        self.require_song_edit_unlocked()?;
        let region = self
            .song_region_selection
            .ok_or_else(|| "No arrangement region is selected".to_string())?;
        let before = self.state.committed_arrangement();
        let Some(existing) = before.clone() else {
            return Err("The project has no song".to_string());
        };
        let start_beat = region.start_beat;
        let end_beat = region.end_beat.min(existing.end_beat);
        if end_beat <= start_beat {
            return Err("The selected region has no length".to_string());
        }

        let mut arrangement = existing.clone();
        let scenes = self.state.capture_project_scenes();
        let mut cleared = 0usize;
        for track in region.track_a..=region.track_b {
            if track >= arrangement.track_lanes.len() {
                continue;
            }
            occlude_span(&mut arrangement, &scenes, track, start_beat, end_beat)?;
            cleared += 1;
        }
        if cleared == 0 {
            return Err("The selected region covers no existing track".to_string());
        }
        // A scene-lane region also removes the scene CHANGES inside it (lane
        // spec 8): each merges into its predecessor, and the scene governing
        // the region's end is restored so nothing after it moves.
        if region.scene_lane {
            let tail_scene = Self::clear_scene_lane_span(&mut arrangement, start_beat, end_beat);
            Self::restore_scene_tail(&mut arrangement, end_beat, tail_scene);
        }
        if arrangement == existing {
            return Ok("The region is already empty".to_string());
        }
        self.commit_arrangement_edit("Delete region", before, Some(arrangement))?;
        Ok(format!(
            "Deleted beats {start_beat}-{end_beat} on {cleared} track{}",
            if cleared == 1 { "" } else { "s" }
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

    /// Re-anchor the one-clip region onto a clip's CURRENT span (spec 6.1).
    ///
    /// A clip drag moves the object the selection names, so the highlight —
    /// and with it copy/delete's target — has to follow it; otherwise a Cmd-C
    /// straight after a move would lift the beats the clip just left. A no-op
    /// unless `clip_id` is the selected clip and still exists.
    pub fn refresh_song_region_for_clip(&mut self, clip_id: ClipId) -> bool {
        let Some(selection) = self.song_clip_selection else {
            return false;
        };
        if selection.clip_id != clip_id {
            return false;
        }
        let Some(arrangement) = self.state.committed_arrangement() else {
            return false;
        };
        let Some((track, clip)) = arrangement.find_clip(clip_id) else {
            return false;
        };
        self.set_song_region_for_clip(SongRegionSelection::new(
            track,
            track,
            clip.start_beat,
            clip.end_beat,
        ))
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
    use crate::app::sound_binding::tests::app_with_take;
    use crate::app::sound_binding::BoundSource;
    use crate::app::AudioBuses;
    use crate::audiograph::LiveGraphPtr;
    use crate::recorder::MasterRecorder;
    use crate::sequencer::{
        default_empty_effect_chain, project_lanes, ClipId, PatternId, PatternSnapshot,
        SequencerState, StepParam,
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

    /// Scene 0 is marked over the whole timeline and every track carries
    /// three clips: `[0,4)` P1, `[4,8)` P2, `[8,16)` P3, end 16. The lanes
    /// are fully covered, so every beat sounds; a deleted clip leaves an
    /// audibly SILENT gap (spec 6.2).
    fn app_with_song_tracks(tracks: usize) -> App {
        let mut app = multi_track_app(tracks);
        let mut arrangement = crate::sequencer::ProjectArrangement::new(tracks, 16.0);
        for track in 0..tracks {
            for (start, end, pattern) in [(0.0, 4.0, 1u64), (4.0, 8.0, 2), (8.0, 16.0, 3)] {
                let id = arrangement.allocate_clip_id().expect("clip id");
                arrangement.track_lanes[track].push(ArrClip::new(id, start, end, Some(pattern)));
            }
        }
        app.arr_replace(arrangement).expect("arrangement installs");
        app
    }

    /// `app_with_song` plus a scene CHANGE at beat 8 (scene 0 -> scene 1),
    /// so the scene-lane region ops have something to carry.
    fn app_with_scene_change() -> App {
        let mut app = app_with_song();
        app.arr_scene_event_insert(8.0, 1)
            .expect("scene change inserts");
        app
    }

    fn scene_lane(app: &App) -> Vec<(f64, usize)> {
        app.state
            .committed_arrangement()
            .expect("arrangement")
            .scene_lane
            .iter()
            .map(|event| (event.start_beat, event.scene))
            .collect()
    }

    /// The stored clips of one lane as `(start, end, source, offset)`.
    fn clips(app: &App, track: usize) -> Vec<(f64, f64, LaneSource, f64)> {
        app.state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[track]
            .iter()
            .map(|clip| (clip.start_beat, clip.end_beat, clip.source(), clip.offset_steps))
            .collect()
    }

    /// The clip a gesture on `track` at `beat` addresses.
    fn clip_at(app: &App, track: usize, beat: f64) -> ClipId {
        app.state
            .committed_arrangement()
            .expect("arrangement")
            .track_lanes[track]
            .iter()
            .find(|clip| clip.contains(beat))
            .expect("a clip covers that beat")
            .id
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

    /// 5.1 on lanes: a LANE GAP carries no span — it is silence, and silence
    /// has no content — but the track stays in the rectangle so a paste
    /// clears the destination there. Pasting therefore reproduces the gap.
    #[test]
    fn copy_omits_lane_gaps_and_pastes_them_back_as_silence() {
        let mut app = app_with_song();
        app.arr_set_end(32.0).expect("extend the song");
        // Track 1: delete the [4,8) clip, leaving a genuine silent gap.
        let clip = clip_at(&app, 1, 4.0);
        app.arr_clip_delete(clip).expect("clip deletes");
        assert_eq!(
            window(&app, 1, 4.0, 8.0),
            vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
            "deleting a clip leaves silence, not the scene's pattern"
        );

        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        assert_eq!(clipboard.tracks[1].0, 1);
        assert_eq!(
            clipboard.tracks[1].1,
            vec![span(4.0, 8.0, 3, 0.0)],
            "the gap is not a span: silence is not content"
        );

        app.song_region_paste(&clipboard, 20.0).expect("paste succeeds");
        assert_eq!(
            window(&app, 1, 20.0, 24.0),
            vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
            "and the gap pastes back as silence"
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
        app.arr_set_end(32.0).expect("extend the song");
        // A silent stretch inside the rectangle: a deleted clip, which is the
        // only way the model expresses silence (spec 6.1).
        let clip = clip_at(&app, 1, 4.0);
        app.arr_clip_delete(clip).expect("clip deletes");
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
        app.arr_set_end(32.0).expect("extend the song");
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
        app.arr_set_end(32.0).expect("extend the song");
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
    /// since the copy, that span pastes as nothing — the paste still applies
    /// and the destination lane is simply left clip-free, i.e. silent (lane
    /// spec 6.2).
    #[test]
    fn paste_skips_a_take_deleted_since_the_copy() {
        let mut app = app_with_song();
        let take = app
            .song_region_to_take(0, 4.0, 12.0)
            .expect("region converts");
        app.arr_set_end(32.0).expect("extend the song");
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        app.song_take_delete(0, take.0).expect("take deletes");

        app.song_region_paste(&clipboard, 20.0)
            .expect("paste still applies");
        assert!(
            app.state.committed_arrangement().expect("arrangement").track_lanes[0]
                .iter()
                .all(|clip| clip.start_beat >= 28.0 || clip.end_beat <= 20.0),
            "the dead take's span pastes as no clip at all"
        );
        assert_eq!(
            window(&app, 0, 20.0, 28.0),
            vec![(0.0, 8.0, LaneSource::Empty, 0.0)],
            "so the lane is silent there"
        );
    }

    /// 5.2 on lanes: delete REMOVES the clips the rectangle covers (trimming
    /// the ones it only partly covers) in one entry — the multi-track
    /// Backspace path.
    ///
    /// The clip is an object, so deleting it is a removal — and the span it
    /// covered goes SILENT (spec 6.2). This is the headline behavior: select
    /// clips, delete, and the timeline is genuinely empty there.
    #[test]
    fn delete_removes_the_clips_in_the_rectangle_in_one_entry() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let depth = app.history.undo_len();

        app.song_region_delete().expect("delete succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        for track in 0..2 {
            assert_eq!(
                clips(&app, track),
                vec![
                    (0.0, 4.0, LaneSource::Pattern(PatternId(1)), 0.0),
                    // The [8,16) clip is left-trimmed to the region end, its
                    // offset re-stamped by the split rule: 4 beats == 16
                    // sixteenth steps, which wraps to 0 in a 16-step pattern.
                    (12.0, 16.0, LaneSource::Pattern(PatternId(3)), 0.0),
                ],
                "track {track} keeps only what the region did not cover"
            );
            assert_eq!(
                window(&app, track, 4.0, 12.0),
                vec![(0.0, 8.0, LaneSource::Empty, 0.0)],
                "track {track} is SILENT across the cleared region"
            );
            assert_eq!(
                window(&app, track, 0.0, 4.0),
                vec![(0.0, 4.0, LaneSource::Pattern(PatternId(1)), 0.0)],
                "track {track} outside the region is untouched"
            );
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

    /// The other half of the delete contract: deleting an ALREADY-silent
    /// region is a no-op — there is nothing underneath to reveal, because a
    /// gap is not backed by anything (spec 6.2).
    #[test]
    fn delete_over_an_already_silent_region_changes_nothing() {
        let mut app = app_with_song();
        let clip = clip_at(&app, 0, 4.0);
        app.arr_clip_delete(clip).expect("clip deletes");
        assert_eq!(
            window(&app, 0, 4.0, 8.0),
            vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
            "the deleted clip's span is silent"
        );
        let before = app.state.committed_arrangement();

        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 8.0));
        let _ = app.song_region_delete();
        assert_eq!(
            app.state.committed_arrangement(),
            before,
            "there is nothing left to delete there"
        );
        assert_eq!(
            window(&app, 0, 4.0, 8.0),
            vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
            "and the span stays silent"
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
        app.select_song_clip(0, ClipId(0))
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

        app.select_song_clip_span(0, ClipId(0), Some((4.0, 12.0)))
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
        app.select_song_clip_span(0, ClipId(0), None)
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

    // --- scene-lane regions (lane spec 8) --------------------------------

    /// A SCENE-LANE marquee copies the scene events inside it, led by the
    /// scene marked at its start so a paste carries the scene markers
    /// rather than inheriting the destination's.
    #[test]
    fn scene_lane_region_copy_carries_the_scene_events() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new_in_lane(0, 1, 4.0, 12.0, true));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        assert!(clipboard.scene_lane);
        assert_eq!(clipboard.scene_events, vec![(0.0, 0), (4.0, 1)]);

        // The identical rectangle swept in a TRACK lane carries no scene
        // events at all — that is the whole point of the bit.
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        assert!(!clipboard.scene_lane);
        assert!(clipboard.scene_events.is_empty());
    }

    /// Pasting a scene-lane rectangle stamps its events at the destination
    /// and leaves everything outside the rectangle playing what it did.
    #[test]
    fn scene_lane_region_paste_stamps_the_scene_events() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new_in_lane(0, 1, 4.0, 12.0, true));
        let clipboard = app.song_region_copy().expect("copy succeeds");
        let depth = app.history.undo_len();

        app.song_region_paste(&clipboard, 16.0).expect("paste succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        assert_eq!(
            scene_lane(&app),
            vec![(0.0, 0), (8.0, 1), (16.0, 0), (20.0, 1)],
            "the copied scene changes land at the destination, relative beats intact"
        );

        undo(&mut app);
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 1)]);
        redo(&mut app);
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 1), (16.0, 0), (20.0, 1)]);
    }

    /// Deleting a scene-lane region removes the scene CHANGES inside it and
    /// restores the scene governing its end, so nothing after the rectangle
    /// moves. It also clears the clips, exactly like a track-lane region.
    #[test]
    fn scene_lane_region_delete_removes_the_scene_changes_and_keeps_the_tail() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new_in_lane(0, 1, 4.0, 12.0, true));
        let depth = app.history.undo_len();

        app.song_region_delete().expect("delete succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        assert_eq!(
            scene_lane(&app),
            vec![(0.0, 0), (12.0, 1)],
            "the change at 8 is gone; scene 1 still governs from the region's end"
        );

        undo(&mut app);
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 1)]);
    }

    /// The same rectangle swept in a TRACK lane never touches the scene lane.
    #[test]
    fn a_track_lane_region_delete_leaves_the_scene_lane_alone() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        app.song_region_delete().expect("delete succeeds");
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 1)]);
    }

    /// The mandatory event at beat 0 can never be removed by a region op
    /// (lane spec 8: the arrangement must start on a scene).
    #[test]
    fn scene_lane_region_ops_never_remove_the_event_at_zero() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new_in_lane(0, 1, 0.0, 12.0, true));
        app.song_region_delete().expect("delete succeeds");
        assert_eq!(
            scene_lane(&app),
            vec![(0.0, 0), (12.0, 1)],
            "beat 0 survives; the tail scene is restored at the region end"
        );
    }

    // --- move (spec 6.2) -------------------------------------------------

    /// The headline contract: the rectangle plays identically somewhere else
    /// and the beats it left go silent. Partially covered clips move only the
    /// part the rectangle covered, phase-rigidly (`offset_steps` untouched).
    #[test]
    fn move_shifts_the_rectangle_and_vacates_the_source() {
        let mut app = app_with_song();
        // Deliberately cutting into the [0,4) and [4,8) clips, so the moved
        // material is a boundary-cut fragment with a non-zero phase.
        app.set_song_region(SongRegionSelection::new(0, 1, 2.0, 6.0));
        let source: Vec<_> = (0..2).map(|track| window(&app, track, 2.0, 6.0)).collect();
        let depth = app.history.undo_len();

        app.song_region_move(10.0).expect("move succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        for track in 0..2 {
            assert_eq!(
                window(&app, track, 12.0, 16.0),
                source[track],
                "track {track} plays the rectangle 10 beats later"
            );
            assert_eq!(
                window(&app, track, 2.0, 6.0),
                vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
                "track {track}'s source rectangle is SILENT — no trimmed leftovers"
            );
        }
        // Phase rigidity at the stored-clip level: 2 beats into a 16-step,
        // 4-beat pattern is 8 steps, and that offset rides along unchanged.
        assert_eq!(
            clips(&app, 0)
                .into_iter()
                .filter(|(start, _, _, _)| *start >= 12.0)
                .collect::<Vec<_>>(),
            vec![
                (12.0, 14.0, LaneSource::Pattern(PatternId(1)), 8.0),
                (14.0, 16.0, LaneSource::Pattern(PatternId(2)), 0.0),
            ]
        );
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 1, 12.0, 16.0)),
            "the highlight follows the move, so a repeated drag chains"
        );

        undo(&mut app);
        for track in 0..2 {
            assert_eq!(
                window(&app, track, 2.0, 6.0),
                source[track],
                "one undo puts track {track} back"
            );
        }
    }

    /// A delta smaller than the region overlaps its own source: clearing the
    /// source BEFORE writing the destination is what keeps the overlap from
    /// erasing the clips the move just placed.
    #[test]
    fn an_overlapping_move_keeps_everything_it_placed() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 0, 4.0, 12.0));
        let source = window(&app, 0, 4.0, 12.0);

        app.song_region_move(4.0).expect("move succeeds");
        assert_eq!(
            window(&app, 0, 8.0, 16.0),
            source,
            "the whole rectangle survives the overlap"
        );
        assert_eq!(
            window(&app, 0, 4.0, 8.0),
            vec![(0.0, 4.0, LaneSource::Empty, 0.0)],
            "only the part of the source the destination does not cover is vacated"
        );
    }

    /// Move follows paste rather than Ableton's Cut Time: running past the
    /// song end extends it, inside the same entry (spec 9's open question).
    #[test]
    fn move_past_the_song_end_extends_it_in_the_same_entry() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 0, 8.0, 16.0));
        let source = window(&app, 0, 8.0, 16.0);
        let depth = app.history.undo_len();

        app.song_region_move(8.0).expect("move succeeds");
        assert_eq!(app.history.undo_len(), depth + 1, "exactly one undo entry");
        assert_eq!(
            app.state.committed_arrangement().expect("arrangement").end_beat,
            24.0
        );
        assert_eq!(window(&app, 0, 16.0, 24.0), source);
    }

    /// A move that would cross beat 0 is refused outright — never silently
    /// truncated to what fits — and a zero delta writes nothing at all.
    #[test]
    fn move_rejects_a_delta_that_would_cross_beat_zero() {
        let mut app = app_with_song();
        app.set_song_region(SongRegionSelection::new(0, 1, 4.0, 12.0));
        let before = app.state.committed_arrangement();
        let depth = app.history.undo_len();

        let error = app.song_region_move(-8.0).expect_err("crosses beat 0");
        assert!(error.contains("before beat 0"), "{error}");
        assert_eq!(app.state.committed_arrangement(), before, "nothing moved");

        app.song_region_move(0.0).expect("a zero delta is not an error");
        assert_eq!(app.history.undo_len(), depth, "and commits no entry");
        assert_eq!(app.state.committed_arrangement(), before);
    }

    /// A SCENE-LANE region carries its scene events, restoring what governed
    /// the vacated span's end so nothing after the move changes scene.
    #[test]
    fn a_scene_lane_region_move_carries_the_scene_events() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new_in_lane(0, 1, 8.0, 12.0, true));
        app.song_region_move(4.0).expect("move succeeds");
        assert_eq!(
            scene_lane(&app),
            vec![(0.0, 0), (12.0, 1)],
            "the scene change moved with the rectangle"
        );
    }

    /// A track-lane region of the same rectangle leaves the scene lane alone,
    /// the same asymmetry copy/paste/delete already follow.
    #[test]
    fn a_track_lane_region_move_leaves_the_scene_lane_alone() {
        let mut app = app_with_scene_change();
        app.set_song_region(SongRegionSelection::new(0, 1, 8.0, 12.0));
        app.song_region_move(4.0).expect("move succeeds");
        assert_eq!(scene_lane(&app), vec![(0.0, 0), (8.0, 1)]);
    }

    /// Spec 6.1: dragging the SELECTED clip moves the object the selection
    /// names, so its one-clip region follows — Cmd-C right after a move must
    /// lift where the clip now is, not where it was.
    #[test]
    fn moving_the_selected_clip_carries_its_one_clip_region() {
        let mut app = app_with_song();
        let clip = clip_at(&app, 0, 4.0);
        app.select_song_clip_span(0, clip, Some((4.0, 8.0)))
            .expect("clip selects");
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 0, 4.0, 8.0))
        );

        app.arr_clip_move(clip, 12.0).expect("clip moves");
        assert!(app.refresh_song_region_for_clip(clip));
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 0, 12.0, 16.0))
        );

        // Some other clip's move leaves the selection where it is.
        let other = clip_at(&app, 1, 4.0);
        assert!(!app.refresh_song_region_for_clip(other));
        assert_eq!(
            app.song_region_selection,
            Some(SongRegionSelection::new(0, 0, 12.0, 16.0))
        );
    }
}
