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

/// A scene *change* on the scene lane: from `start_beat` onward, every track
/// without a clip covering the beat plays this scene's cell. Spans are
/// derived (event to next event, last event to `end_beat`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneEvent {
    pub start_beat: f64,
    pub scene: usize,
}

/// One clip on a track lane: a half-open span `[start_beat, end_beat)` with a
/// source and a phase anchor. The source encoding matches
/// `ProjectSongTrackOverride`: a take excludes a pattern, and both `None` is
/// an explicit-empty clip (silence that still occludes the scene backdrop).
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
    /// (validation forbids carrying both).
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

    /// Spec 6.2 step 2: the scene governing `beat` — the last scene event at
    /// or before it. `None` only for a beat before the first event (which
    /// validation forbids) or an empty lane.
    pub fn scene_at_beat(&self, beat: f64) -> Option<usize> {
        self.scene_event_at_beat(beat).map(|event| event.scene)
    }

    /// The governing scene *event* at `beat` — the last event at or before
    /// it. Compile needs the event itself, not just its scene index: the
    /// event's `start_beat` is the phase anchor for every lane riding the
    /// scene backdrop.
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
/// stamps offsets with. Mirrors `SongApp::pattern_step_mapping` /
/// `take_step_mapping` and the scene-cell lookup inside `split_row_state`.
///
/// Every method defaults to `None` ("unknown") so contexts that cannot see
/// project internals — `SerializedSongContext` — stay valid; an unknown
/// mapping leaves offsets untouched, the same fallback `split_row_state`
/// takes, and an unknown scene cell simply materializes no backdrop override.
pub trait SongCompileContext {
    /// The pattern in scene `scene`'s cell for `track`, as a raw pool id.
    fn song_scene_cell(&self, _scene: usize, _track: usize) -> Option<u64> {
        None
    }

