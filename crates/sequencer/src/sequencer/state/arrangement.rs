//! Arrangement lane model (docs/arrangement-lane-model-spec.md section 6-7).
//!
//! The stored authoring model is lanes: a **scene lane** of scene *changes*
//! and one **track lane** of first-class clips per track. Rows
//! (`ProjectSong`) remain the playback representation, produced from lanes by
//! the pure `compile_arrangement`. This module owns the types, validation
//! (spec 6.1), the resolution accessors (spec 6.2), the compiler (spec 7),
//! and the save-time id mapping (spec 10). `SequencerState` stores the
//! arrangement beside the compiled song and keeps the two in lockstep
//! (`set_committed_arrangement`); the editing primitives live in
//! `app/arr_edit.rs`.

use super::*;

/// Stable logical identity for a clip, mirroring `SongRowId`/`SceneId`:
/// allocated monotonically from `ProjectArrangement::next_clip_id` and never
/// reused within a project.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClipId(pub u64);

/// A scene *change* on the scene lane: inserting or repointing one STAMPS the
/// scene's cells as real clips across its span (spec 6.2/8). The event itself
/// governs nothing at playback — it is a marker plus the gesture that stamped
/// the clips. Spans are derived (event to next event, last event to
/// `end_beat`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneEvent {
    pub start_beat: f64,
    pub scene: usize,
}

/// One clip on a track lane: a half-open span `[start_beat, end_beat)` with a
/// source and a phase anchor. The source encoding matches
/// `ProjectSongTrackOverride`: a take excludes a pattern. Unlike an override,
/// a clip may NOT be sourceless (spec 6.1): silence is the *absence* of a
/// clip, never a stored object, so `LaneSource::Empty` survives only as a
/// compiled override.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrClip {
    pub id: ClipId,
    pub start_beat: f64,
    /// Exclusive; must be `> start_beat`.
    pub end_beat: f64,
    pub pattern_id: Option<u64>,
    /// If `Some`, this clip plays a take (takes spec 6.2) and `pattern_id`
    /// must be `None` (validation 6.1). Serde-defaulted so pattern clips keep
    /// the smaller wire shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_id: Option<u64>,
    /// Start offset into the source in fractional steps of this track's
    /// timebase (takes spec 7): the clip plays source step
    /// `steps(beat - start_beat) + offset_steps`, modulo the pattern length
    /// for patterns; takes are silent past their end. `0.0` — the serde
    /// default — means the clip begins at source step 0.
    #[serde(default, skip_serializing_if = "arr_offset_steps_is_zero")]
    pub offset_steps: f64,
}

fn arr_offset_steps_is_zero(offset: &f64) -> bool {
    *offset == 0.0
}

impl ArrClip {
    /// Pattern clip anchored at source step 0.
    pub fn new(id: ClipId, start_beat: f64, end_beat: f64, pattern_id: Option<u64>) -> Self {
        Self {
            id,
            start_beat,
            end_beat,
            pattern_id,
            take_id: None,
            offset_steps: 0.0,
        }
    }

    /// Take-playing clip (takes spec 6.2).
    pub fn new_take(
        id: ClipId,
        start_beat: f64,
        end_beat: f64,
        take_id: u64,
        offset_steps: f64,
    ) -> Self {
        Self {
            id,
            start_beat,
            end_beat,
            pattern_id: None,
            take_id: Some(take_id),
            offset_steps,
        }
    }

    /// The clip's resolved source (takes spec 6.2), identical in shape to
    /// `ProjectSongTrackOverride::source()`: a take wins over `pattern_id`
    /// (validation forbids carrying both). `Empty` is unreachable for a
    /// *stored* clip — validation rejects it — but the arm stays so the
    /// mapping to `LaneSource` remains total.
    pub fn source(&self) -> LaneSource {
        match (self.take_id, self.pattern_id) {
            (Some(take), _) => LaneSource::Take(TakeId(take)),
            (None, Some(pattern)) => LaneSource::Pattern(PatternId(pattern)),
            (None, None) => LaneSource::Empty,
        }
    }

    /// `true` when `beat` falls inside the clip's half-open span.
    pub fn contains(&self, beat: f64) -> bool {
        beat >= self.start_beat && beat < self.end_beat
    }
}

/// The stored arrangement: a scene lane, one clip lane per track, the song
/// end, the loop flag, and the monotonic clip-id allocator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectArrangement {
    /// Sorted by `start_beat`, strictly increasing, first event at 0.0.
    pub scene_lane: Vec<SceneEvent>,
    /// Outer index is the track; each lane is sorted and non-overlapping.
    #[serde(default)]
    pub track_lanes: Vec<Vec<ArrClip>>,
    pub end_beat: f64,
    #[serde(default)]
    pub loop_enabled: bool,
    /// Monotonic allocator for `ClipId`; never reused within a project.
    pub next_clip_id: u64,
}

impl ProjectArrangement {
    /// An arrangement holding scene 0 from beat 0 and no clips.
    pub fn new(track_count: usize, end_beat: f64) -> Self {
        Self {
            scene_lane: vec![SceneEvent {
                start_beat: 0.0,
                scene: 0,
            }],
            track_lanes: vec![Vec::new(); track_count],
            end_beat,
            loop_enabled: false,
            next_clip_id: 0,
        }
    }

    /// Allocate a fresh `ClipId`. Ids are monotonic and never reused within a
    /// project; exhaustion is an error, mirroring `allocate_row_id`.
    pub fn allocate_clip_id(&mut self) -> Result<ClipId, String> {
        let id = ClipId(self.next_clip_id);
        self.next_clip_id = self
            .next_clip_id
            .checked_add(1)
            .ok_or_else(|| "arrangement clip identity space exhausted".to_string())?;
        Ok(id)
    }

    /// The scene *marked* at `beat` — the last scene event at or before it.
    /// This is the scene the transport and session UI call "current"; it does
    /// NOT decide what any lane plays (spec 6.2: only clips do). `None` only
    /// for a beat before the first event (which validation forbids) or an
    /// empty lane.
    pub fn scene_at_beat(&self, beat: f64) -> Option<usize> {
        self.scene_event_at_beat(beat).map(|event| event.scene)
    }

    /// The governing scene *event* at `beat` — the last event at or before
    /// it. Stamping needs the event itself, not just its scene index: the
    /// event's `start_beat` is the phase anchor of every clip it stamps.
    pub fn scene_event_at_beat(&self, beat: f64) -> Option<&SceneEvent> {
        self.scene_lane
            .iter()
            .rev()
            .find(|event| event.start_beat <= beat)
    }

    /// Spec 6.2 step 1: the clip on `track`'s lane whose span contains
    /// `beat`. Assumes a valid (sorted, non-overlapping) lane.
    pub fn clip_at(&self, track: usize, beat: f64) -> Option<&ArrClip> {
        self.track_lanes
            .get(track)?
            .iter()
            .find(|clip| clip.contains(beat))
    }

    /// Find a clip by id, returning its track lane index alongside it.
    pub fn find_clip(&self, id: ClipId) -> Option<(usize, &ArrClip)> {
        self.track_lanes
            .iter()
            .enumerate()
            .find_map(|(track, lane)| {
                lane.iter()
                    .find(|clip| clip.id == id)
                    .map(|clip| (track, clip))
            })
    }

    /// Check every rule of spec 6.1 against `ctx`. Errors are actionable and
    /// never clamp, reorder, or drop invalid data (the `ProjectSong::validate`
    /// philosophy).
    pub fn validate(&self, ctx: &dyn SongProjectContext) -> Result<(), String> {
        // --- scene lane -------------------------------------------------
        if self.scene_lane.is_empty() {
            return Err("Arrangement must contain at least one scene event".to_string());
        }
        for (idx, event) in self.scene_lane.iter().enumerate() {
            if !event.start_beat.is_finite() || event.start_beat < 0.0 {
                return Err(format!(
                    "Scene event {} start beat {} must be finite and non-negative",
                    idx + 1,
                    event.start_beat
                ));
            }
        }
        let first = self.scene_lane[0].start_beat;
        if first != 0.0 {
            return Err(format!(
                "Scene event 1 must start at beat 0.0, found {first}"
            ));
        }
        for (idx, pair) in self.scene_lane.windows(2).enumerate() {
            if pair[1].start_beat <= pair[0].start_beat {
                return Err(format!(
                    "Scene events {} and {} are not strictly ordered by start beat ({} then {})",
                    idx + 1,
                    idx + 2,
                    pair[0].start_beat,
                    pair[1].start_beat
                ));
            }
        }
        for (idx, event) in self.scene_lane.iter().enumerate() {
            if event.scene >= ctx.song_scene_count() {
                return Err(format!(
                    "Scene event {} references scene {} but the project has {} scene(s)",
                    idx + 1,
                    event.scene + 1,
                    ctx.song_scene_count()
                ));
            }
        }

        // --- lane count -------------------------------------------------
        if self.track_lanes.len() != ctx.song_track_count() {
            return Err(format!(
                "Arrangement has {} track lane(s) but the project has {} track(s)",
                self.track_lanes.len(),
                ctx.song_track_count()
            ));
        }

        // --- end beat (checked before clips so a nonsense end reports
        // itself rather than blaming the first clip that overruns it) -----
        let last_scene_start = self.scene_lane[self.scene_lane.len() - 1].start_beat;
        if !self.end_beat.is_finite() || self.end_beat <= 0.0 {
            return Err(format!(
                "Arrangement end beat {} must be finite and greater than zero",
                self.end_beat
            ));
        }
        if self.end_beat <= last_scene_start {
            return Err(format!(
                "Arrangement end beat {} must be greater than the last scene event's start \
                 beat {}",
                self.end_beat, last_scene_start
            ));
        }

        // --- clips ------------------------------------------------------
        let mut seen_ids = HashSet::new();
        for (track, lane) in self.track_lanes.iter().enumerate() {
            for (idx, clip) in lane.iter().enumerate() {
                if !clip.start_beat.is_finite() || clip.start_beat < 0.0 {
                    return Err(format!(
                        "Track {} clip {} start beat {} must be finite and non-negative",
                        track + 1,
                        idx + 1,
                        clip.start_beat
                    ));
                }
                if !clip.end_beat.is_finite() || clip.end_beat <= clip.start_beat {
                    return Err(format!(
                        "Track {} clip {} ends at beat {} which is not after its start beat {}; \
                         clips must have positive length",
                        track + 1,
                        idx + 1,
                        clip.end_beat,
                        clip.start_beat
                    ));
                }
                if !clip.offset_steps.is_finite() || clip.offset_steps < 0.0 {
                    return Err(format!(
                        "Track {} clip {} offset {} must be a finite, non-negative step count",
                        track + 1,
                        idx + 1,
                        clip.offset_steps
                    ));
                }
                if clip.pattern_id.is_none() && clip.take_id.is_none() {
                    // Spec 6.1/6.2: a span with no clip is silent, so silence
                    // never needs — and never gets — an object of its own. A
                    // sourceless clip would be an invisible "empty clip" the
                    // user cannot see but can collide with.
                    return Err(format!(
                        "Track {} clip {} carries no source; silence is the absence of a clip, \
                         not an empty one — delete it instead",
                        track + 1,
                        idx + 1
                    ));
                }
                if let Some(pattern_id) = clip.pattern_id {
                    if clip.take_id.is_some() {
                        return Err(format!(
                            "Track {} clip {} carries both a take and a pattern; a take clip \
                             must have no pattern id",
                            track + 1,
                            idx + 1
                        ));
                    }
                    if !ctx.song_track_pattern_exists(track, pattern_id) {
                        return Err(format!(
                            "Track {} clip {} references pattern {} which is not in track {}'s \
                             pattern pool",
                            track + 1,
                            idx + 1,
                            pattern_id,
                            track + 1
                        ));
                    }
                }
                if let Some(take_id) = clip.take_id {
                    let Some(total_len) = ctx.song_track_take_len(track, take_id) else {
                        return Err(format!(
                            "Track {} clip {} references take {} which is not in track {}'s \
                             take pool",
                            track + 1,
                            idx + 1,
                            take_id,
                            track + 1
                        ));
                    };
                    if clip.offset_steps >= total_len as f64 {
                        return Err(format!(
                            "Track {} clip {} take offset {} is at or past the take's end \
                             ({} steps); takes never wrap",
                            track + 1,
                            idx + 1,
                            clip.offset_steps,
                            total_len
                        ));
                    }
                }
                if clip.end_beat > self.end_beat {
                    return Err(format!(
                        "Track {} clip {} ends at beat {} which is past the arrangement end \
                         beat {}; extend the arrangement or trim the clip",
                        track + 1,
                        idx + 1,
                        clip.end_beat,
                        self.end_beat
                    ));
                }
                if !seen_ids.insert(clip.id) {
                    return Err(format!(
                        "Track {} clip {} reuses clip id {}; clip ids must be unique",
                        track + 1,
                        idx + 1,
                        clip.id.0
                    ));
                }
                if clip.id.0 >= self.next_clip_id {
                    return Err(format!(
                        "Track {} clip {} has id {} but next_clip_id is {}; \
                         ids must be less than the allocator",
                        track + 1,
                        idx + 1,
                        clip.id.0,
                        self.next_clip_id
                    ));
                }
            }
            for (idx, pair) in lane.windows(2).enumerate() {
                if pair[1].start_beat < pair[0].start_beat {
                    return Err(format!(
                        "Track {} clips {} and {} are not sorted by start beat ({} then {})",
                        track + 1,
                        idx + 1,
                        idx + 2,
                        pair[0].start_beat,
                        pair[1].start_beat
                    ));
                }
                if pair[1].start_beat < pair[0].end_beat {
                    return Err(format!(
                        "Track {} clips {} and {} overlap: clip {} ends at beat {} but clip {} \
                         starts at beat {}",
                        track + 1,
                        idx + 1,
                        idx + 2,
                        idx + 1,
                        pair[0].end_beat,
                        idx + 2,
                        pair[1].start_beat
                    ));
                }
            }
        }

        Ok(())
    }
}

