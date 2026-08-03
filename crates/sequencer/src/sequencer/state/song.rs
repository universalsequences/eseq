//! Song-mode data model (docs/song-mode-spec.md section 5).
//!
//! A committed song is a linear sequence of complete session launch states
//! (`ProjectSongRow`: base scene plus the full per-track override set) at
//! absolute musical beat positions, plus an explicit `end_beat`. Rows carry
//! stable `SongRowId` identity mirroring the `SceneId`/`next_scene_id`
//! precedent in `scenes.rs`. This module owns validation (spec 5.3), the
//! adjacent-identical-row normalization rule, topology helpers (spec 5.4),
//! and the two derived queries of spec 5.5: `state_at_beat` and the
//! track-lane projection `project_lanes`.

use super::*;

/// Stable logical identity for a song row.
///
/// Row order and playback semantics come from `start_beat`; the id exists so
/// selection, undo mementos, and observability can refer to a row across
/// edits that reorder it. Ids are allocated monotonically from
/// `ProjectSong::next_row_id` and never reused within a project.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SongRowId(pub u64);

/// One per-track pattern override inside a song row. `pattern_id` is a
/// track-pattern-pool id (`PatternId`) stored as its raw `u64` because
/// `PatternId` itself is not serialized. `None` is an explicit-empty
/// override: the track plays nothing for the row even when the base scene's
/// cell holds a pattern (the arrangement's sparsity primitive). Serde reads
/// a legacy bare number as `Some`, so old project files load unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSongTrackOverride {
    pub track: usize,
    pub pattern_id: Option<u64>,
    /// If `Some`, this override plays a take (takes spec 6.2) and
    /// `pattern_id` must be `None` (validation 6.3). Serde-defaulted so
    /// existing files load unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_id: Option<u64>,
    /// Start offset into the source in fractional pattern steps of this
    /// track's timebase (takes spec 6.2): the anchored phase formula plays
    /// source step `steps(beat - start_beat) + offset_steps` (mod pattern
    /// length for pattern sources; takes are silent past their end). `0.0`
    /// — the serde default, so existing files load unchanged — means the
    /// clip begins at source step 0 at its row start.
    #[serde(default, skip_serializing_if = "offset_steps_is_zero")]
    pub offset_steps: f64,
}

impl ProjectSongTrackOverride {
    /// Override with the default offset (clip starts at source step 0).
    pub fn new(track: usize, pattern_id: Option<u64>) -> Self {
        Self {
            track,
            pattern_id,
            take_id: None,
            offset_steps: 0.0,
        }
    }

    /// Take-playing override (takes spec 6.2).
    pub fn new_take(track: usize, take_id: u64, offset_steps: f64) -> Self {
        Self {
            track,
            pattern_id: None,
            take_id: Some(take_id),
            offset_steps,
        }
    }

    /// The override's resolved source (takes spec 6.2): a take wins over
    /// `pattern_id` (validation forbids carrying both).
    pub fn source(&self) -> LaneSource {
        match (self.take_id, self.pattern_id) {
            (Some(take), _) => LaneSource::Take(TakeId(take)),
            (None, Some(pattern)) => LaneSource::Pattern(PatternId(pattern)),
            (None, None) => LaneSource::Empty,
        }
    }
}

/// Resolved lane content source (takes spec 6.2), so downstream code never
/// juggles the two id options on the override.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LaneSource {
    Pattern(PatternId),
    Take(TakeId),
    Empty,
}

impl LaneSource {
    pub fn pattern(&self) -> Option<PatternId> {
        match self {
            LaneSource::Pattern(id) => Some(*id),
            _ => None,
        }
    }

    pub fn take(&self) -> Option<TakeId> {
        match self {
            LaneSource::Take(id) => Some(*id),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, LaneSource::Empty)
    }
}

fn offset_steps_is_zero(offset: &f64) -> bool {
    *offset == 0.0
}

/// A complete session launch state beginning at `start_beat`: a base scene
/// plus the complete set of per-track overrides. An override absent from the
/// row is inactive even if the preceding row had one (spec 5.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSongRow {
    pub id: SongRowId,
    pub start_beat: f64,
    /// The scene *marked* over this row, or `None` for an unscened row
    /// (empty-arrangement spec 4.2): no label, and an absent override
    /// resolves to silence instead of a scene cell. Serde reads a legacy
    /// bare number as `Some`, so pre-v7 rows load unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<usize>,
    #[serde(default)]
    pub overrides: Vec<ProjectSongTrackOverride>,
}

/// The committed song: rows ordered by `start_beat`, an explicit end
/// position, and the monotonic row-id allocator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSong {
    pub rows: Vec<ProjectSongRow>,
    pub end_beat: f64,
    #[serde(default)]
    pub loop_enabled: bool,
    /// Monotonic allocator for `SongRowId`; never reused within a project.
    pub next_row_id: u64,
}