    /// `(steps_per_beat, num_steps)` for `pattern_id` in `track`'s pool under
    /// the pattern's base timebase (takes spec 7.2/7.4). Per-step timebase
    /// plocks deliberately do not participate in stamping.
    fn song_track_pattern_step_mapping(
        &self,
        _track: usize,
        _pattern_id: u64,
    ) -> Option<(f64, f64)> {
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

    fn song_track_pattern_step_mapping(&self, track: usize, pattern_id: u64) -> Option<(f64, f64)> {
        let data = self.track_pools.get(track)?.get(PatternId(pattern_id))?;
        let num_steps = data.track_params.num_steps.max(1);
        let step_beats = data.track_params.timebase.step_beats(num_steps);
        (step_beats > 0.0).then(|| (1.0 / step_beats, num_steps as f64))
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

/// Advance a pattern lane's `offset_steps` by `delta_beats` of playback,
/// normalized into `[0, num_steps)`. Byte-for-byte the rule in
/// `SongApp::advanced_offset`, including the boundary-epsilon collapse to 0 so
/// a clip landing on a pattern boundary stamps an implicit zero offset.
fn advanced_pattern_offset(
    ctx: &dyn SongCompileContext,
    track: usize,
    pattern_id: u64,
    offset_steps: f64,
    delta_beats: f64,
) -> f64 {
    let Some((steps_per_beat, num_steps)) = ctx.song_track_pattern_step_mapping(track, pattern_id)
    else {
        return offset_steps;
    };
    let advanced = (offset_steps + delta_beats * steps_per_beat).rem_euclid(num_steps);
    if advanced < 1e-9 || advanced > num_steps - 1e-9 {
        0.0
    } else {
        advanced
    }
}

/// The launch override a clip contributes at boundary `beat`, with
/// `offset_steps` stamped by the takes spec 7 split rule — the same arithmetic
/// `split_row_state` applies to a row split, measured from the clip's start
/// instead of the governing row's.
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

/// The override a lane riding the *scene backdrop* contributes at boundary
/// `beat`, or `None` when it needs none.
///
/// This is `split_row_state`'s `None` arm, and it is not optional: the row
/// model gives scene-resolved lanes no phase memory (`RuntimeSongRow::
/// lane_offsets` is `0.0` for them and `song_playback` advances only from the
/// row's own `start_beat`), so without a materialized offset a backdrop lane
/// would restart its pattern at step 0 on every boundary row — i.e. any clip
/// edge on any *other* track would retrigger it mid-cycle.
///
/// The anchor is the governing scene *event*'s `start_beat`: the scene
/// launches at its event and runs continuously until the next one. As in
/// `split_row_state`, an override is materialized only when the advanced
/// offset is nonzero, so lanes stay implicit whenever they can and `normalize`
/// still collapses no-op boundaries.
fn backdrop_override(
    ctx: &dyn SongCompileContext,
    track: usize,
    scene: usize,
    scene_start_beat: f64,
    beat: f64,
) -> Option<ProjectSongTrackOverride> {
    let pattern_id = ctx.song_scene_cell(scene, track)?;
    let offset = advanced_pattern_offset(ctx, track, pattern_id, 0.0, beat - scene_start_beat);
    (offset != 0.0).then_some(ProjectSongTrackOverride {
        track,
        pattern_id: Some(pattern_id),
        take_id: None,
        offset_steps: offset,
    })
}

/// Spec 7: compile lanes into the playback row model.
///
/// The boundary set is every scene-event start plus every clip start and end
/// below `end_beat` (compared exactly — gestures already quantize). Each
/// boundary becomes one row carrying the governing scene, one override per
/// lane whose clip contains it (phase-stamped by the split rule), and one
/// materialized backdrop override per lane that rides the scene mid-cycle
/// (see `backdrop_override`). Adjacent identical rows collapse (`normalize`),
/// then ids are assigned by index so equal input always compiles to an
/// identical row layout.
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
                // No clip covers the beat: the lane rides the scene backdrop
                // and may need its phase materialized.
                None => {
                    overrides.extend(backdrop_override(ctx, track, scene, event.start_beat, beat))
                }
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
/// - A per-track override opens a clip that runs until the first later row
///   that changes that lane; a row that drops the override closes the clip at
///   its beat, and an explicit-empty override becomes an explicit-empty clip.
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
/// override reappears verbatim on its row. Compile may add *more* overrides
/// than the rows had — a lane riding the scene backdrop mid-cycle gets its
/// phase materialized (`backdrop_override`), which the row model could not
/// express — so `compile_arrangement(lowered)` is the input song plus, per
/// row, zero or more overrides on tracks that row did not mention.
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

    // Track lanes: one clip per contiguous run of a lane's launch state.
    for track in 0..track_count {
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
                        push_lowered_clip(&mut arrangement, track, clip)?;
                    }
                    open = Some(ArrClip {
                        id: ClipId(0), // assigned when the clip is pushed
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
                        push_lowered_clip(&mut arrangement, track, clip)?;
                    }
                }
            }
        }
        if let Some(clip) = open.take() {
            push_lowered_clip(&mut arrangement, track, clip)?;
        }
    }

    Ok(arrangement)
}