/// Everything compile needs beyond `SongProjectContext`'s existence checks:
/// the scene cells it resolves the backdrop from, and the `steps()` mapping it
/// stamps offsets with. Mirrors `SongApp::pattern_geometry` /
/// `take_step_mapping` and the scene-cell lookup the retired row-split helper
/// did.
///
/// Every method defaults to `None` ("unknown") so contexts that cannot see
/// project internals — `SerializedSongContext` — stay valid; an unknown
/// mapping leaves offsets untouched, the same fallback the row path took, and
/// an unknown scene cell simply materializes no backdrop override.
pub trait SongCompileContext {
    /// The pattern in scene `scene`'s cell for `track`, as a raw pool id.
    fn song_scene_cell(&self, _scene: usize, _track: usize) -> Option<u64> {
        None
    }

    /// The real beat↔step geometry of `pattern_id` in `track`'s pool (takes
    /// spec 7.1/7.2/7.4) — per-step timebase and sync plocks included, so
    /// stamped offsets invert the exact boundaries the runtime resolves them
    /// through.
    fn song_track_pattern_geometry(
        &self,
        _track: usize,
        _pattern_id: u64,
    ) -> Option<PatternStepGeometry> {
        None
    }

    /// `(steps_per_beat, total_len_steps)` for `take_id` on `track` (the
    /// chunk-domain mapping, takes spec 6.1: chunks are `MAX_STEPS`-long
    /// patterns under the first chunk's base timebase).
    fn song_track_take_step_mapping(&self, _track: usize, _take_id: u64) -> Option<(f64, f64)> {
        None
    }
}

impl SongCompileContext for ProjectScenes {
    fn song_scene_cell(&self, scene: usize, track: usize) -> Option<u64> {
        self.scenes
            .get(scene)?
            .cells
            .get(track)
            .copied()
            .flatten()
            .map(|pattern| pattern.0)
    }

    fn song_track_pattern_geometry(
        &self,
        track: usize,
        pattern_id: u64,
    ) -> Option<PatternStepGeometry> {
        let data = self.track_pools.get(track)?.get(PatternId(pattern_id))?;
        Some(data.step_geometry())
    }

    fn song_track_take_step_mapping(&self, track: usize, take_id: u64) -> Option<(f64, f64)> {
        let take = self.take_pools.get(track)?.get(TakeId(take_id))?;
        let first_chunk = self.track_pools.get(track)?.get(*take.chunks.first()?)?;
        let step_beats = first_chunk.track_params.timebase.step_beats(MAX_STEPS);
        (step_beats > 0.0).then(|| (1.0 / step_beats, take.total_len_steps as f64))
    }
}

impl SongCompileContext for SerializedSongContext {}

/// The context a compile needs: the validation surface plus the scene cells
/// and timebase mappings. Blanket-implemented, so any existing
/// `SongProjectContext` gains it for free (with the default "unknown" answers).
pub trait ArrangementContext: SongProjectContext + SongCompileContext {}

impl<T: SongProjectContext + SongCompileContext> ArrangementContext for T {}

/// The one place pattern playback position wraps (clip-edit-target spec 5.1).
///
/// `offset_steps` is the phase *within the clip's loop window* and
/// `delta_steps` an advance along it; the result is the absolute source step
/// `window_start + (offset + delta) mod window_len`. Today every window is
/// `(0, num_steps)`, hardcoded at the call sites — when sub-pattern loop
/// windows land, they land here, not in a three-site semantics hunt. The
/// three historical `rem_euclid` sites all funnel through this helper:
/// `advanced_pattern_offset` below, the runtime advance in
/// `song_playback.rs`, and `restamped_clip` (via `stamped_clip_override` →
/// `advanced_pattern_offset`).
pub fn pattern_play_step(offset_steps: f64, delta_steps: f64, window: (f64, f64)) -> f64 {
    let (window_start, window_len) = window;
    if window_len <= 0.0 {
        return window_start;
    }
    window_start + (offset_steps + delta_steps).rem_euclid(window_len)
}

/// Advance a pattern lane's `offset_steps` by `delta_beats` of playback in the
/// pattern's REAL step geometry (per-step timebase/sync plocks included),
/// normalized into `[0, num_steps)` through the shared window helper.
/// Byte-for-byte the rule in `SongApp::advanced_offset`, including the
/// boundary-epsilon collapse to 0 so a clip landing on a pattern boundary
/// stamps an implicit zero offset.
fn advanced_pattern_offset(
    ctx: &dyn SongCompileContext,
    track: usize,
    pattern_id: u64,
    offset_steps: f64,
    delta_beats: f64,
) -> f64 {
    let Some(geometry) = ctx.song_track_pattern_geometry(track, pattern_id) else {
        return offset_steps;
    };
    let num_steps = geometry.num_steps() as f64;
    // The beat->step advance happens in the real geometry (already wrapped
    // to the pattern); the window helper re-states the wrap so loop windows
    // have one seam to land in (clip-edit-target spec 5.1).
    let advanced = pattern_play_step(
        geometry.advance(offset_steps, delta_beats),
        0.0,
        (0.0, num_steps),
    );
    if advanced < 1e-9 || advanced > num_steps - 1e-9 {
        0.0
    } else {
        advanced
    }
}

/// The launch override a clip contributes at boundary `beat`, with
/// `offset_steps` stamped by the takes spec 7 split rule — the same arithmetic
/// the row path applied to a row split, measured from the clip's start instead
/// of the governing row's.
pub fn stamped_clip_override(
    ctx: &dyn SongCompileContext,
    track: usize,
    clip: &ArrClip,
    beat: f64,
) -> ProjectSongTrackOverride {
    let delta_beats = beat - clip.start_beat;
    if let Some(take_id) = clip.take_id {
        // Take lanes advance linearly, never wrapping (takes spec 6.1); past
        // the take's end the lane becomes an explicit-empty override (the
        // silent tail).
        let Some((steps_per_beat, total_len)) = ctx.song_track_take_step_mapping(track, take_id)
        else {
            return ProjectSongTrackOverride::new_take(track, take_id, clip.offset_steps);
        };
        let advanced = clip.offset_steps + delta_beats * steps_per_beat;
        if advanced >= total_len - 1e-6 {
            ProjectSongTrackOverride::new(track, None)
        } else {
            ProjectSongTrackOverride::new_take(track, take_id, advanced.max(0.0))
        }
    } else if let Some(pattern_id) = clip.pattern_id {
        ProjectSongTrackOverride {
            track,
            pattern_id: Some(pattern_id),
            take_id: None,
            offset_steps: advanced_pattern_offset(
                ctx,
                track,
                pattern_id,
                clip.offset_steps,
                delta_beats,
            ),
        }
    } else {
        // Explicit-empty clip: silence that still occludes the scene cell.
        ProjectSongTrackOverride::new(track, None)
    }
}

/// Re-anchor `clip` so it starts at `beat`, keeping the music it was playing
/// there (spec 8: "left-trims re-stamp `offset_steps` by the split rule").
///
/// The arithmetic is not re-derived: the new source and offset are exactly
/// what `stamped_clip_override` — the compiler's own split rule — produces at
/// `beat`. `None` means the re-anchored clip would have no source at all (a
/// take trimmed past its own end): that span is silent, and silence is the
/// absence of a clip, so the caller DROPS it rather than storing an empty one
/// (spec 6.1).
///
/// `beat` may be *before* `clip.start_beat` (a left-edge grow): pattern
/// offsets wrap backwards through `rem_euclid` and take offsets clamp at 0.
pub fn restamped_clip(
    ctx: &dyn SongCompileContext,
    track: usize,
    clip: &ArrClip,
    beat: f64,
) -> Option<ArrClip> {
    let stamped = stamped_clip_override(ctx, track, clip, beat);
    if stamped.pattern_id.is_none() && stamped.take_id.is_none() {
        return None;
    }
    Some(ArrClip {
        id: clip.id,
        start_beat: beat,
        end_beat: clip.end_beat,
        pattern_id: stamped.pattern_id,
        take_id: stamped.take_id,
        offset_steps: stamped.offset_steps,
    })
}

/// Ableton-style truncation (spec 14, locked): clear `[start, end)` on
/// `track`'s lane so an incoming clip can own it. The incoming clip always
/// wins; nothing is rejected for overlapping.
///
/// Four cases, per existing clip `c`:
///
/// 1. **Disjoint** (`c.end <= start` or `c.start >= end`) — untouched.
/// 2. **Fully covered** (`start <= c.start` and `c.end <= end`) — removed.
/// 3. **Overlapped at one edge** — trimmed to the incoming clip's edge. A
///    *right* trim (`c` starts before `start`) only shortens the span, so the
///    phase anchor is untouched; a *left* trim (`c` ends after `end`)
///    re-stamps `offset_steps` through `restamped_clip`, so the surviving
///    tail keeps playing exactly what it played there (and is dropped
///    outright when nothing is left to play).
/// 4. **Strictly containing** (`c.start < start` and `end < c.end`) — split
///    around the incoming span: the left fragment keeps `c`'s identity and
///    anchor, the right fragment gets a fresh `ClipId` and a re-stamped
///    offset.
///
/// The caller inserts its own clip afterwards (`insert_clip_sorted`); this
/// only makes room. Non-overlap is thus an invariant every write op
/// maintains rather than something validation has to reject at the UI edge.
pub fn occlude_span(
    arr: &mut ProjectArrangement,
    ctx: &dyn SongCompileContext,
    track: usize,
    start: f64,
    end: f64,
) -> Result<(), String> {
    if arr.track_lanes.get(track).is_none() {
        return Err(format!("Track {} has no arrangement lane", track + 1));
    }
    let lane = std::mem::take(&mut arr.track_lanes[track]);
    let mut kept: Vec<ArrClip> = Vec::with_capacity(lane.len() + 1);
    let mut split_tails: Vec<ArrClip> = Vec::new();
    for clip in lane {
        // 1. Disjoint.
        if clip.end_beat <= start || clip.start_beat >= end {
            kept.push(clip);
            continue;
        }
        // 2. Fully covered.
        if clip.start_beat >= start && clip.end_beat <= end {
            continue;
        }
        // 4. The incoming span lands strictly inside.
        if clip.start_beat < start && clip.end_beat > end {
            let mut left = clip;
            left.end_beat = start;
            kept.push(left);
            // A tail with nothing left to play (a take past its end) is
            // dropped, not stored as an empty clip.
            if let Some(mut right) = restamped_clip(ctx, track, &clip, end) {
                right.id = ClipId(0); // reassigned below, after the borrow ends
                split_tails.push(right);
            }
            continue;
        }
        // 3a. Right trim: the clip starts before the span and ends inside it.
        if clip.start_beat < start {
            let mut trimmed = clip;
            trimmed.end_beat = start;
            kept.push(trimmed);
            continue;
        }
        // 3b. Left trim: the clip starts inside the span and ends after it.
        kept.extend(restamped_clip(ctx, track, &clip, end));
    }
    for mut tail in split_tails {
        tail.id = arr.allocate_clip_id()?;
        kept.push(tail);
    }
    kept.sort_by(|a, b| {
        a.start_beat
            .partial_cmp(&b.start_beat)
            .expect("validated clip beats are finite")
    });
    arr.track_lanes[track] = kept;
    Ok(())
}