/// One span of the per-track lane projection (spec 5.5). Derived, never
/// stored: `pattern` resolves override-else-scene-cell-else-`None`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneClip {
    /// Row whose state this span comes from.
    pub row_id: SongRowId,
    pub start_beat: f64,
    /// Next row's start, or the song end.
    pub end_beat: f64,
    /// Resolved pattern: override, else scene cell, else `None`. For a take
    /// span this stays `None`; consult `source`.
    pub pattern: Option<PatternId>,
    /// Resolved content source (takes spec 6.2): override source, else the
    /// scene cell's pattern, else `Empty`.
    pub source: LaneSource,
    /// Start offset into the source in fractional steps (takes spec 6.2):
    /// the override's stored offset, or `0.0` for scene-resolved spans.
    pub offset_steps: f64,
    /// Render hint: `true` when the source came from a row override.
    pub from_override: bool,
}

/// Minimal borrowed view of the project a song is validated against: how many
/// scenes and tracks exist and which pattern-pool ids each track holds.
pub trait SongProjectContext {
    fn song_scene_count(&self) -> usize;
    fn song_track_count(&self) -> usize;
    fn song_track_pattern_exists(&self, track: usize, pattern_id: u64) -> bool;
    /// Playable length in steps of `take_id` in `track`'s take pool, or
    /// `None` when the take does not exist (takes spec 6.3). Defaults to
    /// "no takes exist" so contexts predating takes stay valid.
    fn song_track_take_len(&self, _track: usize, _take_id: u64) -> Option<u32> {
        None
    }
}

impl SongProjectContext for ProjectScenes {
    fn song_scene_count(&self) -> usize {
        self.scenes.len()
    }

    fn song_track_count(&self) -> usize {
        self.track_pools.len()
    }

    fn song_track_pattern_exists(&self, track: usize, pattern_id: u64) -> bool {
        self.track_pools
            .get(track)
            .is_some_and(|pool| pool.contains(PatternId(pattern_id)))
    }

    fn song_track_take_len(&self, track: usize, take_id: u64) -> Option<u32> {
        self.take_pools
            .get(track)
            .and_then(|takes| takes.get(TakeId(take_id)))
            .map(|take| take.total_len_steps)
    }
}

/// Validation context for a serialized project file. On load the pattern
/// pools are rebuilt from scene cells (`ProjectScenes::from_pattern_snapshots`
/// inserts one pool entry per scene per track, ids starting at 1), so every
/// track's pool holds exactly the ids `1..=scene_count`.
#[derive(Clone, Debug, Default)]
pub struct SerializedSongContext {
    pub scene_count: usize,
    pub track_count: usize,
    /// `(take_id, total_len_steps)` per track from the serialized take
    /// pools (takes spec 6.3). Empty for files predating takes.
    pub takes: Vec<Vec<(u64, u32)>>,
}

impl SongProjectContext for SerializedSongContext {
    fn song_scene_count(&self) -> usize {
        self.scene_count
    }

    fn song_track_count(&self) -> usize {
        self.track_count
    }

    fn song_track_pattern_exists(&self, track: usize, pattern_id: u64) -> bool {
        track < self.track_count && pattern_id >= 1 && pattern_id <= self.scene_count as u64
    }

    fn song_track_take_len(&self, track: usize, take_id: u64) -> Option<u32> {
        self.takes
            .get(track)?
            .iter()
            .find(|(id, _)| *id == take_id)
            .map(|(_, len)| *len)
    }
}