/// Append `clip` to `track`'s lane with a freshly allocated id. Zero-length
/// clips cannot arise from sorted rows (starts are strictly increasing), but
/// one would be meaningless anyway, so it is dropped rather than stored.
fn push_lowered_clip(
    arrangement: &mut ProjectArrangement,
    track: usize,
    mut clip: ArrClip,
) -> Result<(), String> {
    if clip.end_beat <= clip.start_beat {
        return Ok(());
    }
    clip.id = arrangement.allocate_clip_id()?;
    arrangement.track_lanes[track].push(clip);
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

    // --- fixtures (song.rs test vocabulary) -----------------------------

    fn clip(id: u64, start: f64, end: f64, pattern_id: u64) -> ArrClip {
        ArrClip::new(ClipId(id), start, end, Some(pattern_id))
    }

    fn empty_clip(id: u64, start: f64, end: f64) -> ArrClip {
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
        let chunk_data = scenes.track_pools[0].get(PatternId(1)).unwrap().clone();
        let chunk_a = scenes.track_pools[0].insert(chunk_data.clone());
        let chunk_b = scenes.track_pools[0].insert(chunk_data);
        let take = scenes.take_pools[0].insert(None, vec![chunk_a, chunk_b], 300);
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
        assert_eq!(
            scenes.song_track_pattern_step_mapping(0, 1),
            Some((4.0, 16.0)),
            "fixtures assume 16 sixteenth-note steps"
        );
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
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, Vec::new()),
                    row(1, 16.0, 1, Vec::new()),
                    row(2, 48.0, 2, Vec::new()),
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
                    row(0, 0.0, 0, Vec::new()),
                    row(1, 4.0, 0, vec![over(0, 2)]),
                    row(2, 8.0, 0, Vec::new()),
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
        // The last row is where the clip ends and track 0 rejoins the scene 1
        // backdrop, which has been running since beat 7: both lanes are
        // 16 - 7 = 9 beats == 36 steps into scene 1's pattern (P2), 36 mod
        // 16 == 4.
        assert_eq!(
            compiled,
            song(
                vec![
                    row(0, 0.0, 0, Vec::new()),
                    row(1, 4.0, 0, vec![over(0, 2)]),
                    row(2, 7.0, 1, vec![over_at(0, 2, 12.0)]),
                    row(3, 16.0, 1, vec![over_at(0, 2, 4.0), over_at(1, 2, 4.0)]),
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
        assert_eq!(compiled.rows[1].overrides, vec![over_at(0, 2, 6.0)]);
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

    #[test]
    fn compile_keeps_the_scene_backdrop_phase_continuous_across_a_foreign_clip_edge() {
        // Track 1 has no clips at all: it rides scene 0's 16-step (4-beat)
        // pattern for the whole song. Track 0's clip edges still force
        // boundary rows, and the row model gives scene-resolved lanes no
        // phase memory — so track 1's phase must be materialized or it would
        // retrigger from step 0 at beats 3 and 5.
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 3.0, 5.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);

        // Beat 0 is the scene's own anchor: nothing to materialize.
        assert_eq!(override_for(row_at(&compiled, 0.0), 1), None);
        // 3 beats * 4 steps/beat = 12 steps into scene 0's cell (P1).
        assert_eq!(
            override_for(row_at(&compiled, 3.0), 1),
            Some(&over_at(1, 1, 12.0))
        );
        // 5 beats * 4 = 20 steps, mod 16 == 4.
        assert_eq!(
            override_for(row_at(&compiled, 5.0), 1),
            Some(&over_at(1, 1, 4.0))
        );
        // Track 0 rejoins the backdrop at beat 5 with the same phase.
        assert_eq!(
            override_for(row_at(&compiled, 5.0), 0),
            Some(&over_at(0, 1, 4.0))
        );
    }

    #[test]
    fn compile_omits_the_backdrop_override_when_the_boundary_lands_on_a_pattern_boundary() {
        // Same shape, but every boundary is a whole number of 4-beat pattern
        // loops from the scene event, so the backdrop needs no materialized
        // phase and the rows stay lean.
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
                    row(0, 0.0, 0, Vec::new()),
                    row(1, 4.0, 0, vec![over(0, 2)]),
                    row(2, 8.0, 0, Vec::new()),
                ],
                16.0
            )
        );
        for r in &compiled.rows {
            assert_eq!(override_for(r, 1), None, "no backdrop override anywhere");
        }
    }

    #[test]
    fn compile_anchors_backdrop_phase_to_the_scene_event_not_the_row() {
        // Scene 1 launches at beat 6 — not a multiple of the 4-beat pattern
        // length — and track 0 has two clips inside its span, so the row
        // preceding the beat-9 boundary is at beat 8, not at the scene event.
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0), ev(6.0, 1)],
            vec![
                vec![clip(0, 7.0, 8.0, 3), clip(1, 9.0, 10.0, 3)],
                Vec::new(),
            ],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        // 9 - 6 = 3 beats into scene 1's cell (P2) => 12 steps.
        // Measured from beat 0 it would be 36 mod 16 == 4; measured from the
        // previous row (beat 8) it would be 4 as well — 12 pins the anchor.
        assert_eq!(
            override_for(row_at(&compiled, 9.0), 1),
            Some(&over_at(1, 2, 12.0))
        );
        // The scene event's own row needs no materialized phase.
        assert_eq!(override_for(row_at(&compiled, 6.0), 1), None);
        // And at beat 7, one beat in: 4 steps.
        assert_eq!(
            override_for(row_at(&compiled, 7.0), 1),
            Some(&over_at(1, 2, 4.0))
        );
    }

    #[test]
    fn compile_emits_no_backdrop_override_for_a_scene_cell_that_is_empty() {
        let mut scenes = test_scenes();
        scenes.scenes[0].cells[1] = None;
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 3.0, 5.0, 2)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        for r in &compiled.rows {
            assert_eq!(
                override_for(r, 1),
                None,
                "an empty scene cell has no phase to materialize"
            );
        }
        // Track 0's backdrop still materializes where its cell is populated.
        assert_eq!(
            override_for(row_at(&compiled, 5.0), 0),
            Some(&over_at(0, 1, 4.0))
        );
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
                    row(0, 0.0, 0, vec![over(0, 2)]),
                    row(1, 5.0, 0, vec![over_at(0, 2, 4.0), over(1, 3)]),
                    row(2, 8.0, 0, vec![over_at(1, 3, 12.0)]),
                    row(3, 12.0, 0, Vec::new()),
                ],
                16.0
            )
        );
    }

    #[test]
    fn compile_emits_explicit_empty_overrides_for_empty_clips() {
        let scenes = test_scenes();
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![empty_clip(0, 4.0, 8.0)], Vec::new()],
            16.0,
        );
        let compiled = compile_ok(&arr, &scenes);
        assert_eq!(compiled.rows[1].overrides, vec![empty_over(0)]);
        assert_eq!(compiled.rows[1].overrides[0].pattern_id, None);
        assert_eq!(compiled.rows[1].overrides[0].take_id, None);
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
                        vec![ProjectSongTrackOverride::new_take(0, take.0, 0.0)]
                    ),
                    row(1, 80.0, 1, vec![empty_over(0)]),
                    row(2, 100.0, 1, Vec::new()),
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
            vec![ProjectSongTrackOverride::new_take(0, take.0, 40.0)]
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
            vec![row(0, 0.0, 0, vec![over(0, 2)]), row(1, 8.0, 0, Vec::new())]
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
                vec![row(0, 0.0, 0, Vec::new()), row(1, 8.0, 0, vec![over(0, 2)])],
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
                vec![empty_clip(2, 3.0, 7.0), clip(3, 7.0, 30.0, 3)],
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
        assert!(empty_clip(0, 0.0, 4.0).source().is_empty());
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
        let source = scenes.track_pools[1].get(PatternId(1)).unwrap().clone();
        let orphan = scenes.track_pools[1].insert(source);
        let mut arr = valid_arrangement();
        arr.track_lanes[1] = vec![clip(1, 16.0, 24.0, orphan.0)];
        let err = arrangement_for_serialization(&arr, &scenes).unwrap_err();
        assert!(err.contains("Track 2 clip 1"), "{err}");
        assert!(err.contains("beats 16-24"), "{err}");
        assert!(err.contains("not assigned"), "{err}");
    }

    #[test]
    fn arrangement_for_serialization_passes_empty_and_take_clips_through() {
        let (scenes, take) = scenes_with_take();
        let mut arr = valid_arrangement();
        arr.track_lanes[0] = vec![
            empty_clip(0, 4.0, 8.0),
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
    /// answers "unknown" for every scene cell and timebase, so compiling
    /// against it silently drops every scene-backdrop phase override.
    #[test]
    fn compiling_against_the_serialized_context_loses_backdrop_phase() {
        let arr = arrangement(
            vec![ev(0.0, 0)],
            vec![vec![clip(0, 3.0, 5.0, 2)], Vec::new()],
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
            "the live compile materializes backdrop phase"
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
        assert_eq!(compiled.rows, vec![row(0, 0.0, 0, Vec::new())]);
    }
}