/// Insert `clip` into `track`'s lane keeping the lane sorted by start beat.
/// The caller is responsible for having made room (`occlude_span`).
pub fn insert_clip_sorted(arr: &mut ProjectArrangement, track: usize, clip: ArrClip) {
    let lane = &mut arr.track_lanes[track];
    let position = lane
        .iter()
        .position(|existing| existing.start_beat > clip.start_beat)
        .unwrap_or(lane.len());
    lane.insert(position, clip);
}

/// Stamp the scene lane's cells as real clips over `[from_beat, to_beat)`
/// (spec 6.2/8).
///
/// This is the operation that makes "everything audible is a visible clip"
/// true. For every scene span intersecting the window, every track whose cell
/// in that scene holds a pattern gets one clip covering the intersection; a
/// track whose cell is empty gets nothing and is silent there. Stamping
/// TRUNCATES: `occlude_span` clears the window first, so re-stamping a span
/// replaces whatever was there, exactly like any other clip write (spec 14,
/// locked).
///
/// **The phase anchor is the global timeline, not the scene event.** A
/// stamped clip carries the free-run offset `steps(start) mod L` (takes spec
/// 7.2), so it plays as though its pattern had been running since beat 0:
/// source step 0 always lands on the same absolute beats no matter where the
/// scene boundary sits. Dragging a boundary therefore changes how MUCH of the
/// pattern you hear, never WHEN its steps fall — "modifying scenes should
/// never change the flow of rhythm of the underlying track pattern clips".
///
/// Anchoring on the event's own `start_beat` instead (rev 2's first attempt)
/// restarted the pattern at step 0 at the boundary, so moving a boundary onto
/// a beat that is not a whole number of pattern cycles shifted every
/// downstream hit off the grid. It also disagreed with capture, which has
/// always stamped performed launches free-run; this is one rule for both.
pub fn stamp_scene_clips(
    arr: &mut ProjectArrangement,
    ctx: &dyn SongCompileContext,
    from_beat: f64,
    to_beat: f64,
) -> Result<(), String> {
    if !(to_beat > from_beat) {
        return Ok(());
    }
    let spans = arrangement_scene_spans(arr);
    let track_count = arr.track_lanes.len();
    for span in spans {
        let start = span.start_beat.max(from_beat);
        let end = span.end_beat.min(to_beat);
        if end <= start {
            continue;
        }
        for track in 0..track_count {
            occlude_span(arr, ctx, track, start, end)?;
            let Some(pattern_id) = ctx.song_scene_cell(span.scene, track) else {
                // No cell: the scene says nothing for this track, so the span
                // stays an honest silent gap.
                continue;
            };
            // Free-run against the global clock (takes spec 7.2), NOT against
            // `span.start_beat`: the grid must not move when the boundary does.
            let offset_steps = advanced_pattern_offset(ctx, track, pattern_id, 0.0, start);
            let id = arr.allocate_clip_id()?;
            insert_clip_sorted(
                arr,
                track,
                ArrClip {
                    id,
                    start_beat: start,
                    end_beat: end,
                    pattern_id: Some(pattern_id),
                    take_id: None,
                    offset_steps,
                },
            );
        }
    }
    Ok(())
}

/// Append the arrangement lane for a newly appended project track and stamp
/// every scene cell that already exists for that track.
///
/// Track creation grows `ProjectScenes` before it publishes the new topology.
/// A committed arrangement must grow in the same operation: leaving the lane
/// count behind makes the arrangement invalid and rejects every subsequent
/// edit. The new lane follows the same visible-clip rule as scene stamping,
/// but touches no existing lane. Bare scene cells remain honest gaps; a
/// materialized cell becomes one clip for every matching scene span.
///
/// Preparation is transactional. Clip ids and phase offsets are computed
/// before `arr` is changed, so allocator exhaustion or a topology mismatch
/// leaves the committed arrangement untouched.
pub fn append_scene_stamped_track_lane(
    arr: &mut ProjectArrangement,
    ctx: &dyn SongCompileContext,
    track: usize,
) -> Result<(), String> {
    if arr.track_lanes.len() != track {
        return Err(format!(
            "Cannot append arrangement lane for track {}: the arrangement currently has {} \
             track lane(s)",
            track + 1,
            arr.track_lanes.len()
        ));
    }

    let prepared = arrangement_scene_spans(arr)
        .into_iter()
        .filter_map(|span| {
            let pattern_id = ctx.song_scene_cell(span.scene, track)?;
            let offset_steps =
                advanced_pattern_offset(ctx, track, pattern_id, 0.0, span.start_beat);
            Some((
                span.start_beat,
                span.end_beat,
                pattern_id,
                offset_steps,
            ))
        })
        .collect::<Vec<_>>();
    let clip_count = u64::try_from(prepared.len())
        .map_err(|_| "arrangement clip count exceeds the identity space".to_string())?;
    let next_clip_id = arr
        .next_clip_id
        .checked_add(clip_count)
        .ok_or_else(|| "arrangement clip identity space exhausted".to_string())?;

    let lane = prepared
        .into_iter()
        .enumerate()
        .map(|(index, (start_beat, end_beat, pattern_id, offset_steps))| ArrClip {
            id: ClipId(arr.next_clip_id + index as u64),
            start_beat,
            end_beat,
            pattern_id: Some(pattern_id),
            take_id: None,
            offset_steps,
        })
        .collect();
    arr.next_clip_id = next_clip_id;
    arr.track_lanes.push(lane);
    Ok(())
}

/// One derived scene span for the UI read surface (spec 12): a scene EVENT
/// plus the beat it runs to (the next event's start, else the arrangement
/// end). One span per event and no more — this is the surface that makes the
/// jagged scene lane structurally impossible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSpan {
    pub start_beat: f64,
    pub end_beat: f64,
    pub scene: usize,
}

/// One span of the retired **backdrop** model: a stretch of a track lane with
/// NO clip over it, where the governing scene's cell used to show through.
///
/// The backdrop was removed (spec 6.2, "rejected models"): a gap is silence
/// now. This type survives for exactly one purpose — migrating project files
/// written under the old rule, where those gaps really did sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyBackdropSpan {
    pub start_beat: f64,
    pub end_beat: f64,
    pub scene: usize,
    pub pattern_id: u64,
    /// The pattern phase at `start_beat`, anchored on the scene event — what
    /// the old compiler materialized for a lane riding the backdrop.
    pub offset_steps: f64,
}

/// Derive the scene spans (spec 12). Each event runs until the next one, and
/// the last runs to `end_beat`; events at or past `end_beat` produce nothing.
pub fn arrangement_scene_spans(arr: &ProjectArrangement) -> Vec<SceneSpan> {
    let mut spans = Vec::with_capacity(arr.scene_lane.len());
    for (index, event) in arr.scene_lane.iter().enumerate() {
        if event.start_beat >= arr.end_beat {
            continue;
        }
        let end_beat = arr
            .scene_lane
            .get(index + 1)
            .map(|next| next.start_beat.min(arr.end_beat))
            .unwrap_or(arr.end_beat);
        if end_beat <= event.start_beat {
            continue;
        }
        spans.push(SceneSpan {
            start_beat: event.start_beat,
            end_beat,
            scene: event.scene,
        });
    }
    spans
}

/// Derive what the retired backdrop model made a lane's GAPS sound like, one
/// list per track lane. Migration input only (`migrate_legacy_backdrops`).
///
/// Every gap in a lane — before the first clip, between clips, after the last
/// — is intersected with the scene spans, and each intersection where the
/// governing scene has a cell for the track yields one span. A scene cell
/// that is empty contributes nothing: the lane really was silent there too.
pub fn legacy_backdrop_spans(
    arr: &ProjectArrangement,
    ctx: &dyn SongCompileContext,
) -> Vec<Vec<LegacyBackdropSpan>> {
    let scene_spans = arrangement_scene_spans(arr);
    arr.track_lanes
        .iter()
        .enumerate()
        .map(|(track, lane)| {
            // Lane gaps, in order. Clips are sorted and non-overlapping.
            let mut gaps: Vec<(f64, f64)> = Vec::new();
            let mut cursor = 0.0f64;
            for clip in lane {
                if clip.start_beat > cursor {
                    gaps.push((cursor, clip.start_beat.min(arr.end_beat)));
                }
                cursor = cursor.max(clip.end_beat);
            }
            if cursor < arr.end_beat {
                gaps.push((cursor, arr.end_beat));
            }
            let mut spans = Vec::new();
            for (gap_start, gap_end) in gaps {
                if gap_end <= gap_start {
                    continue;
                }
                for scene_span in &scene_spans {
                    let start = gap_start.max(scene_span.start_beat);
                    let end = gap_end.min(scene_span.end_beat);
                    if end <= start {
                        continue;
                    }
                    let Some(pattern_id) = ctx.song_scene_cell(scene_span.scene, track) else {
                        continue;
                    };
                    spans.push(LegacyBackdropSpan {
                        start_beat: start,
                        end_beat: end,
                        scene: scene_span.scene,
                        pattern_id,
                        offset_steps: advanced_pattern_offset(
                            ctx,
                            track,
                            pattern_id,
                            0.0,
                            start - scene_span.start_beat,
                        ),
                    });
                }
            }
            spans
        })
        .collect()
}

/// Migrate an arrangement authored under the retired **backdrop** model so it
/// sounds identical under the current one (spec 10, v5 → v6).
///
/// Under v5 a lane gap played the governing scene's cell; it is silence now.
/// So: freeze what every gap sounded like into real clips, and drop the old
/// explicit-empty clips — those spans were deliberate silence, which the new
/// model spells as a gap. The result compiles to the same audible song, phase
/// offsets included, and contains nothing but ordinary clips the user can see
/// and delete.
pub fn migrate_legacy_backdrops(
    arr: &ProjectArrangement,
    ctx: &dyn SongCompileContext,
) -> Result<ProjectArrangement, String> {
    // Computed BEFORE the empty clips are dropped: an explicit-empty clip
    // occluded the backdrop, so its span must stay a gap.
    let backdrops = legacy_backdrop_spans(arr, ctx);
    let mut migrated = arr.clone();
    for lane in &mut migrated.track_lanes {
        lane.retain(|clip| clip.pattern_id.is_some() || clip.take_id.is_some());
    }
    for (track, spans) in backdrops.iter().enumerate() {
        for span in spans {
            let id = migrated.allocate_clip_id()?;
            insert_clip_sorted(
                &mut migrated,
                track,
                ArrClip {
                    id,
                    start_beat: span.start_beat,
                    end_beat: span.end_beat,
                    pattern_id: Some(span.pattern_id),
                    take_id: None,
                    offset_steps: span.offset_steps,
                },
            );
        }
    }
    Ok(migrated)
}