impl ProjectSong {
    /// Check every rule of spec 5.3 against `ctx`. Errors are actionable and
    /// never clamp, reorder, or drop invalid data.
    pub fn validate(&self, ctx: &dyn SongProjectContext) -> Result<(), String> {
        if self.rows.is_empty() {
            return Err("Song must contain at least one row".to_string());
        }
        for (idx, row) in self.rows.iter().enumerate() {
            if !row.start_beat.is_finite() || row.start_beat < 0.0 {
                return Err(format!(
                    "Song row {} start beat {} must be finite and non-negative",
                    idx + 1,
                    row.start_beat
                ));
            }
        }
        let first = self.rows[0].start_beat;
        if first != 0.0 {
            return Err(format!(
                "Song row 1 must start at beat 0.0, found {first}"
            ));
        }
        for (idx, pair) in self.rows.windows(2).enumerate() {
            if pair[1].start_beat <= pair[0].start_beat {
                return Err(format!(
                    "Song rows {} and {} are not strictly ordered by start beat ({} then {})",
                    idx + 1,
                    idx + 2,
                    pair[0].start_beat,
                    pair[1].start_beat
                ));
            }
        }
        let last_start = self.rows[self.rows.len() - 1].start_beat;
        if !self.end_beat.is_finite() || self.end_beat <= last_start {
            return Err(format!(
                "Song end beat {} must be finite and greater than the last row's start beat {}",
                self.end_beat, last_start
            ));
        }
        for (idx, row) in self.rows.iter().enumerate() {
            if let Some(scene) = row.scene {
                if scene >= ctx.song_scene_count() {
                    return Err(format!(
                        "Song row {} references scene {} but the project has {} scene(s)",
                        idx + 1,
                        scene + 1,
                        ctx.song_scene_count()
                    ));
                }
            }
            for pair in row.overrides.windows(2) {
                if pair[1].track == pair[0].track {
                    return Err(format!(
                        "Song row {} contains more than one override for track {}",
                        idx + 1,
                        pair[0].track + 1
                    ));
                }
                if pair[1].track < pair[0].track {
                    return Err(format!(
                        "Song row {} overrides are not in ascending track order",
                        idx + 1
                    ));
                }
            }
            for over in &row.overrides {
                if over.track >= ctx.song_track_count() {
                    return Err(format!(
                        "Song row {} overrides track {} but the project has {} track(s)",
                        idx + 1,
                        over.track + 1,
                        ctx.song_track_count()
                    ));
                }
                if let Some(pattern_id) = over.pattern_id {
                    if !ctx.song_track_pattern_exists(over.track, pattern_id) {
                        return Err(format!(
                            "Song row {} references pattern {} which is not in track {}'s \
                             pattern pool",
                            idx + 1,
                            pattern_id,
                            over.track + 1
                        ));
                    }
                }
                if let Some(take_id) = over.take_id {
                    // Takes spec 6.3: a take override carries no pattern id,
                    // must name an existing take, and cannot start past the
                    // take's end (takes never wrap).
                    if over.pattern_id.is_some() {
                        return Err(format!(
                            "Song row {} track {} carries both a take and a pattern; a take \
                             override must have no pattern id",
                            idx + 1,
                            over.track + 1
                        ));
                    }
                    let Some(total_len) = ctx.song_track_take_len(over.track, take_id) else {
                        return Err(format!(
                            "Song row {} references take {} which is not in track {}'s \
                             take pool",
                            idx + 1,
                            take_id,
                            over.track + 1
                        ));
                    };
                    if over.offset_steps >= total_len as f64 {
                        return Err(format!(
                            "Song row {} track {} take offset {} is at or past the take's \
                             end ({} steps); takes never wrap",
                            idx + 1,
                            over.track + 1,
                            over.offset_steps,
                            total_len
                        ));
                    }
                }
                if !over.offset_steps.is_finite() || over.offset_steps < 0.0 {
                    return Err(format!(
                        "Song row {} track {} offset {} must be a finite, non-negative \
                         step count",
                        idx + 1,
                        over.track + 1,
                        over.offset_steps
                    ));
                }
            }
        }
        for (idx, pair) in self.rows.windows(2).enumerate() {
            if pair[0].scene == pair[1].scene && pair[0].overrides == pair[1].overrides {
                return Err(format!(
                    "Song rows {} and {} contain identical launch states; \
                     normalization removes the redundant later row",
                    idx + 1,
                    idx + 2
                ));
            }
        }
        let mut seen = HashSet::new();
        for (idx, row) in self.rows.iter().enumerate() {
            if !seen.insert(row.id) {
                return Err(format!(
                    "Song row {} reuses row id {}; row ids must be unique",
                    idx + 1,
                    row.id.0
                ));
            }
            if row.id.0 >= self.next_row_id {
                return Err(format!(
                    "Song row {} has id {} but next_row_id is {}; \
                     ids must be less than the allocator",
                    idx + 1,
                    row.id.0,
                    self.next_row_id
                ));
            }
        }
        Ok(())
    }

    /// Spec 5.3 canonical form: remove each row whose launch state equals the
    /// immediately preceding row's. The earlier row (and its id) survives.
    pub fn normalize(&mut self) {
        self.rows.dedup_by(|later, earlier| {
            earlier.scene == later.scene && earlier.overrides == later.overrides
        });
    }

    /// Allocate a fresh `SongRowId`. Ids are monotonic and never reused
    /// within a project; exhaustion is an error, mirroring the `SceneId`
    /// allocator in `scenes.rs`.
    pub fn allocate_row_id(&mut self) -> Result<SongRowId, String> {
        let id = SongRowId(self.next_row_id);
        self.next_row_id = self
            .next_row_id
            .checked_add(1)
            .ok_or_else(|| "song row identity space exhausted".to_string())?;
        Ok(id)
    }
}

/// Spec 5.5 "state at beat": the row with the greatest `start_beat <= beat`,
/// or `None` when `beat >= end_beat` (`beat` is loop-normalized first when
/// `loop_enabled`). Assumes a valid song (rows sorted by `start_beat`).
pub fn state_at_beat(song: &ProjectSong, beat: f64) -> Option<&ProjectSongRow> {
    if !beat.is_finite() || beat < 0.0 {
        return None;
    }
    let beat = if song.loop_enabled {
        if song.end_beat > 0.0 {
            beat.rem_euclid(song.end_beat)
        } else {
            return None;
        }
    } else {
        if beat >= song.end_beat {
            return None;
        }
        beat
    };
    song.rows.iter().rev().find(|row| row.start_beat <= beat)
}