/// Spec 7: compile lanes into the playback row model.
///
/// The boundary set is every scene-event start plus every clip start and end
/// below `end_beat` (compared exactly — gestures already quantize). Each
/// boundary becomes one row carrying the marked scene plus **one override per
/// track**: the covering clip's, phase-stamped by the split rule, or an
/// explicit-empty override for a lane no clip covers. That second case is the
/// crux of the model: a row with no override for a track resolves to the
/// scene cell in `preflight_runtime_song`, which is exactly the backdrop
/// fallback the model removed, so an uncovered lane MUST say "silent" out
/// loud. Adjacent identical rows collapse (`normalize`), then ids are assigned
/// by index so equal input always compiles to an identical row layout.
pub fn compile_arrangement<C: ArrangementContext>(
    arr: &ProjectArrangement,
    ctx: &C,
) -> Result<ProjectSong, String> {
    arr.validate(ctx)?;

    // 1. Boundary set.
    let mut boundaries: Vec<f64> = Vec::new();
    for event in &arr.scene_lane {
        if event.start_beat < arr.end_beat {
            boundaries.push(event.start_beat);
        }
    }
    for lane in &arr.track_lanes {
        for clip in lane {
            for edge in [clip.start_beat, clip.end_beat] {
                if edge < arr.end_beat {
                    boundaries.push(edge);
                }
            }
        }
    }
    boundaries.sort_by(|a, b| a.partial_cmp(b).expect("validated beats are finite"));
    boundaries.dedup();

    // 2. One row per boundary. Boundaries ascend and every lane is sorted and
    // non-overlapping, so each lane keeps a forward-only cursor instead of
    // rescanning: compile stays linear in (boundaries + clips).
    let mut rows: Vec<ProjectSongRow> = Vec::with_capacity(boundaries.len());
    let mut cursors = vec![0usize; arr.track_lanes.len()];
    for beat in boundaries {
        let event = arr
            .scene_event_at_beat(beat)
            .ok_or_else(|| format!("Arrangement has no scene governing beat {beat}"))?;
        let scene = event.scene;
        let mut overrides = Vec::new();
        for (track, lane) in arr.track_lanes.iter().enumerate() {
            let cursor = &mut cursors[track];
            while lane.get(*cursor).is_some_and(|clip| clip.end_beat <= beat) {
                *cursor += 1;
            }
            match lane.get(*cursor).filter(|clip| clip.contains(beat)) {
                Some(clip) => overrides.push(stamped_clip_override(ctx, track, clip, beat)),
                // No clip covers the beat: the lane is SILENT. An absent
                // override would resolve to the row's scene cell, so silence
                // has to be stated explicitly.
                None => overrides.push(ProjectSongTrackOverride::new(track, None)),
            }
        }
        rows.push(ProjectSongRow {
            // Replaced below; ids are positional so they must be assigned
            // after normalization collapses redundant rows.
            id: SongRowId(0),
            start_beat: beat,
            scene,
            overrides,
        });
    }

    let mut song = ProjectSong {
        rows,
        end_beat: arr.end_beat,
        loop_enabled: arr.loop_enabled,
        next_row_id: 0,
    };

    // 3. Normalize, then 4. deterministic ids.
    song.normalize();
    for (idx, row) in song.rows.iter_mut().enumerate() {
        row.id = SongRowId(idx as u64);
    }
    song.next_row_id = song.rows.len() as u64;

    // 5. Debug-only self-check: compile output is correct by construction, so
    // this guards the compiler rather than the caller.
    if cfg!(debug_assertions) {
        song.validate(ctx)
            .map_err(|err| format!("compiled arrangement is invalid: {err}"))?;
    }
    Ok(song)
}

/// The inverse of `compile_arrangement`: derive lanes from a row list (spec 8).
///
/// Rows carry no clip identity, so the mapping is unambiguous:
///
/// - A row's scene becomes a `SceneEvent` only when it differs from the
///   previous row's scene (the row model's scene column fragments at every
///   row; that is exactly the "jagged lane" the lane model removes).
/// - Each scene event STAMPS the scene's cells as clips across its span
///   (`stamp_scene_clips`), exactly as inserting one interactively does.
/// - A per-track override then truncates on top: it opens a clip that runs
///   until the first later row that changes that lane; a row that drops the
///   override closes the clip at its beat (the stamped scene clip resumes
///   underneath), and an explicit-empty override carves a silent hole.
/// - "Changes that lane" is decided by *compiling the open clip forward*: a
///   later row's override merges into the open clip only when the clip would
///   compile to exactly that override at that beat
///   (`stamped_clip_override(clip, beat) == declared`). Anything else — a
///   different source, or the same source re-anchored to a phase the clip
///   would not have reached — closes the clip and opens a new one. Source
///   equality alone would silently swallow a deliberate retrigger.
///
/// The guarantee this buys, for any `rows` a row primitive produced: every
/// row start is a boundary of the compiled arrangement, and every declared
/// override reappears verbatim on its row. Compile always emits *more*
/// overrides than the rows had — every unmentioned lane gets the stamped
/// scene cell at its true phase, or an explicit-empty — so
/// `compile_arrangement(lowered)` is the input song with every lane's
/// resolution spelled out rather than left to the row's scene.
///
/// `rows` must already be sorted by `start_beat` with canonical (track-sorted,
/// duplicate-free) overrides — everything `ProjectSong::validate` guarantees.
/// `ctx` has to see the live scenes: the phase arithmetic needs the timebases.
pub fn lower_rows_to_arrangement<C: ArrangementContext>(
    rows: &[ProjectSongRow],
    end_beat: f64,
    loop_enabled: bool,
    track_count: usize,
    next_clip_id: u64,
    ctx: &C,
) -> Result<ProjectArrangement, String> {
    if rows.is_empty() {
        return Err("An arrangement needs at least one row to lower".to_string());
    }
    let last_start = rows[rows.len() - 1].start_beat;
    if !end_beat.is_finite() || end_beat <= last_start {
        // Rows at or past the end would lower to zero-length clips and
        // silently vanish; `ProjectSong::validate` words it the same way.
        return Err(format!(
            "Song end beat {end_beat} must be finite and greater than the last row's start \
             beat {last_start}"
        ));
    }

    let mut arrangement = ProjectArrangement {
        scene_lane: Vec::new(),
        track_lanes: vec![Vec::new(); track_count],
        end_beat,
        loop_enabled,
        next_clip_id,
    };

    // Scene lane: one event per *change*.
    for row in rows {
        let changed = match arrangement.scene_lane.last() {
            Some(event) => event.scene != row.scene,
            None => true,
        };
        if changed {
            arrangement.scene_lane.push(SceneEvent {
                start_beat: row.start_beat,
                scene: row.scene,
            });
        }
    }

    // The scene events stamp their cells first — that is what a scene event
    // DOES (spec 6.2) — and the declared overrides then truncate on top.
    stamp_scene_clips(&mut arrangement, ctx, 0.0, end_beat)?;

    // Track lanes: one run per contiguous stretch of a lane's launch state.
    for track in 0..track_count {
        let mut runs: Vec<ArrClip> = Vec::new();
        let mut open: Option<ArrClip> = None;
        for row in rows {
            match row.overrides.iter().find(|over| over.track == track) {
                Some(declared) => {
                    let continues = open.as_ref().is_some_and(|clip| {
                        stamped_clip_override(ctx, track, clip, row.start_beat) == *declared
                    });
                    if continues {
                        continue;
                    }
                    if let Some(mut clip) = open.take() {
                        clip.end_beat = row.start_beat;
                        runs.push(clip);
                    }
                    open = Some(ArrClip {
                        id: ClipId(0), // assigned when the run is applied
                        start_beat: row.start_beat,
                        end_beat,
                        pattern_id: declared.pattern_id,
                        take_id: declared.take_id,
                        offset_steps: declared.offset_steps,
                    });
                }
                None => {
                    if let Some(mut clip) = open.take() {
                        clip.end_beat = row.start_beat;
                        runs.push(clip);
                    }
                }
            }
        }
        runs.extend(open.take());
        for run in runs {
            apply_lowered_run(&mut arrangement, ctx, track, run)?;
        }
    }

    Ok(arrangement)
}

/// Lay one declared run over the stamped scene clips, truncating them like
/// any other clip write. A run with no source is a declared explicit-empty
/// override: it carves a silent hole and stores nothing (spec 6.1).
fn apply_lowered_run(
    arrangement: &mut ProjectArrangement,
    ctx: &dyn SongCompileContext,
    track: usize,
    mut clip: ArrClip,
) -> Result<(), String> {
    if clip.end_beat <= clip.start_beat {
        return Ok(());
    }
    occlude_span(arrangement, ctx, track, clip.start_beat, clip.end_beat)?;
    if clip.pattern_id.is_none() && clip.take_id.is_none() {
        return Ok(());
    }
    clip.id = arrangement.allocate_clip_id()?;
    insert_clip_sorted(arrangement, track, clip);
    Ok(())
}

/// Translate a live committed arrangement into the id domain the project
/// loader rebuilds (spec 10) — the sibling of `song_for_serialization`, with
/// the same "scene index + 1" mapping and the same refusal to save a
/// reference the project format cannot persist.
///
/// Pools are reconstructed from scene cells on load, so track `t`'s cell in
/// scene `j` becomes `PatternId(j + 1)`. A clip referencing a pattern that is
/// in no scene cell is not persisted by the project format at all, so saving
/// it is rejected — naming the clip and its track — rather than silently
/// dropped. Take clips need no mapping: take ids are stable across save/load.
pub fn arrangement_for_serialization(
    arrangement: &ProjectArrangement,
    scenes: &ProjectScenes,
) -> Result<ProjectArrangement, String> {
    let mut serialized = arrangement.clone();
    for (track, lane) in serialized.track_lanes.iter_mut().enumerate() {
        for (idx, clip) in lane.iter_mut().enumerate() {
            // Explicit-empty and take clips carry no pool id.
            let Some(live_raw) = clip.pattern_id else {
                continue;
            };
            let live_id = PatternId(live_raw);
            let scene_idx = scenes
                .scenes
                .iter()
                .position(|scene| scene.cells.get(track).copied().flatten() == Some(live_id))
                .ok_or_else(|| {
                    format!(
                        "Track {} clip {} (beats {}-{}) references pattern {} which is not \
                         assigned to any scene cell and cannot be saved; assign it to a scene \
                         cell or change the clip",
                        track + 1,
                        idx + 1,
                        clip.start_beat,
                        clip.end_beat,
                        live_raw
                    )
                })?;
            clip.pattern_id = Some(scene_idx as u64 + 1);
        }
    }
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared window helper (clip-edit-target spec 5.1): today's windows
    /// are always `(0, num_steps)`, so it must be byte-for-byte the old
    /// `rem_euclid` rule — including backwards wrap — and window-relative
    /// when a real window start arrives.
    #[test]
    fn pattern_play_step_matches_the_rem_euclid_rule_and_honours_windows() {
        assert_eq!(pattern_play_step(0.0, 4.0, (0.0, 16.0)), 4.0);
        assert_eq!(pattern_play_step(12.0, 8.0, (0.0, 16.0)), 4.0);
        // Backwards wrap (a left-edge grow re-stamps through the same rule).
        assert_eq!(pattern_play_step(4.0, -12.0, (0.0, 16.0)), 8.0);
        // Exact boundary lands on 0, not num_steps.
        assert_eq!(pattern_play_step(0.0, 16.0, (0.0, 16.0)), 0.0);
        // A degenerate window collapses to its start instead of NaN.
        assert_eq!(pattern_play_step(3.0, 5.0, (0.0, 0.0)), 0.0);
        // Window-relative: the phase wraps INSIDE the window.
        assert_eq!(pattern_play_step(2.0, 5.0, (8.0, 4.0)), 8.0 + 3.0);
    }

    // --- fixtures (song.rs test vocabulary) -----------------------------

    fn clip(id: u64, start: f64, end: f64, pattern_id: u64) -> ArrClip {
        ArrClip::new(ClipId(id), start, end, Some(pattern_id))
    }

    /// A clip with no source. Only validation and the migration path may see
    /// one — the primitives refuse to make them (spec 6.1).
    fn sourceless_clip(id: u64, start: f64, end: f64) -> ArrClip {
        ArrClip::new(ClipId(id), start, end, None)
    }

    fn ev(start_beat: f64, scene: usize) -> SceneEvent {
        SceneEvent { start_beat, scene }
    }

    fn over(track: usize, pattern_id: u64) -> ProjectSongTrackOverride {
        ProjectSongTrackOverride::new(track, Some(pattern_id))
    }

    fn over_at(track: usize, pattern_id: u64, offset_steps: f64) -> ProjectSongTrackOverride {
        ProjectSongTrackOverride {
            track,
            pattern_id: Some(pattern_id),
            take_id: None,
            offset_steps,
        }
    }

    fn empty_over(track: usize) -> ProjectSongTrackOverride {
        ProjectSongTrackOverride::new(track, None)
    }

    fn row(
        id: u64,
        start_beat: f64,
        scene: usize,
        overrides: Vec<ProjectSongTrackOverride>,
    ) -> ProjectSongRow {
        ProjectSongRow {
            id: SongRowId(id),
            start_beat,
            scene,
            overrides,
        }
    }

    fn song(rows: Vec<ProjectSongRow>, end_beat: f64) -> ProjectSong {
        let next_row_id = rows.len() as u64;
        ProjectSong {
            rows,
            end_beat,
            loop_enabled: false,
            next_row_id,
        }
    }

    /// Two-track, three-scene project. Per-track pool ids are 1..=3 with
    /// scene j's cell holding PatternId(j + 1) — the rebuilt-on-load shape.
    /// Every pattern is the default 16 steps at a sixteenth timebase, so
    /// `steps_per_beat == 4.0` and the pattern length is 4 beats.
    fn test_scenes() -> ProjectScenes {
        let snapshots = vec![
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
        ];
        ProjectScenes::from_pattern_snapshots(&snapshots, 0)
    }

    /// `test_scenes` plus one 300-step, two-chunk take on track 0. At four
    /// steps per beat the take is playable for 75 beats.
    fn scenes_with_take() -> (ProjectScenes, TakeId) {
        let mut scenes = test_scenes();
        let chunk_data = scenes.track_pools[0].get(PatternId(1)).unwrap();
        let chunk_a = scenes.track_pools[0].insert(chunk_data.clone());
        let chunk_b = scenes.track_pools[0].insert(chunk_data);
        let sound = scenes.track_pools[0].refs(chunk_a).expect("chunk refs");
        let take = scenes.take_pools[0].insert(None, vec![chunk_a, chunk_b], 300, sound);
        (scenes, take)
    }

    fn arrangement(
        scene_lane: Vec<SceneEvent>,
        lanes: Vec<Vec<ArrClip>>,
        end_beat: f64,
    ) -> ProjectArrangement {
        let next_clip_id = lanes
            .iter()
            .flatten()
            .map(|clip| clip.id.0 + 1)
            .max()
            .unwrap_or(0);
        ProjectArrangement {
            scene_lane,
            track_lanes: lanes,
            end_beat,
            loop_enabled: false,
            next_clip_id,
        }
    }

    fn valid_arrangement() -> ProjectArrangement {
        arrangement(
            vec![ev(0.0, 0), ev(32.0, 1)],
            vec![vec![clip(0, 4.0, 8.0, 2)], vec![clip(1, 16.0, 24.0, 3)]],
            64.0,
        )
    }

    /// Compile and assert the row model the playback engine gets is valid.
    fn compile_ok(arr: &ProjectArrangement, scenes: &ProjectScenes) -> ProjectSong {
        let compiled = compile_arrangement(arr, scenes).expect("arrangement compiles");
        compiled
            .validate(scenes)
            .expect("compile output must pass ProjectSong::validate");
        compiled
    }

    // --- fixture sanity -------------------------------------------------

    #[test]
    fn test_patterns_are_four_steps_per_beat() {
        let scenes = test_scenes();
        let geometry = scenes
            .song_track_pattern_geometry(0, 1)
            .expect("fixture pattern resolves");
        assert_eq!(geometry.num_steps(), 16, "fixtures assume 16 steps");
        assert_eq!(
            geometry.cycle_beats(),
            4.0,
            "fixtures assume sixteenth-note steps (4 steps per beat)"
        );
        assert_eq!(geometry.steps_at_beats(1.5), 6.0);
        assert_eq!(geometry.beats_at_steps(6.0), 1.5);
        let (scenes, take) = scenes_with_take();
        assert_eq!(
            scenes.song_track_take_step_mapping(0, take.0),
            Some((4.0, 300.0))
        );
    }

    #[test]
    fn valid_arrangement_passes_validation() {
        valid_arrangement()
            .validate(&test_scenes())
            .expect("arrangement should validate");
    }

    // --- validation (spec 6.1) ------------------------------------------

    #[test]
    fn validate_rejects_empty_scene_lane() {
        let mut arr = valid_arrangement();
        arr.scene_lane.clear();
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("at least one scene event"), "{err}");
    }

    #[test]
    fn validate_rejects_first_scene_event_off_zero() {
        let mut arr = valid_arrangement();
        arr.scene_lane[0].start_beat = 1.0;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("must start at beat 0.0"), "{err}");
    }

    #[test]
    fn validate_rejects_non_increasing_scene_events() {
        let mut arr = valid_arrangement();
        arr.scene_lane[1].start_beat = 0.0;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("strictly ordered"), "{err}");
    }

    #[test]
    fn validate_rejects_non_finite_scene_event_beat() {
        let mut arr = valid_arrangement();
        arr.scene_lane[1].start_beat = f64::NAN;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("finite and non-negative"), "{err}");
    }

    #[test]
    fn validate_rejects_out_of_range_scene() {
        let mut arr = valid_arrangement();
        arr.scene_lane[1].scene = 3;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("references scene 4"), "{err}");
    }

    #[test]
    fn validate_rejects_bad_end_beat() {
        let mut arr = valid_arrangement();
        arr.end_beat = 0.0;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("greater than zero"), "{err}");

        let mut arr = valid_arrangement();
        arr.end_beat = f64::INFINITY;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("finite"), "{err}");

        // End at/before the last scene event.
        let mut arr = valid_arrangement();
        arr.end_beat = 32.0;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("last scene event"), "{err}");

        // End before a clip's end.
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 40.0, 1)], Vec::new()],
            32.0,
        );
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("past the arrangement end beat"), "{err}");
        arr.end_beat = 40.0;
        arr.validate(&test_scenes())
            .expect("a clip ending exactly at end_beat is legal");
    }

    #[test]
    fn validate_rejects_unsorted_and_overlapping_clips() {
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, 16.0, 20.0, 1), clip(2, 4.0, 8.0, 1)];
        arr.next_clip_id = 3;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("not sorted by start beat"), "{err}");

        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, 4.0, 12.0, 1), clip(2, 8.0, 16.0, 1)];
        arr.next_clip_id = 3;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("overlap"), "{err}");
    }

    #[test]
    fn validate_accepts_adjacent_same_source_clips() {
        // Unlike adjacent identical rows, two back-to-back clips of the same
        // pattern are distinct objects the user made (spec 6.1).
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, 4.0, 8.0, 1), clip(2, 8.0, 12.0, 1)];
        arr.next_clip_id = 3;
        arr.validate(&test_scenes())
            .expect("adjacent same-source clips are legal");
    }

    #[test]
    fn validate_rejects_zero_and_negative_length_clips() {
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, 4.0, 4.0, 1)];
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("positive length"), "{err}");

        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, 8.0, 4.0, 1)];
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("positive length"), "{err}");

        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, -4.0, 4.0, 1)];
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("finite and non-negative"), "{err}");
    }

    /// Spec 6.1: silence is the absence of a clip. A sourceless clip is an
    /// invisible object the user cannot see but can collide with, so the
    /// model refuses to hold one.
    #[test]
    fn validate_rejects_a_sourceless_clip() {
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![sourceless_clip(0, 4.0, 8.0)];
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("carries no source"), "{err}");
        assert!(err.contains("Track 1 clip 1"), "{err}");
    }

    #[test]
    fn validate_rejects_missing_pattern() {
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![clip(0, 4.0, 8.0, 9)];
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("pattern 9"), "{err}");
        assert!(err.contains("Track 1"), "{err}");
    }

    #[test]
    fn validate_rejects_negative_and_non_finite_offsets() {
        let mut arr = valid_arrangement();
        arr.track_lanes[0][0].offset_steps = -1.0;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("non-negative step count"), "{err}");

        let mut arr = valid_arrangement();
        arr.track_lanes[0][0].offset_steps = f64::NAN;
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("non-negative step count"), "{err}");
    }

    #[test]
    fn validate_checks_take_clips_against_the_take_pool() {
        let (scenes, take) = scenes_with_take();
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![ArrClip::new_take(ClipId(0), 4.0, 8.0, take.0, 12.5)];
        arr.validate(&scenes).expect("take clip validates");

        // Unknown take id.
        arr.track_lanes[0] = vec![ArrClip::new_take(ClipId(0), 4.0, 8.0, 99, 0.0)];
        let err = arr.validate(&scenes).unwrap_err();
        assert!(err.contains("take 99"), "{err}");

        // Take and pattern on the same clip.
        arr.track_lanes[0] = vec![ArrClip {
            id: ClipId(0),
            start_beat: 4.0,
            end_beat: 8.0,
            pattern_id: Some(1),
            take_id: Some(take.0),
            offset_steps: 0.0,
        }];
        let err = arr.validate(&scenes).unwrap_err();
        assert!(err.contains("both a take and a pattern"), "{err}");

        // Offset at/past the take end (takes never wrap).
        arr.track_lanes[0] = vec![ArrClip::new_take(ClipId(0), 4.0, 8.0, take.0, 300.0)];
        let err = arr.validate(&scenes).unwrap_err();
        assert!(err.contains("past the take's end"), "{err}");
    }

    #[test]
    fn validate_rejects_wrong_lane_count() {
        let mut arr = valid_arrangement();
        arr.track_lanes.pop();
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("1 track lane(s)"), "{err}");
        assert!(err.contains("2 track(s)"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_and_out_of_range_clip_ids() {
        let mut arr = valid_arrangement();
        arr.track_lanes[1][0].id = ClipId(0);
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("reuses clip id 0"), "{err}");

        let mut arr = valid_arrangement();
        arr.track_lanes[1][0].id = ClipId(7);
        let err = arr.validate(&test_scenes()).unwrap_err();
        assert!(err.contains("next_clip_id"), "{err}");
    }

    #[test]
    fn allocate_clip_id_is_monotonic_and_errors_on_exhaustion() {
        let mut arr = valid_arrangement();
        assert_eq!(arr.allocate_clip_id().unwrap(), ClipId(2));
        assert_eq!(arr.allocate_clip_id().unwrap(), ClipId(3));
        assert_eq!(arr.next_clip_id, 4);

        arr.next_clip_id = u64::MAX;
        let err = arr.allocate_clip_id().unwrap_err();
        assert!(err.contains("exhausted"), "{err}");
        assert_eq!(arr.next_clip_id, u64::MAX);
    }

    // --- resolution accessors (spec 6.2) --------------------------------

    #[test]
    fn scene_and_clip_accessors_resolve_spans() {
        let arr = valid_arrangement();
        assert_eq!(arr.scene_at_beat(0.0), Some(0));
        assert_eq!(arr.scene_at_beat(31.999), Some(0));
        assert_eq!(arr.scene_at_beat(32.0), Some(1));
        assert!(arr.scene_at_beat(-1.0).is_none());

        assert_eq!(arr.clip_at(0, 4.0).map(|c| c.id), Some(ClipId(0)));
        assert_eq!(arr.clip_at(0, 7.999).map(|c| c.id), Some(ClipId(0)));
        assert!(arr.clip_at(0, 8.0).is_none(), "spans are half-open");
        assert!(arr.clip_at(0, 3.999).is_none());
        assert_eq!(arr.find_clip(ClipId(1)).map(|(t, _)| t), Some(1));
        assert!(arr.find_clip(ClipId(9)).is_none());
    }

    // --- compile (spec 7) -----------------------------------------------

    #[test]
    fn compile_scene_lane_only_emits_one_row_per_scene_event() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0), ev(16.0, 1), ev(48.0, 2)],
            vec![Vec::new(), Vec::new()],
            64.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        // No clips anywhere means SILENCE everywhere, and silence has to be
        // stated: an absent override would resolve to the row's scene cell.
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, vec![empty_over(0), empty_over(1)]),
                    row(1, 16.0, 1, vec![empty_over(0), empty_over(1)]),
                    row(2, 48.0, 2, vec![empty_over(0), empty_over(1)]),
                ],
                64.0
            )
        );
    }

    #[test]
    fn compile_clip_inside_one_scene_span_emits_before_during_after() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 4.0, 8.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, vec![empty_over(0), empty_over(1)]),
                    row(1, 4.0, 0, vec![over(0, 2), empty_over(1)]),
                    row(2, 8.0, 0, vec![empty_over(0), empty_over(1)]),
                ],
                16.0
            )
        );
    }

    #[test]
    fn compile_keeps_a_clip_opaque_across_a_scene_change_and_restamps_phase() {
        // The whole point of the lane pivot: the clip survives the scene
        // change beneath it, with its offset advanced by the split rule.
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0), ev(7.0, 1)],
            vec![vec![clip(0, 4.0, 16.0, 2)], Vec::new()],
            32.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        // steps(7.0 - 4.0) = 3 beats * 4 steps/beat = 12 steps, mod 16.
        //
        // The scene change at beat 7 does NOT interrupt the clip, and the last
        // row is where the clip ends: nothing was stamped there, so the lane
        // simply goes silent.
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, vec![empty_over(0), empty_over(1)]),
                    row(1, 4.0, 0, vec![over(0, 2), empty_over(1)]),
                    row(2, 7.0, 1, vec![over_at(0, 2, 12.0), empty_over(1)]),
                    row(3, 16.0, 1, vec![empty_over(0), empty_over(1)]),
                ],
                32.0
            )
        );
        assert_eq!(compiled.rows[2].overrides[0].offset_steps, 12.0);
    }

    #[test]
    fn compile_wraps_the_stamped_offset_at_the_pattern_length() {
        let scenes = test_scenes();
        // Scene change 5.5 beats into the clip: 22 steps, mod 16 == 6.
        let arr = arrangement(
            vec![ev(0.0, 0), ev(5.5, 1)],
            vec![vec![clip(0, 0.0, 16.0, 2)], Vec::new()],
            32.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        assert_eq!(
            compiled.rows[1].overrides,
            vec![over_at(0, 2, 6.0), empty_over(1)]
        );
    }

    /// Find the compiled row starting exactly at `beat`.
    fn row_at(song: &ProjectSong, beat: f64) -> &ProjectSongRow {
        song.rows
            .iter()
            .find(|r| r.start_beat == beat)
            .unwrap_or_else(|| panic!("expected a compiled row at beat {beat}: {:?}", song.rows))
    }

    fn override_for(row: &ProjectSongRow, track: usize) -> Option<&ProjectSongTrackOverride> {
        row.overrides.iter().find(|over| over.track == track)
    }

    /// The crux of the model (spec 6.2/7): a lane no clip covers compiles to
    /// an EXPLICIT-EMPTY override, never to nothing. `preflight_runtime_song`
    /// resolves an absent override from the row's scene cell, so leaving the
    /// lane unmentioned would resurrect the scene backdrop this model
    /// removed — the bug where deleting a clip looked like a no-op.
    #[test]
    fn compile_states_silence_explicitly_for_every_uncovered_lane() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 3.0, 5.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);

        // Track 1 has no clips at all and scene 0's cell for it is P1 — under
        // the retired backdrop rule it would have played that pattern.
        assert_eq!(scenes.song_scene_cell(0, 1), Some(1));
        for r in &compiled.rows {
            assert_eq!(
                override_for(r, 1),
                Some(&empty_over(1)),
                "an uncovered lane must say 'silent' out loud"
            );
        }
        // Track 0 says it too, on either side of its clip.
        assert_eq!(override_for(row_at(&compiled, 0.0), 0), Some(&empty_over(0)));
        assert_eq!(override_for(row_at(&compiled, 3.0), 0), Some(&over(0, 2)));
        assert_eq!(override_for(row_at(&compiled, 5.0), 0), Some(&empty_over(0)));
    }

    /// A scene event is not a playback rule any more; it is the gesture that
    /// STAMPS clips (spec 6.2/8). The stamped clips free-run against the
    /// GLOBAL clock, not the event, so the grid never moves with the boundary.
    #[test]
    fn stamp_scene_clips_writes_the_scene_cells_as_real_clips() {
        let scenes = test_scenes();
        let mut arr = arrangement(vec![ev(0.0, 0), ev(6.0, 1)], vec![Vec::new(); 2], 16.0);
        stamp_scene_clips(&mut arr, &scenes, 0.0, 16.0).expect("stamps");
        arr.validate(&scenes).expect("stamping leaves a valid lane");

        // Scene j's cell is P(j + 1) on both tracks, so both lanes get the
        // same two clips: [0, 6) of P1 and [6, 16) of P2. Beat 6 is 24 steps
        // into the global grid, and 24 mod 16 == 8 — the scene starts
        // mid-cycle rather than restarting the pattern.
        for track in 0..2 {
            assert_eq!(
                arr.track_lanes[track]
                    .iter()
                    .map(|c| (c.start_beat, c.end_beat, c.pattern_id, c.offset_steps))
                    .collect::<Vec<_>>(),
                vec![(0.0, 6.0, Some(1), 0.0), (6.0, 16.0, Some(2), 8.0)],
                "track {track}"
            );
        }

        // Everything audible is a clip, and every stamped clip's phase at any
        // beat is just `steps(beat) mod L`: beat 6 -> 8, beat 9 -> 36 mod 16
        // == 4, and beat 8 (32 steps) -> 0, exactly on the grid.
        let compiled = compile_ok(&arr, &scenes);
        assert_eq!(
            override_for(row_at(&compiled, 6.0), 0),
            Some(&over_at(0, 2, 8.0)),
            "the global grid is the anchor, not the event"
        );
        let mut probe = arr.clone();
        probe.track_lanes[1].clear();
        let id = probe.allocate_clip_id().unwrap();
        probe.track_lanes[1].push(ArrClip::new(id, 9.0, 10.0, Some(3)));
        let compiled = compile_ok(&probe, &scenes);
        assert_eq!(
            override_for(row_at(&compiled, 9.0), 0),
            Some(&over_at(0, 2, 4.0))
        );
    }

    /// The no-op case that hides the bug: a boundary sitting on a whole
    /// number of pattern cycles stamps step 0 either way.
    #[test]
    fn stamping_at_an_aligned_boundary_still_anchors_at_step_zero() {
        let scenes = test_scenes();
        // 8 beats == 32 steps == two whole 16-step cycles.
        let mut arr = arrangement(vec![ev(0.0, 0), ev(8.0, 1)], vec![Vec::new(); 2], 16.0);
        stamp_scene_clips(&mut arr, &scenes, 0.0, 16.0).expect("stamps");
        assert_eq!(
            arr.track_lanes[0]
                .iter()
                .map(|c| (c.start_beat, c.pattern_id, c.offset_steps))
                .collect::<Vec<_>>(),
            vec![(0.0, Some(1), 0.0), (8.0, Some(2), 0.0)]
        );
    }

    /// The defect this rule fixes: stamped clips stay GRID-LOCKED when a
    /// scene boundary moves. Moving a boundary must change how much of a
    /// pattern is heard, never when its steps fall.
    #[test]
    fn stamped_clips_stay_grid_locked_when_a_scene_boundary_moves() {
        let scenes = test_scenes();
        // Patterns are 16 steps at 4 steps/beat, so source step 0 lands on
        // every multiple of 4 beats: 0, 4, 8, 12, 16, 20, ...
        let aligned = {
            let mut arr = arrangement(vec![ev(0.0, 0), ev(16.0, 1)], vec![Vec::new(); 2], 32.0);
            stamp_scene_clips(&mut arr, &scenes, 0.0, 32.0).expect("stamps");
            arr
        };
        // 16 beats == 64 steps == four whole cycles, so the scene-1 clip
        // starts at step 0.
        assert_eq!(aligned.track_lanes[0][1].offset_steps, 0.0);

        // Now drag the boundary to beat 13 — deliberately NOT a multiple of
        // the 4-beat cycle — and re-stamp from there.
        let mut moved = arrangement(vec![ev(0.0, 0), ev(13.0, 1)], vec![Vec::new(); 2], 32.0);
        stamp_scene_clips(&mut moved, &scenes, 0.0, 32.0).expect("stamps");
        // 13 beats == 52 steps; 52 mod 16 == 4.
        assert_eq!(
            moved.track_lanes[0]
                .iter()
                .map(|c| (c.start_beat, c.end_beat, c.pattern_id, c.offset_steps))
                .collect::<Vec<_>>(),
            vec![(0.0, 13.0, Some(1), 0.0), (13.0, 32.0, Some(2), 4.0)]
        );

        // The proof: the beats where the pattern reaches source step 0 are
        // unchanged by the move. 12 is still step 0 (in the scene-0 clip),
        // and 16 and 20 are still step 0 in the scene-1 clip.
        let aligned_song = compile_ok(&aligned, &scenes);
        let moved_song = compile_ok(&moved, &scenes);
        for beat in [12.0, 16.0, 20.0, 24.0] {
            assert_eq!(
                phase_at(&moved_song, &scenes, 0, beat),
                0.0,
                "beat {beat} must still be source step 0 after the move"
            );
            assert_eq!(
                phase_at(&aligned_song, &scenes, 0, beat),
                phase_at(&moved_song, &scenes, 0, beat),
                "moving the boundary changed the rhythm at beat {beat}"
            );
        }
        // And an off-grid beat agrees too: 14 beats == 56 steps, 56 mod 16 == 8.
        assert_eq!(phase_at(&moved_song, &scenes, 0, 14.0), 8.0);
    }

    /// The phase a compiled song puts `track` at on `beat`: the governing
    /// row's override advanced to the beat.
    fn phase_at(
        song: &ProjectSong,
        ctx: &dyn SongCompileContext,
        track: usize,
        beat: f64,
    ) -> f64 {
        let row = song
            .rows
            .iter()
            .rev()
            .find(|row| row.start_beat <= beat)
            .expect("a row governs every beat");
        let over = row
            .overrides
            .iter()
            .find(|over| over.track == track)
            .expect("every lane states its resolution");
        let pattern_id = over.pattern_id.expect("the fixture uses pattern clips");
        advanced_pattern_offset(
            ctx,
            track,
            pattern_id,
            over.offset_steps,
            beat - row.start_beat,
        )
    }

    /// A scene cell that holds nothing stamps nothing: that lane is silent
    /// under the scene, and silence is an honest gap.
    #[test]
    fn stamp_scene_clips_skips_a_scene_cell_that_is_empty() {
        let mut scenes = test_scenes();
        scenes.scenes[0].cells[1] = None;
        let mut arr = arrangement(vec![ev(0.0, 0)], vec![Vec::new(); 2], 16.0);
        stamp_scene_clips(&mut arr, &scenes, 0.0, 16.0).expect("stamps");
        assert_eq!(arr.track_lanes[1], Vec::new());
        assert_eq!(
            arr.track_lanes[0]
                .iter()
                .map(|c| (c.start_beat, c.end_beat, c.pattern_id))
                .collect::<Vec<_>>(),
            vec![(0.0, 16.0, Some(1))]
        );
    }

    /// Stamping TRUNCATES, like every other clip write (spec 14, locked): a
    /// re-stamp replaces whatever is under its span.
    #[test]
    fn stamp_scene_clips_truncates_what_it_lands_on() {
        let scenes = test_scenes();
        let mut arr = arrangement(
            vec![ev(0.0, 0), ev(8.0, 1)],
            vec![vec![clip(0, 4.0, 12.0, 3)], Vec::new()],
            16.0,
        );
        // Re-stamp only the second scene's span.
        stamp_scene_clips(&mut arr, &scenes, 8.0, 16.0).expect("stamps");
        assert_eq!(
            arr.track_lanes[0]
                .iter()
                .map(|c| (c.start_beat, c.end_beat, c.pattern_id))
                .collect::<Vec<_>>(),
            vec![(4.0, 8.0, Some(3)), (8.0, 16.0, Some(2))],
            "the clip is right-trimmed at the stamped span's edge"
        );
        arr.validate(&scenes).expect("still valid");
    }

    #[test]
    fn compile_handles_staggered_clips_on_two_tracks() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 8.0, 2)], vec![clip(1, 5.0, 12.0, 3)]],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        // At beat 5 track 0 has played 20 steps => 20 mod 16 == 4.
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, vec![over(0, 2), empty_over(1)]),
                    row(1, 5.0, 0, vec![over_at(0, 2, 4.0), over(1, 3)]),
                    row(2, 8.0, 0, vec![empty_over(0), over_at(1, 3, 12.0)]),
                    row(3, 12.0, 0, vec![empty_over(0), empty_over(1)]),
                ],
                16.0
            )
        );
    }

    /// A GAP between two clips compiles to an explicit-empty override — the
    /// only way the model spells silence now that empty clips are gone.
    #[test]
    fn compile_emits_explicit_empty_overrides_for_lane_gaps() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 4.0, 2), clip(1, 8.0, 12.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        let gap = override_for(row_at(&compiled, 4.0), 0).expect("the gap is stated");
        assert_eq!(gap.pattern_id, None);
        assert_eq!(gap.take_id, None);
    }

    #[test]
    fn compile_turns_a_take_clip_silent_past_its_end() {
        let (scenes, take) = scenes_with_take();
        // The take is 300 steps == 75 beats at four steps per beat.
        let arr = arrangement(
            vec![ev(0.0, 0), ev(80.0, 1)],
            vec![
                vec![ArrClip::new_take(ClipId(0), 0.0, 100.0, take.0, 0.0)],
                Vec::new(),
            ],
            128.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        assert_eq!(
            compiled,
            song(
                vec![
                    row(
                        0,
                        0.0,
                        0,
                        vec![
                            ProjectSongTrackOverride::new_take(0, take.0, 0.0),
                            empty_over(1)
                        ]
                    ),
                    // The take runs dry at beat 75 -> silent; the clip's own
                    // end at 100 changes nothing, so `normalize` drops it.
                    row(1, 80.0, 1, vec![empty_over(0), empty_over(1)]),
                ],
                128.0
            )
        );
    }

    #[test]
    fn compile_advances_a_take_offset_inside_its_span() {
        let (scenes, take) = scenes_with_take();
        let arr = arrangement(
            vec![ev(0.0, 0), ev(10.0, 1)],
            vec![
                vec![ArrClip::new_take(ClipId(0), 2.0, 40.0, take.0, 8.0)],
                Vec::new(),
            ],
            64.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        // 8 + steps(10 - 2) = 8 + 32 = 40; takes never wrap.
        let scene_change = compiled
            .rows
            .iter()
            .find(|r| r.start_beat == 10.0)
            .expect("scene change compiles to a row");
        assert_eq!(
            scene_change.overrides,
            vec![
                ProjectSongTrackOverride::new_take(0, take.0, 40.0),
                empty_over(1)
            ]
        );
    }

    #[test]
    fn compile_normalizes_boundaries_that_change_nothing_and_renumbers_rows() {
        let scenes = test_scenes();
        // Two back-to-back clips of the same pattern, the second anchored at
        // step 0: the boundary at beat 4 compiles to the same launch state.
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 4.0, 2), clip(2, 4.0, 8.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        assert_eq!(compiled.rows.len(), 2, "{:?}", compiled.rows);
        let ids: Vec<u64> = compiled.rows.iter().map(|r| r.id.0).collect();
        assert_eq!(ids, vec![0, 1], "ids are contiguous 0..len after normalize");
        assert_eq!(compiled.next_row_id, 2);
        assert_eq!(
            compiled.rows,
            vec![
                row(0, 0.0, 0, vec![over(0, 2), empty_over(1)]),
                row(1, 8.0, 0, vec![empty_over(0), empty_over(1)])
            ]
        );
    }

    #[test]
    fn compile_drops_boundaries_at_or_past_the_end_beat() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 8.0, 16.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        // The clip's end edge lands exactly on `end_beat` and is dropped.
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, vec![empty_over(0), empty_over(1)]),
                    row(1, 8.0, 0, vec![over(0, 2), empty_over(1)])
                ],
                16.0
            )
        );
    }

    #[test]
    fn compile_carries_loop_enabled_and_is_deterministic() {
        let scenes = test_scenes();
        let mut arr = valid_arrangement();
        arr.loop_enabled = true;
        let first = compile_ok(&arr, &scenes);
        let second = compile_ok(&arr, &scenes);
        assert!(first.loop_enabled);
        assert_eq!(first, second, "equal input compiles to an identical layout");
    }

    #[test]
    fn compile_propagates_validation_errors() {
        let scenes = test_scenes();
        let mut arr = valid_arrangement();
        arr.scene_lane[0].start_beat = 4.0;
        let err = compile_arrangement(&arr, &scenes).unwrap_err();
        assert!(err.contains("must start at beat 0.0"), "{err}");
    }

    #[test]
    fn compile_output_always_passes_song_validation() {
        let (scenes, take) = scenes_with_take();
        let arr = arrangement(
            vec![ev(0.0, 0), ev(6.0, 1), ev(21.0, 2)],
            vec![
                vec![
                    clip(0, 0.0, 5.0, 2),
                    ArrClip::new_take(ClipId(1), 9.0, 100.0, take.0, 3.0),
                ],
                vec![clip(2, 3.0, 7.0, 1), clip(3, 7.0, 30.0, 3)],
            ],
            128.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        assert!(compiled.rows.len() >= 3);
        // Overrides come out in ascending track order, as ProjectSong
        // validation requires.
        for r in &compiled.rows {
            for pair in r.overrides.windows(2) {
                assert!(pair[0].track < pair[1].track);
            }
        }
    }

    // --- serde ----------------------------------------------------------

    #[test]
    fn arrangement_serde_round_trips_and_skips_defaults() {
        let mut arr = valid_arrangement();
        arr.loop_enabled = true;
        arr.track_lanes[0][0].offset_steps = 7.25;
        let json = serde_json::to_string(&arr).expect("serialize arrangement");
        let restored: ProjectArrangement =
            serde_json::from_str(&json).expect("deserialize arrangement");
        assert_eq!(restored, arr);

        // Zero offsets and absent takes stay off the wire.
        let json = serde_json::to_string(&clip(0, 0.0, 4.0, 1)).expect("serialize clip");
        assert!(!json.contains("offset_steps"), "{json}");
        assert!(!json.contains("take_id"), "{json}");

        // loop_enabled and track_lanes are serde-defaulted.
        let json =
            r#"{"scene_lane":[{"start_beat":0.0,"scene":0}],"end_beat":8.0,"next_clip_id":0}"#;
        let restored: ProjectArrangement =
            serde_json::from_str(json).expect("deserialize minimal arrangement");
        assert!(!restored.loop_enabled);
        assert!(restored.track_lanes.is_empty());
    }

    #[test]
    fn clip_source_mirrors_the_override_encoding() {
        assert_eq!(
            clip(0, 0.0, 4.0, 3).source(),
            LaneSource::Pattern(PatternId(3))
        );
        assert_eq!(
            ArrClip::new_take(ClipId(0), 0.0, 4.0, 5, 0.0).source(),
            LaneSource::Take(TakeId(5))
        );
        assert!(sourceless_clip(0, 0.0, 4.0).source().is_empty());
    }

    // --- serialization (spec 10) ----------------------------------------

    #[test]
    fn arrangement_for_serialization_maps_pool_ids_to_scene_cell_positions() {
        let scenes = test_scenes();
        let arr = valid_arrangement();
        // In the rebuilt-shape pools the ids already equal scene index + 1,
        // so serialization is the identity here.
        let serialized = arrangement_for_serialization(&arr, &scenes).expect("serializable");
        assert_eq!(serialized, arr);
    }

    #[test]
    fn arrangement_for_serialization_rejects_pattern_not_in_any_scene_cell() {
        let mut scenes = test_scenes();
        // Fork a pattern into track 1's pool without assigning it to a cell.
        let source = scenes.track_pools[1].get(PatternId(1)).unwrap();
        let orphan = scenes.track_pools[1].insert(source);
        let mut arr = valid_arrangement();
        arr.track_lanes[1] = vec![clip(1, 16.0, 24.0, orphan.0)];
        let err = arrangement_for_serialization(&arr, &scenes).unwrap_err();
        assert!(err.contains("Track 2 clip 1"), "{err}");
        assert!(err.contains("beats 16-24"), "{err}");
        assert!(err.contains("not assigned"), "{err}");
    }

    #[test]
    fn arrangement_for_serialization_passes_sourceless_and_take_clips_through() {
        let (scenes, take) = scenes_with_take();
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![
            sourceless_clip(0, 4.0, 8.0),
            ArrClip::new_take(ClipId(4), 8.0, 12.0, take.0, 2.0),
        ];
        arr.next_clip_id = 5;
        let serialized = arrangement_for_serialization(&arr, &scenes).expect("serializable");
        assert_eq!(serialized.track_lanes[0], arr.track_lanes[0]);
    }

    /// The save/load round trip end to end (spec 10): serialize into the
    /// rebuilt-pool id domain, go through the wire, and compile against the
    /// scenes the loader rebuilt — the compiled song must be the one the live
    /// arrangement produced.
    #[test]
    fn arrangement_round_trips_through_save_and_load() {
        let scenes = test_scenes();
        let mut arr = valid_arrangement();
        arr.loop_enabled = true;
        arr.track_lanes[0][0].offset_steps = 5.0;
        let live_song = compile_arrangement(&arr, &scenes).expect("live compile");

        let serialized = arrangement_for_serialization(&arr, &scenes).expect("serializable");
        let json = serde_json::to_string(&serialized).expect("serialize");
        let loaded: ProjectArrangement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(loaded, serialized);

        // The loader rebuilds pools from scene cells, which is exactly what
        // `test_scenes` models.
        let rebuilt = test_scenes();
        let loaded_song = compile_arrangement(&loaded, &rebuilt).expect("load compile");
        assert_eq!(loaded_song, live_song);
    }

    /// The load path may not compile against `SerializedSongContext`: it
    /// answers "unknown" for every timebase, so a clip crossing a boundary
    /// another lane created keeps its start-of-clip phase instead of the
    /// phase it actually reached — the music retriggers mid-cycle.
    #[test]
    fn compiling_against_the_serialized_context_loses_clip_phase() {
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 8.0, 2)], vec![clip(1, 5.0, 12.0, 3)]],
            16.0,
        );
        let live = compile_arrangement(&arr, &test_scenes()).expect("live compile");
        let serialized_ctx = SerializedSongContext {
            scene_count: 3,
            track_count: 2,
            takes: Vec::new(),
        };
        let thin = compile_arrangement(&arr, &serialized_ctx).expect("thin compile");
        assert!(
            live.rows.iter().any(|row| row
                .overrides
                .iter()
                .any(|over| over.offset_steps != 0.0)),
            "the live compile advances the clip's phase across the boundary"
        );
        assert!(
            thin.rows
                .iter()
                .all(|row| row.overrides.iter().all(|over| over.offset_steps == 0.0)),
            "the serialized context knows no timebases, so it stamps nothing"
        );
        assert_ne!(live, thin);
    }

    #[test]
    fn new_builds_an_empty_arrangement_that_validates() {
        let arr = ProjectArrangement::new(2, 16.0);
        arr.validate(&test_scenes())
            .expect("empty arrangement is valid");
        let compiled = compile_ok(&arr, &test_scenes());
        assert_eq!(
            compiled.rows,
            vec![row(0, 0.0, 0, vec![empty_over(0), empty_over(1)])]
        );
    }
    // --- truncation (spec 14, locked) -----------------------------------

    /// `occlude_span`'s four cases, on one lane, against the fixture's
    /// four-beat (16-step) patterns.
    #[test]
    fn occlude_span_trims_removes_and_splits() {
        let scenes = test_scenes();

        // 1. Disjoint clips are untouched.
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 4.0, 1), clip(1, 12.0, 16.0, 1)], Vec::new()],
            32.0,
        );
        let untouched = arr.track_lanes[0].clone();
        occlude_span(&mut arr, &scenes, 0, 6.0, 10.0).expect("occludes");
        assert_eq!(arr.track_lanes[0], untouched);

        // 2. A fully covered clip is removed (edges count as covered).
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 4.0, 8.0, 1)], Vec::new()],
            32.0,
        );
        occlude_span(&mut arr, &scenes, 0, 4.0, 8.0).expect("occludes");
        assert!(arr.track_lanes[0].is_empty());

        // 3a. Right trim: only the span shortens, the anchor is untouched.
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 8.0, 1)], Vec::new()],
            32.0,
        );
        occlude_span(&mut arr, &scenes, 0, 5.0, 12.0).expect("occludes");
        assert_eq!(
            arr.track_lanes[0]
                .iter()
                .map(|c| (c.id, c.start_beat, c.end_beat, c.offset_steps))
                .collect::<Vec<_>>(),
            vec![(ClipId(0), 0.0, 5.0, 0.0)]
        );

        // 3b. Left trim: the survivor re-stamps its offset by the split rule
        // — 5 beats at four steps per beat is 20 steps, 20 mod 16 == 4.
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 12.0, 1)], Vec::new()],
            32.0,
        );
        occlude_span(&mut arr, &scenes, 0, 0.0, 5.0).expect("occludes");
        assert_eq!(
            arr.track_lanes[0]
                .iter()
                .map(|c| (c.id, c.start_beat, c.end_beat, c.offset_steps))
                .collect::<Vec<_>>(),
            vec![(ClipId(0), 5.0, 12.0, 4.0)]
        );

        // 4. A span landing strictly inside splits the clip around it; the
        // right fragment gets a FRESH id and its own re-stamped offset
        // (10 beats == 40 steps, 40 mod 16 == 8).
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 0.0, 16.0, 1)], Vec::new()],
            32.0,
        );
        occlude_span(&mut arr, &scenes, 0, 6.0, 10.0).expect("occludes");
        assert_eq!(
            arr.track_lanes[0]
                .iter()
                .map(|c| (c.id, c.start_beat, c.end_beat, c.offset_steps))
                .collect::<Vec<_>>(),
            vec![(ClipId(0), 0.0, 6.0, 0.0), (ClipId(1), 10.0, 16.0, 8.0)]
        );
        assert_eq!(arr.next_clip_id, 2, "the split tail consumes an id");
        arr.validate(&scenes)
            .expect("truncation leaves a valid, non-overlapping lane");
    }

    /// A take clip left-trimmed past its own end is dropped entirely: that
    /// span is the silent tail, and silence is the absence of a clip.
    #[test]
    fn occlude_span_drops_a_take_trimmed_past_its_end() {
        let (scenes, take) = scenes_with_take();
        // 300 steps at four per beat == 75 beats of content.
        let mut arr = arrangement(
            vec![ev(0.0, 0)],
            vec![
                vec![ArrClip::new_take(ClipId(0), 0.0, 100.0, take.0, 0.0)],
                Vec::new(),
            ],
            128.0,
        );
        occlude_span(&mut arr, &scenes, 0, 0.0, 80.0).expect("occludes");
        assert!(
            arr.track_lanes[0].is_empty(),
            "nothing is left to play, so the clip is DROPPED — silence is a \
             gap, not an empty clip: {:?}",
            arr.track_lanes[0]
        );
        arr.validate(&scenes).expect("still valid");
    }

    /// `restamped_clip` runs backwards too, which is what a left-edge GROW
    /// needs: the clip has to start earlier playing what it would have been
    /// playing then.
    #[test]
    fn restamped_clip_runs_backwards_for_a_left_edge_grow() {
        let scenes = test_scenes();
        let source = clip(0, 8.0, 16.0, 1);
        let grown = restamped_clip(&scenes, 0, &source, 5.0).expect("still plays");
        assert_eq!(grown.start_beat, 5.0);
        // -3 beats == -12 steps; rem_euclid over 16 steps == 4.
        assert_eq!(grown.offset_steps, 4.0);
        assert_eq!(
            restamped_clip(&scenes, 0, &grown, 8.0)
                .expect("still plays")
                .offset_steps,
            source.offset_steps,
            "and the round trip lands exactly back on the original anchor"
        );
    }

    // --- derived UI read surfaces (spec 12) ------------------------------

    /// One span per scene EVENT, ending at the next (or at `end_beat`) — the
    /// surface that makes the jagged scene lane structurally impossible: a
    /// clip edge contributes no span at all.
    #[test]
    fn scene_spans_emit_one_span_per_event_regardless_of_clips() {
        let arr = arrangement(
            vec![ev(0.0, 0), ev(16.0, 1), ev(32.0, 2)],
            // Four clips with edges at 4/8/20/40 — none of which may split
            // the scene lane.
            vec![
                vec![clip(0, 4.0, 8.0, 2), clip(1, 20.0, 40.0, 3)],
                Vec::new(),
            ],
            48.0,
        );
        assert_eq!(
            arrangement_scene_spans(&arr),
            vec![
                SceneSpan { start_beat: 0.0, end_beat: 16.0, scene: 0 },
                SceneSpan { start_beat: 16.0, end_beat: 32.0, scene: 1 },
                SceneSpan { start_beat: 32.0, end_beat: 48.0, scene: 2 },
            ]
        );

        // A single scene over the whole arrangement is exactly ONE span —
        // the phase-5 acceptance case.
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 16.0, 32.0, 2)], Vec::new()],
            48.0,
        );
        assert_eq!(
            arrangement_scene_spans(&arr),
            vec![SceneSpan { start_beat: 0.0, end_beat: 48.0, scene: 0 }]
        );
    }

    /// A scene event at or past `end_beat` contributes nothing, and the last
    /// span is clamped to the end.
    #[test]
    fn scene_spans_drop_events_at_or_past_the_end() {
        let arr = arrangement(vec![ev(0.0, 0), ev(48.0, 1)], vec![Vec::new(); 2], 48.0);
        assert_eq!(
            arrangement_scene_spans(&arr),
            vec![SceneSpan { start_beat: 0.0, end_beat: 48.0, scene: 0 }]
        );
    }

    // --- v5 -> v6 migration (spec 10) ------------------------------------

    /// The legacy backdrop derivation, which is what migration freezes into
    /// clips: every lane GAP filled with the governing scene's cell, split at
    /// scene boundaries, phase-anchored on the scene event.
    #[test]
    fn legacy_backdrop_spans_fill_lane_gaps_split_by_scene() {
        let scenes = test_scenes();
        // Track 0: clip over [8,16); track 1: no clips at all.
        let arr = arrangement(
            vec![ev(0.0, 0), ev(24.0, 1)],
            vec![vec![clip(0, 8.0, 16.0, 2)], Vec::new()],
            32.0,
        );
        let spans = legacy_backdrop_spans(&arr, &scenes);

        // Lane 0's gaps are [0,8) and [16,32); the second is split by the
        // scene change at 24. Scene j's cell is PatternId(j + 1).
        assert_eq!(
            spans[0],
            vec![
                LegacyBackdropSpan {
                    start_beat: 0.0,
                    end_beat: 8.0,
                    scene: 0,
                    pattern_id: 1,
                    offset_steps: 0.0,
                },
                LegacyBackdropSpan {
                    start_beat: 16.0,
                    end_beat: 24.0,
                    scene: 0,
                    // 16 beats past the scene event at 4 steps/beat is 64
                    // steps, which wraps to 0 in a 16-step pattern.
                    pattern_id: 1,
                    offset_steps: 0.0,
                },
                LegacyBackdropSpan {
                    start_beat: 24.0,
                    end_beat: 32.0,
                    scene: 1,
                    pattern_id: 2,
                    offset_steps: 0.0,
                },
            ]
        );
        // A clip-free lane is backdrop end to end, one span per scene.
        assert_eq!(
            spans[1]
                .iter()
                .map(|span| (span.start_beat, span.end_beat, span.scene))
                .collect::<Vec<_>>(),
            vec![(0.0, 24.0, 0), (24.0, 32.0, 1)]
        );
    }

    /// A gap that opens mid-pattern-cycle carries the advanced phase, and a
    /// lane fully covered by clips has no ghosts at all.
    #[test]
    fn legacy_backdrop_spans_carry_phase_and_vanish_under_full_coverage() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            // Track 0's clip ends at beat 2, a quarter of the way into the
            // 4-beat pattern: the ghost after it starts at step 8.
            vec![vec![clip(0, 0.0, 2.0, 2)], vec![clip(1, 0.0, 16.0, 3)]],
            16.0,
        );
        let spans = legacy_backdrop_spans(&arr, &scenes);
        assert_eq!(spans[0].len(), 1);
        assert_eq!(spans[0][0].start_beat, 2.0);
        assert_eq!(spans[0][0].offset_steps, 8.0);
        assert!(
            spans[1].is_empty(),
            "a lane with no gaps has no backdrop showing through"
        );
    }
    /// The migration contract (spec 10): a v5 arrangement — where a lane gap
    /// played the governing scene's cell — must load into a v6 arrangement
    /// that sounds IDENTICAL, phase offsets included.
    ///
    /// "Sounds identical" is checked against a reference implementation of the
    /// old rule, probed densely across the whole song: for every track at
    /// every probe beat, what the v5 model resolved must equal what the
    /// migrated arrangement's compiled song resolves.
    #[test]
    fn migration_makes_a_v5_arrangement_sound_identical() {
        let scenes = test_scenes();
        // Gaps everywhere: before a clip, between clips, after the last one,
        // across a scene change placed off the pattern grid (beat 10 is 2.5
        // pattern cycles in), plus an explicit-empty clip that must stay
        // silent rather than become backdrop.
        let v5 = arrangement(
            vec![ev(0.0, 0), ev(10.0, 1)],
            vec![
                vec![clip(0, 3.0, 5.0, 3), clip(1, 13.0, 17.0, 3)],
                vec![sourceless_clip(2, 6.0, 9.0), clip(3, 20.0, 24.0, 2)],
            ],
            32.0,
        );

        let migrated = migrate_legacy_backdrops(&v5, &scenes).expect("migrates");
        migrated
            .validate(&scenes)
            .expect("the migrated arrangement is valid under the new rules");
        assert!(
            migrated
                .track_lanes
                .iter()
                .flatten()
                .all(|clip| !clip.source().is_empty()),
            "migration leaves no sourceless clips: {:?}",
            migrated.track_lanes
        );
        let compiled = compile_ok(&migrated, &scenes);

        for step in 0..(32 * 4) {
            let beat = step as f64 / 4.0;
            for track in 0..2 {
                assert_eq!(
                    resolve_compiled(&compiled, &scenes, track, beat),
                    resolve_v5(&v5, &scenes, track, beat),
                    "track {track} at beat {beat}"
                );
            }
        }

        // And the explicit-empty clip's span is genuinely silent on both
        // sides of the migration (it was, and still is, a deliberate hole).
        assert_eq!(resolve_v5(&v5, &scenes, 1, 7.0), None);
        assert_eq!(resolve_compiled(&compiled, &scenes, 1, 7.0), None);
    }

    /// The retired v5 resolution rule, implemented independently of the
    /// compiler: clip, else the governing scene event's cell anchored on the
    /// event, else nothing. Pattern sources only (the fixture uses no takes).
    fn resolve_v5(
        arr: &ProjectArrangement,
        ctx: &dyn SongCompileContext,
        track: usize,
        beat: f64,
    ) -> Option<(u64, f64)> {
        if let Some(clip) = arr.clip_at(track, beat) {
            let pattern_id = clip.pattern_id?;
            return Some((
                pattern_id,
                advanced_pattern_offset(
                    ctx,
                    track,
                    pattern_id,
                    clip.offset_steps,
                    beat - clip.start_beat,
                ),
            ));
        }
        let event = arr.scene_event_at_beat(beat)?;
        let pattern_id = ctx.song_scene_cell(event.scene, track)?;
        Some((
            pattern_id,
            advanced_pattern_offset(ctx, track, pattern_id, 0.0, beat - event.start_beat),
        ))
    }

    /// What the PLAYBACK model resolves at `beat`: the governing row's
    /// override if it has one, else the row's scene cell (the fallback in
    /// `preflight_runtime_song`), advanced from the row's start.
    fn resolve_compiled(
        song: &ProjectSong,
        ctx: &dyn SongCompileContext,
        track: usize,
        beat: f64,
    ) -> Option<(u64, f64)> {
        let row = song
            .rows
            .iter()
            .rev()
            .find(|row| row.start_beat <= beat)
            .expect("a row governs every beat");
        let (pattern_id, offset) = match row.overrides.iter().find(|over| over.track == track) {
            Some(over) => (over.pattern_id?, over.offset_steps),
            None => (ctx.song_scene_cell(row.scene, track)?, 0.0),
        };
        Some((
            pattern_id,
            advanced_pattern_offset(ctx, track, pattern_id, offset, beat - row.start_beat),
        ))
    }
}