/// Spec 5.5 track-lane projection: for every track (outer index), the ordered
/// clip spans covering `[0, end_beat)` with no gaps or overlaps. Adjacent
/// equal spans are deliberately NOT merged — merging is a view concern.
/// Assumes a valid song.
pub fn project_lanes(song: &ProjectSong, scenes: &ProjectScenes) -> Vec<Vec<LaneClip>> {
    (0..scenes.track_pools.len())
        .map(|track| {
            song.rows
                .iter()
                .enumerate()
                .map(|(idx, row)| {
                    let end_beat = song
                        .rows
                        .get(idx + 1)
                        .map(|next| next.start_beat)
                        .unwrap_or(song.end_beat);
                    let override_entry = row
                        .overrides
                        .iter()
                        .find(|over| over.track == track);
                    let (source, offset_steps, from_override) = match override_entry {
                        // Explicit-empty (`pattern_id: None`, no take)
                        // resolves to no source WITHOUT falling back to the
                        // scene cell.
                        Some(over) => (over.source(), over.offset_steps, true),
                        None => (
                            row.scene
                                .and_then(|scene| scenes.scenes.get(scene))
                                .and_then(|scene| scene.cells.get(track))
                                .copied()
                                .flatten()
                                .map(LaneSource::Pattern)
                                .unwrap_or(LaneSource::Empty),
                            0.0,
                            false,
                        ),
                    };
                    LaneClip {
                        row_id: row.id,
                        start_beat: row.start_beat,
                        end_beat,
                        pattern: source.pattern(),
                        source,
                        offset_steps,
                        from_override,
                    }
                })
                .collect()
        })
        .collect()
}

/// 0-based positions of the rows whose base scene is `scene`.
pub fn song_rows_referencing_scene(song: &ProjectSong, scene: usize) -> Vec<usize> {
    song.rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.scene == Some(scene))
        .map(|(idx, _)| idx)
        .collect()
}

/// 0-based positions of the rows carrying an override for `track` that
/// references `pattern_id`.
pub fn song_rows_referencing_track_pattern(
    song: &ProjectSong,
    track: usize,
    pattern_id: u64,
) -> Vec<usize> {
    song.rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row.overrides
                .iter()
                .any(|over| over.track == track && over.pattern_id == Some(pattern_id))
        })
        .map(|(idx, _)| idx)
        .collect()
}

/// Format 0-based row positions as a 1-based, comma-separated list for
/// actionable "referenced by song row(s) ..." errors.
pub fn format_song_row_positions(positions: &[usize]) -> String {
    positions
        .iter()
        .map(|idx| (idx + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Decrement scene references above a deleted scene index. The caller must
/// already have rejected the deletion when any row references the deleted
/// scene itself (spec 5.4).
pub fn remap_song_after_scene_delete(song: &mut ProjectSong, deleted_scene: usize) {
    for row in &mut song.rows {
        if let Some(scene) = row.scene.as_mut() {
            if *scene > deleted_scene {
                *scene -= 1;
            }
        }
    }
}

/// Translate a live committed song into the id domain the project loader
/// rebuilds: pools are reconstructed from scene cells, so track `t`'s cell in
/// scene `j` becomes `PatternId(j + 1)` on load. Each override's live pool id
/// is mapped to the first scene whose cell for that track holds it. A pattern
/// referenced only by the song (assigned to no scene cell) is not persisted
/// by the project format at all, so saving such a song is rejected with the
/// referencing row positions rather than silently dropped.
pub fn song_for_serialization(
    song: &ProjectSong,
    scenes: &ProjectScenes,
) -> Result<ProjectSong, String> {
    let mut serialized = song.clone();
    for (idx, row) in serialized.rows.iter_mut().enumerate() {
        for over in &mut row.overrides {
            // Explicit-empty overrides carry no pool id; they serialize as-is.
            let Some(live_raw) = over.pattern_id else {
                continue;
            };
            let live_id = PatternId(live_raw);
            let scene_idx = scenes
                .scenes
                .iter()
                .position(|scene| scene.cells.get(over.track).copied().flatten() == Some(live_id))
                .ok_or_else(|| {
                    format!(
                        "Song row {} references track {} pattern {} which is not assigned \
                         to any scene cell and cannot be saved; assign it to a scene cell \
                         or update the song row",
                        idx + 1,
                        over.track + 1,
                        live_raw
                    )
                })?;
            over.pattern_id = Some(scene_idx as u64 + 1);
        }
    }
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn over(track: usize, pattern_id: u64) -> ProjectSongTrackOverride {
        ProjectSongTrackOverride::new(track, Some(pattern_id))
    }

    fn empty_over(track: usize) -> ProjectSongTrackOverride {
        ProjectSongTrackOverride::new(track, None)
    }

    fn row(id: u64, start_beat: f64, scene: usize, overrides: Vec<ProjectSongTrackOverride>) -> ProjectSongRow {
        ProjectSongRow {
            id: SongRowId(id),
            start_beat,
            scene: Some(scene),
            overrides,
        }
    }

    fn ctx() -> SerializedSongContext {
        SerializedSongContext {
            scene_count: 3,
            track_count: 2,
            takes: Vec::new(),
        }
    }

    fn valid_song() -> ProjectSong {
        ProjectSong {
            rows: vec![
                row(0, 0.0, 0, vec![over(1, 2)]),
                row(1, 32.0, 1, Vec::new()),
                row(2, 47.5, 2, vec![over(0, 1), over(1, 3)]),
            ],
            end_beat: 64.0,
            loop_enabled: false,
            next_row_id: 3,
        }
    }

    /// Two-track, three-scene project. Per-track pool ids are 1..=3 with
    /// scene j's cell holding PatternId(j + 1) — the rebuilt-on-load shape.
    fn test_scenes() -> ProjectScenes {
        let snapshots = vec![
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
            PatternSnapshot::new_default(2, &[]),
        ];
        ProjectScenes::from_pattern_snapshots(&snapshots, 0)
    }

    /// `test_scenes` plus one 300-step, two-chunk take on track 0.
    fn scenes_with_take() -> (ProjectScenes, TakeId) {
        let mut scenes = test_scenes();
        let chunk_data = scenes.track_pools[0].get(PatternId(1)).unwrap();
        let chunk_a = scenes.track_pools[0].insert(chunk_data.clone());
        let chunk_b = scenes.track_pools[0].insert(chunk_data);
        let sound = scenes.track_pools[0].refs(chunk_a).expect("chunk refs");
        let take = scenes.take_pools[0].insert(None, vec![chunk_a, chunk_b], 300, sound);
        (scenes, take)
    }

    #[test]
    fn take_override_validates_against_the_take_pool() {
        let (scenes, take) = scenes_with_take();
        let mut song = valid_song();
        song.rows[1].overrides = vec![ProjectSongTrackOverride::new_take(0, take.0, 12.5)];
        song.validate(&scenes).expect("take override validates");

        // Unknown take id.
        song.rows[1].overrides = vec![ProjectSongTrackOverride::new_take(0, 99, 0.0)];
        let err = song.validate(&scenes).unwrap_err();
        assert!(err.contains("take 99"), "{err}");

        // Take and pattern on the same override.
        song.rows[1].overrides = vec![ProjectSongTrackOverride {
            track: 0,
            pattern_id: Some(1),
            take_id: Some(take.0),
            offset_steps: 0.0,
        }];
        let err = song.validate(&scenes).unwrap_err();
        assert!(err.contains("both a take and a pattern"), "{err}");

        // Offset at/past the take end (takes never wrap, spec 6.3).
        song.rows[1].overrides = vec![ProjectSongTrackOverride::new_take(0, take.0, 300.0)];
        let err = song.validate(&scenes).unwrap_err();
        assert!(err.contains("past the take's end"), "{err}");
    }

    #[test]
    fn take_override_serde_round_trips_and_is_skipped_when_absent() {
        let over = ProjectSongTrackOverride::new_take(1, 3, 7.5);
        let json = serde_json::to_string(&over).expect("serialize take override");
        assert!(json.contains("take_id"), "{json}");
        let restored: ProjectSongTrackOverride =
            serde_json::from_str(&json).expect("deserialize take override");
        assert_eq!(restored, over);

        // Pattern overrides keep the pre-take wire shape.
        let json = serde_json::to_string(&super::tests::over(0, 5)).expect("serialize");
        assert!(!json.contains("take_id"), "{json}");
        let legacy = r#"{"track":1,"pattern_id":3}"#;
        let restored: ProjectSongTrackOverride =
            serde_json::from_str(legacy).expect("deserialize legacy override");
        assert_eq!(restored.take_id, None);
    }

    #[test]
    fn project_lanes_resolves_take_sources() {
        let (scenes, take) = scenes_with_take();
        let mut song = valid_song();
        song.rows[1].overrides = vec![ProjectSongTrackOverride::new_take(0, take.0, 4.0)];
        let lanes = project_lanes(&song, &scenes);
        let clip = &lanes[0][1];
        assert_eq!(clip.source, LaneSource::Take(take));
        assert_eq!(clip.pattern, None, "take spans expose no pattern id");
        assert_eq!(clip.offset_steps, 4.0);
        assert!(clip.from_override);
        // Scene-resolved spans stay pattern sources.
        assert_eq!(lanes[0][0].source, LaneSource::Pattern(PatternId(1)));
        // Explicit-empty resolves to `Empty`.
        song.rows[1].overrides = vec![empty_over(0)];
        let lanes = project_lanes(&song, &scenes);
        assert!(lanes[0][1].source.is_empty());
    }

    #[test]
    fn valid_song_passes_validation() {
        valid_song().validate(&ctx()).expect("song should validate");
    }

    #[test]
    fn validate_rejects_empty_song() {
        let song = ProjectSong {
            rows: Vec::new(),
            end_beat: 16.0,
            loop_enabled: false,
            next_row_id: 0,
        };
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("at least one row"), "{err}");
    }

    #[test]
    fn validate_rejects_nonzero_first_row() {
        let mut song = valid_song();
        song.rows[0].start_beat = 1.0;
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("must start at beat 0.0"), "{err}");
    }

    #[test]
    fn validate_rejects_non_finite_and_negative_beats() {
        let mut song = valid_song();
        song.rows[1].start_beat = f64::NAN;
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("finite and non-negative"), "{err}");

        let mut song = valid_song();
        song.rows[1].start_beat = -4.0;
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("finite and non-negative"), "{err}");
    }

    #[test]
    fn validate_rejects_unordered_rows() {
        let mut song = valid_song();
        song.rows[2].start_beat = 32.0;
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("strictly ordered"), "{err}");
    }

    #[test]
    fn validate_rejects_end_beat_not_after_last_row() {
        let mut song = valid_song();
        song.end_beat = 47.5;
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("greater than the last row's start beat"), "{err}");

        let mut song = valid_song();
        song.end_beat = f64::INFINITY;
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("finite"), "{err}");
    }

    #[test]
    fn validate_rejects_missing_scene() {
        let mut song = valid_song();
        song.rows[1].scene = Some(3);
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("references scene 4"), "{err}");
    }

    #[test]
    fn validate_rejects_missing_override_track() {
        let mut song = valid_song();
        song.rows[0].overrides = vec![over(2, 1)];
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("overrides track 3"), "{err}");
    }

    #[test]
    fn validate_rejects_pattern_missing_from_pool() {
        let mut song = valid_song();
        song.rows[0].overrides = vec![over(1, 9)];
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("pattern 9"), "{err}");
        assert!(err.contains("track 2"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_and_unsorted_overrides() {
        let mut song = valid_song();
        song.rows[2].overrides = vec![over(1, 1), over(1, 2)];
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("more than one override for track 2"), "{err}");

        let mut song = valid_song();
        song.rows[2].overrides = vec![over(1, 3), over(0, 1)];
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("ascending track order"), "{err}");
    }

    #[test]
    fn validate_rejects_adjacent_identical_rows() {
        let mut song = valid_song();
        song.rows[1].scene = Some(0);
        song.rows[1].overrides = vec![over(1, 2)];
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("identical launch states"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_and_out_of_range_row_ids() {
        let mut song = valid_song();
        song.rows[2].id = SongRowId(0);
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("reuses row id 0"), "{err}");

        let mut song = valid_song();
        song.rows[2].id = SongRowId(7);
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("next_row_id"), "{err}");
    }

    #[test]
    fn normalize_removes_later_adjacent_identical_row_keeping_earlier_id() {
        let mut song = ProjectSong {
            rows: vec![
                row(0, 0.0, 0, vec![over(0, 1)]),
                row(1, 8.0, 0, vec![over(0, 1)]),
                row(2, 16.0, 1, Vec::new()),
                row(3, 24.0, 0, vec![over(0, 1)]),
            ],
            end_beat: 32.0,
            loop_enabled: false,
            next_row_id: 4,
        };
        song.normalize();
        let ids: Vec<u64> = song.rows.iter().map(|r| r.id.0).collect();
        // Row 1 is folded into row 0; the non-adjacent identical row 3 stays.
        assert_eq!(ids, vec![0, 2, 3]);
        assert_eq!(song.rows[0].start_beat, 0.0);
    }

    #[test]
    fn allocate_row_id_is_monotonic_and_errors_on_exhaustion() {
        let mut song = valid_song();
        assert_eq!(song.allocate_row_id().unwrap(), SongRowId(3));
        assert_eq!(song.allocate_row_id().unwrap(), SongRowId(4));
        assert_eq!(song.next_row_id, 5);

        song.next_row_id = u64::MAX;
        let err = song.allocate_row_id().unwrap_err();
        assert!(err.contains("exhausted"), "{err}");
        assert_eq!(song.next_row_id, u64::MAX);
    }

    #[test]
    fn state_at_beat_boundary_cases() {
        let song = valid_song();
        // Exactly on a row start.
        assert_eq!(state_at_beat(&song, 0.0).unwrap().id, SongRowId(0));
        assert_eq!(state_at_beat(&song, 32.0).unwrap().id, SongRowId(1));
        assert_eq!(state_at_beat(&song, 47.5).unwrap().id, SongRowId(2));
        // Between rows.
        assert_eq!(state_at_beat(&song, 31.999).unwrap().id, SongRowId(0));
        assert_eq!(state_at_beat(&song, 40.0).unwrap().id, SongRowId(1));
        // At and past end_beat.
        assert!(state_at_beat(&song, 64.0).is_none());
        assert!(state_at_beat(&song, 100.0).is_none());
        // Invalid inputs.
        assert!(state_at_beat(&song, -1.0).is_none());
        assert!(state_at_beat(&song, f64::NAN).is_none());
    }

    #[test]
    fn state_at_beat_loop_normalizes_first() {
        let mut song = valid_song();
        song.loop_enabled = true;
        // end_beat wraps to beat zero.
        assert_eq!(state_at_beat(&song, 64.0).unwrap().id, SongRowId(0));
        // 64 + 40 wraps into row 1's span.
        assert_eq!(state_at_beat(&song, 104.0).unwrap().id, SongRowId(1));
        // Second wrap lands exactly on row 2's start.
        assert_eq!(state_at_beat(&song, 64.0 * 2.0 + 47.5).unwrap().id, SongRowId(2));
    }

    #[test]
    fn project_lanes_covers_full_span_with_override_and_scene_resolution() {
        let scenes = test_scenes();
        let song = valid_song();
        let lanes = project_lanes(&song, &scenes);
        assert_eq!(lanes.len(), 2);

        for lane in &lanes {
            // Full coverage of [0, end_beat) with no gaps or overlaps.
            assert_eq!(lane.len(), song.rows.len());
            assert_eq!(lane[0].start_beat, 0.0);
            assert_eq!(lane[lane.len() - 1].end_beat, song.end_beat);
            for pair in lane.windows(2) {
                assert_eq!(pair[0].end_beat, pair[1].start_beat);
            }
        }

        // Track 0: scene-provided in rows 0 and 1, overridden in row 2.
        assert_eq!(lanes[0][0].pattern, Some(PatternId(1)));
        assert!(!lanes[0][0].from_override);
        assert_eq!(lanes[0][1].pattern, Some(PatternId(2)));
        assert!(!lanes[0][1].from_override);
        assert_eq!(lanes[0][2].pattern, Some(PatternId(1)));
        assert!(lanes[0][2].from_override);

        // Track 1: overridden in row 0, scene-provided in row 1 (the
        // override does not leak forward), overridden again in row 2.
        assert_eq!(lanes[1][0].pattern, Some(PatternId(2)));
        assert!(lanes[1][0].from_override);
        assert_eq!(lanes[1][1].pattern, Some(PatternId(2)));
        assert!(!lanes[1][1].from_override);
        assert_eq!(lanes[1][2].pattern, Some(PatternId(3)));
        assert!(lanes[1][2].from_override);

        // Row ids ride along for editor identity.
        assert_eq!(lanes[0][2].row_id, SongRowId(2));
    }

    #[test]
    fn project_lanes_resolves_empty_cell_to_none() {
        let mut scenes = test_scenes();
        scenes.scenes[1].cells[0] = None;
        let song = valid_song();
        let lanes = project_lanes(&song, &scenes);
        assert_eq!(lanes[0][1].pattern, None);
        assert!(!lanes[0][1].from_override);
    }

    #[test]
    fn explicit_empty_override_validates_and_silences_the_lane() {
        let scenes = test_scenes();
        let mut song = valid_song();
        // Row 1's scene cell for track 0 holds PatternId(2); an
        // explicit-empty override must win over it.
        song.rows[1].overrides = vec![empty_over(0)];
        song.validate(&ctx()).expect("explicit-empty override validates");
        let lanes = project_lanes(&song, &scenes);
        assert_eq!(lanes[0][1].pattern, None, "no fallback to the scene cell");
        assert!(lanes[0][1].from_override);
        // The other track's lane is untouched.
        assert_eq!(lanes[1][1].pattern, Some(PatternId(2)));
    }

    #[test]
    fn explicit_empty_override_serde_round_trips_and_reads_legacy_numbers() {
        let mut song = valid_song();
        song.rows[1].overrides = vec![empty_over(0)];
        let json = serde_json::to_string(&song).expect("serialize song");
        let restored: ProjectSong = serde_json::from_str(&json).expect("deserialize song");
        assert_eq!(restored, song);

        // Legacy files store bare numbers; they must load as `Some`.
        let json = r#"{"track":1,"pattern_id":3}"#;
        let restored: ProjectSongTrackOverride =
            serde_json::from_str(json).expect("deserialize legacy override");
        assert_eq!(restored, over(1, 3));
    }

    #[test]
    fn offset_steps_round_trips_and_defaults_to_zero_on_legacy_json() {
        // A nonzero offset survives serialization exactly.
        let mut song = valid_song();
        song.rows[0].overrides = vec![ProjectSongTrackOverride {
            track: 1,
            pattern_id: Some(2),
            take_id: None,
            offset_steps: 7.25,
        }];
        song.validate(&ctx()).expect("offset override validates");
        let json = serde_json::to_string(&song).expect("serialize song");
        let restored: ProjectSong = serde_json::from_str(&json).expect("deserialize song");
        assert_eq!(restored, song);

        // Legacy override JSON (no offset field) loads with offset 0.0.
        let json = r#"{"track":1,"pattern_id":3}"#;
        let restored: ProjectSongTrackOverride =
            serde_json::from_str(json).expect("deserialize legacy override");
        assert_eq!(restored.offset_steps, 0.0);
        assert_eq!(restored, over(1, 3));

        // A zero offset is skipped on write, keeping files byte-identical
        // to the pre-offset format.
        let json = serde_json::to_string(&over(0, 5)).expect("serialize override");
        assert!(!json.contains("offset_steps"), "{json}");

        // Validation rejects negative and non-finite offsets.
        let mut song = valid_song();
        song.rows[0].overrides = vec![ProjectSongTrackOverride {
            track: 1,
            pattern_id: Some(2),
            take_id: None,
            offset_steps: -1.0,
        }];
        let err = song.validate(&ctx()).unwrap_err();
        assert!(err.contains("offset"), "{err}");
    }

    #[test]
    fn project_lanes_carries_override_offsets() {
        let scenes = test_scenes();
        let mut song = valid_song();
        song.rows[2].overrides = vec![ProjectSongTrackOverride {
            track: 0,
            pattern_id: Some(1),
            take_id: None,
            offset_steps: 3.5,
        }];
        let lanes = project_lanes(&song, &scenes);
        assert_eq!(lanes[0][2].offset_steps, 3.5);
        // Scene-resolved spans anchor at offset 0.
        assert_eq!(lanes[0][0].offset_steps, 0.0);
        assert_eq!(lanes[0][1].offset_steps, 0.0);
    }

    #[test]
    fn song_for_serialization_passes_explicit_empty_overrides_through() {
        let scenes = test_scenes();
        let mut song = valid_song();
        song.rows[1].overrides = vec![empty_over(0)];
        let serialized = song_for_serialization(&song, &scenes).expect("serializable");
        assert_eq!(serialized.rows[1].overrides, vec![empty_over(0)]);
    }

    #[test]
    fn song_reference_queries_report_row_positions() {
        let song = valid_song();
        assert_eq!(song_rows_referencing_scene(&song, 1), vec![1]);
        assert!(song_rows_referencing_scene(&song, 5).is_empty());
        assert_eq!(song_rows_referencing_track_pattern(&song, 1, 2), vec![0]);
        assert_eq!(song_rows_referencing_track_pattern(&song, 1, 3), vec![2]);
        assert!(song_rows_referencing_track_pattern(&song, 0, 9).is_empty());
        assert_eq!(format_song_row_positions(&[0, 2]), "1, 3");
    }

    #[test]
    fn remap_song_after_scene_delete_decrements_higher_scenes() {
        let mut song = valid_song();
        remap_song_after_scene_delete(&mut song, 1);
        assert_eq!(song.rows[0].scene, Some(0));
        assert_eq!(song.rows[2].scene, Some(1));
    }

    #[test]
    fn song_for_serialization_maps_pool_ids_to_scene_cell_positions() {
        let scenes = test_scenes();
        let song = valid_song();
        // In the rebuilt-shape pools the ids already equal scene index + 1,
        // so serialization is the identity here.
        let serialized = song_for_serialization(&song, &scenes).expect("serializable");
        assert_eq!(serialized, song);
    }

    #[test]
    fn song_for_serialization_rejects_pattern_not_in_any_scene_cell() {
        let mut scenes = test_scenes();
        // Fork a pattern into track 1's pool without assigning it to a cell.
        let source = scenes.track_pools[1].get(PatternId(1)).unwrap();
        let orphan = scenes.track_pools[1].insert(source);
        let mut song = valid_song();
        song.rows[0].overrides = vec![over(1, orphan.0)];
        let err = song_for_serialization(&song, &scenes).unwrap_err();
        assert!(err.contains("Song row 1"), "{err}");
        assert!(err.contains("not assigned"), "{err}");
    }

    #[test]
    fn song_serde_round_trips_and_defaults_loop_enabled() {
        let mut song = valid_song();
        song.loop_enabled = true;
        let json = serde_json::to_string(&song).expect("serialize song");
        let restored: ProjectSong = serde_json::from_str(&json).expect("deserialize song");
        assert_eq!(restored, song);

        // loop_enabled and overrides are serde-defaulted.
        let json = r#"{"rows":[{"id":0,"start_beat":0.0,"scene":0}],"end_beat":8.0,"next_row_id":1}"#;
        let restored: ProjectSong = serde_json::from_str(json).expect("deserialize minimal song");
        assert!(!restored.loop_enabled);
        assert!(restored.rows[0].overrides.is_empty());
    }
}
