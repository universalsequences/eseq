//! Audio regions on the arrangement (docs/audio-tracks-spec.md §9).
//!
//! An audio track's lane is a sorted, non-overlapping list of regions, each
//! playing a span of one immutable source file at its native speed (warp
//! off). Regions live in `ProjectArrangement` beside the clip lanes so they
//! share its validation, undo patches and track remaps, and take their ids
//! from the same `ClipId` allocator so arrangement selection addresses clips
//! and regions alike. They are never compiled into song rows: the region
//! player reads them directly (`app::audio_tracks`).

use super::*;

/// Identity of an immutable audio source owned by the project. Allocated
/// from `ProjectArrangement::next_audio_source_id`, never reused.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AudioSourceId(pub u64);

/// What the project knows about one source file without decoding it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSourceMeta {
    /// File name inside the project's audio folder (`<sha256>.wav`).
    pub file: String,
    /// Display name (the dropped sample's title).
    pub name: String,
    pub sample_rate: u32,
    /// Channel count of the original file; playback is always stereo.
    pub channels: u16,
    pub frames: u64,
}

impl AudioSourceMeta {
    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }
}

/// One region on an audio lane: over `[start_beat, end_beat)` the track plays
/// its source from `source_offset` at native speed, repitched by `transpose`
/// (warp off: pitch and speed move together). A looping region jumps back to
/// `loop_start` whenever it reaches `loop_end`, so extending it repeats the
/// loop; otherwise a span longer than the audio left plays silence past the
/// end, as a tempo change can produce.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioRegion {
    /// Drawn from the same allocator as `ArrClip::id`.
    pub id: ClipId,
    pub source: AudioSourceId,
    pub start_beat: f64,
    /// Exclusive; must be `> start_beat`.
    pub end_beat: f64,
    /// Source frame heard at `start_beat`.
    #[serde(default)]
    pub source_offset: u64,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub gain_db: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub fade_in_seconds: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub fade_out_seconds: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    /// Semitones; speed scales by `2^(transpose/12)`.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub transpose: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub loop_enabled: bool,
    /// The loop window in source frames, `[loop_start, loop_end)`; read only
    /// while `loop_enabled`.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub loop_start: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub loop_end: u64,
    /// Parameter values this region locks while it plays, like step p-locks
    /// (eseq-a921): the track's one fx/mixer state otherwise. At most one
    /// lock per target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locks: Vec<RegionLock>,
}

/// What a region lock sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RegionLockTarget {
    /// A parameter of the track's effect at `slot`. `effect` is that slot's
    /// effect id when the lock was made: a slot now holding another effect
    /// leaves the lock inert rather than writing into the wrong device.
    Effect { slot: u16, effect: String, param: u16 },
}

/// What the effect knobs show for region locks (eseq-a921): the regions
/// selected for locking, the locks of the first of them, and every effect
/// parameter some region of each audio track locks. Entries are validated
/// against the effect each slot holds now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioLockView {
    /// `(track, regions)`: an effect edit on `track` locks these regions.
    pub selection: Option<(usize, Vec<ClipId>)>,
    /// `(slot, param) -> value` of the first selected region's locks.
    pub shown: Vec<((usize, usize), f32)>,
    /// `(track, slot, param)` some region locks.
    pub locked: Vec<(usize, usize, usize)>,
}

impl AudioLockView {
    /// The regions an effect edit on `track` locks, if any are selected.
    pub fn regions_for(&self, track: usize) -> Option<&[ClipId]> {
        self.selection
            .as_ref()
            .filter(|(owner, regions)| *owner == track && !regions.is_empty())
            .map(|(_, regions)| regions.as_slice())
    }

    /// What a knob for `track`'s effect `slot`:`param` shows while regions
    /// are selected: `Some(Some(value))` locked, `Some(None)` unlocked (the
    /// track value), `None` when no region is selected there.
    pub fn shown(&self, track: usize, slot: usize, param: usize) -> Option<Option<f32>> {
        self.regions_for(track)?;
        Some(self.shown.iter().find(|(key, _)| *key == (slot, param)).map(|(_, value)| *value))
    }

    pub fn has_lock(&self, track: usize, slot: usize, param: usize) -> bool {
        self.locked.contains(&(track, slot, param))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionLock {
    pub target: RegionLockTarget,
    pub value: f32,
}

impl AudioRegion {
    pub fn lock(&self, target: &RegionLockTarget) -> Option<f32> {
        self.locks.iter().find(|lock| &lock.target == target).map(|lock| lock.value)
    }

    /// Set (`Some`) or clear (`None`) the lock on `target`.
    pub fn set_lock(&mut self, target: RegionLockTarget, value: Option<f32>) {
        let existing = self.locks.iter().position(|lock| lock.target == target);
        match (existing, value) {
            (Some(index), Some(value)) => self.locks[index].value = value,
            (Some(index), None) => {
                self.locks.remove(index);
            }
            (None, Some(value)) => self.locks.push(RegionLock { target, value }),
            (None, None) => {}
        }
    }
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}

impl LaneSpan for AudioRegion {
    fn id(&self) -> ClipId {
        self.id
    }
    fn set_id(&mut self, id: ClipId) {
        self.id = id;
    }
    fn start_beat(&self) -> f64 {
        self.start_beat
    }
    fn end_beat(&self) -> f64 {
        self.end_beat
    }
    fn set_end_beat(&mut self, end: f64) {
        self.end_beat = end;
    }
}

impl AudioRegion {
    pub fn new(id: ClipId, source: AudioSourceId, start_beat: f64, end_beat: f64) -> Self {
        Self {
            id,
            source,
            start_beat,
            end_beat,
            source_offset: 0,
            gain_db: 0.0,
            fade_in_seconds: 0.0,
            fade_out_seconds: 0.0,
            muted: false,
            transpose: 0.0,
            loop_enabled: false,
            loop_start: 0,
            loop_end: 0,
            locks: Vec::new(),
        }
    }

    /// Source frames per second of playback: the pitch's speed factor.
    pub fn speed(&self) -> f64 {
        2f64.powf(self.transpose as f64 / 12.0)
    }

    /// The loop window, when looping over a non-empty one.
    pub fn loop_window(&self) -> Option<(f64, f64)> {
        (self.loop_enabled && self.loop_end > self.loop_start)
            .then_some((self.loop_start as f64, self.loop_end as f64))
    }

    /// Where `position` source frames of straight playback land once the
    /// loop has folded them back into its window.
    pub fn fold_into_loop(&self, position: f64) -> f64 {
        match self.loop_window() {
            Some((start, end)) if position >= end => start + (position - end).rem_euclid(end - start),
            _ => position,
        }
    }

    /// The part of this region from `beat` on: the same audio, so the source
    /// offset advances by the frames that played before `beat` (at its
    /// pitch, folded into its loop).
    pub fn tail_at(&self, beat: f64, seconds_per_beat: f64, source_rate: u32) -> Self {
        let played = (beat - self.start_beat).max(0.0)
            * seconds_per_beat
            * source_rate as f64
            * self.speed();
        let position = self.fold_into_loop(self.source_offset as f64 + played);
        Self {
            start_beat: beat,
            source_offset: position.round() as u64,
            ..self.clone()
        }
    }
}

/// Beats a source of `frames` at `sample_rate` lasts at `bpm`.
pub fn audio_frames_to_beats(frames: u64, sample_rate: u32, bpm: f64) -> f64 {
    frames as f64 / sample_rate.max(1) as f64 * bpm / 60.0
}

impl ProjectArrangement {
    /// Track `track`'s audio lane, empty when it has none.
    pub fn audio_lane(&self, track: usize) -> &[AudioRegion] {
        self.audio_lanes.get(track).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Track `track`'s audio lane, created (with any lanes before it) when
    /// missing. Audio lanes are sparse: a project without audio tracks
    /// carries none.
    pub fn audio_lane_mut(&mut self, track: usize) -> &mut Vec<AudioRegion> {
        if self.audio_lanes.len() <= track {
            self.audio_lanes.resize_with(track + 1, Vec::new);
        }
        &mut self.audio_lanes[track]
    }

    /// Find a region by id, returning its track alongside it.
    pub fn find_audio_region(&self, id: ClipId) -> Option<(usize, &AudioRegion)> {
        self.audio_lanes.iter().enumerate().find_map(|(track, lane)| {
            lane.iter().find(|region| region.id == id).map(|region| (track, region))
        })
    }

    /// [`Self::find_audio_region`], mutably.
    pub fn find_audio_region_mut(&mut self, id: ClipId) -> Option<(usize, &mut AudioRegion)> {
        self.audio_lanes.iter_mut().enumerate().find_map(|(track, lane)| {
            lane.iter_mut().find(|region| region.id == id).map(|region| (track, region))
        })
    }

    pub fn audio_source(&self, id: AudioSourceId) -> Option<&AudioSourceMeta> {
        self.audio_sources.get(&id.0)
    }

    /// Take `before`'s sources and source-id allocator. Sources are project
    /// assets, not content: an edit that rebuilds the arrangement keeps them
    /// so source ids are never reused.
    pub fn inherit_audio_sources(&mut self, before: &ProjectArrangement) {
        self.audio_sources = before.audio_sources.clone();
        self.next_audio_source_id = before.next_audio_source_id;
    }

    /// The source meta `region` plays, or an error when it is missing.
    pub fn region_source(&self, region: &AudioRegion) -> Result<&AudioSourceMeta, String> {
        self.audio_source(region.source)
            .ok_or_else(|| "Audio region's source is missing".to_string())
    }

    /// Register `meta` as a new source, or return the existing source that
    /// already holds the same file (the folder is content-addressed, so the
    /// same file name is the same audio).
    pub fn add_audio_source(&mut self, meta: AudioSourceMeta) -> Result<AudioSourceId, String> {
        if let Some((id, _)) = self.audio_sources.iter().find(|(_, existing)| existing.file == meta.file) {
            return Ok(AudioSourceId(*id));
        }
        let id = AudioSourceId(self.next_audio_source_id);
        self.next_audio_source_id = self
            .next_audio_source_id
            .checked_add(1)
            .ok_or_else(|| "audio source identity space exhausted".to_string())?;
        self.audio_sources.insert(id.0, meta);
        Ok(id)
    }

    /// Clear `[start, end)` on `track`'s audio lane so an incoming region can
    /// own it (the same cases as `occlude_span`). A left-trimmed or split
    /// tail keeps playing the audio it played, so its source offset advances.
    pub fn occlude_audio_span(
        &mut self,
        track: usize,
        start: f64,
        end: f64,
        seconds_per_beat: f64,
    ) -> Result<(), String> {
        let lane = std::mem::take(self.audio_lane_mut(track));
        let sources = &self.audio_sources;
        let mut next_clip_id = self.next_clip_id;
        let kept = occlude_lane(
            lane,
            start,
            end,
            |region, beat| {
                let rate = sources.get(&region.source.0).map(|meta| meta.sample_rate)?;
                Some(region.tail_at(beat, seconds_per_beat, rate))
            },
            || {
                let id = ClipId(next_clip_id);
                next_clip_id = next_clip_id
                    .checked_add(1)
                    .ok_or_else(|| "arrangement clip identity space exhausted".to_string())?;
                Ok(id)
            },
        );
        self.next_clip_id = next_clip_id;
        self.audio_lanes[track] = kept?;
        Ok(())
    }

    /// Insert `region` keeping the lane sorted. The caller made room
    /// (`occlude_audio_span`).
    pub fn insert_audio_region_sorted(&mut self, track: usize, region: AudioRegion) {
        insert_sorted(self.audio_lane_mut(track), region);
    }

    /// Remove region `id`, returning it and its track.
    pub fn take_audio_region(&mut self, id: ClipId) -> Option<(usize, AudioRegion)> {
        let (track, index) = self.audio_lanes.iter().enumerate().find_map(|(track, lane)| {
            lane.iter().position(|region| region.id == id).map(|index| (track, index))
        })?;
        Some((track, self.audio_lanes[track].remove(index)))
    }

    /// Spec §9 rules for the audio lanes: spans as for every lane, each
    /// region's source registered, and its offset inside that source.
    pub(super) fn validate_audio_lanes(
        &self,
        track_count: usize,
        seen_ids: &mut HashSet<ClipId>,
    ) -> Result<(), String> {
        if self.audio_lanes.len() > track_count {
            return Err(format!(
                "Arrangement has {} audio lane(s) but the project has {} track(s)",
                self.audio_lanes.len(),
                track_count
            ));
        }
        for (track, lane) in self.audio_lanes.iter().enumerate() {
            self.validate_lane_spans(lane, &format!("Track {} audio", track + 1), seen_ids)?;
            for (idx, region) in lane.iter().enumerate() {
                let Some(source) = self.audio_source(region.source) else {
                    return Err(format!(
                        "Track {} audio region {} references audio source {} which the \
                         arrangement does not hold",
                        track + 1,
                        idx + 1,
                        region.source.0
                    ));
                };
                if region.source_offset >= source.frames {
                    return Err(format!(
                        "Track {} audio region {} starts at source frame {} but '{}' has {} \
                         frame(s)",
                        track + 1,
                        idx + 1,
                        region.source_offset,
                        source.name,
                        source.frames
                    ));
                }
                if region.loop_enabled
                    && !(region.loop_start < region.loop_end && region.loop_end <= source.frames)
                {
                    return Err(format!(
                        "Track {} audio region {} loops over [{}, {}) which is not inside '{}' ({} frame(s))",
                        track + 1,
                        idx + 1,
                        region.loop_start,
                        region.loop_end,
                        source.name,
                        source.frames
                    ));
                }
                for (what, value) in [
                    ("gain", region.gain_db),
                    ("transpose", region.transpose),
                    ("fade-in", region.fade_in_seconds),
                    ("fade-out", region.fade_out_seconds),
                ] {
                    if !value.is_finite() {
                        return Err(format!(
                            "Track {} audio region {} {what} {value} must be finite",
                            track + 1,
                            idx + 1
                        ));
                    }
                }
            }
        }
        for (id, _) in &self.audio_sources {
            if *id >= self.next_audio_source_id {
                return Err(format!(
                    "Audio source id {id} is not below next_audio_source_id {}",
                    self.next_audio_source_id
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(frames: u64) -> AudioSourceMeta {
        AudioSourceMeta {
            file: format!("{frames}.wav"),
            name: "hit".to_string(),
            sample_rate: 48_000,
            channels: 2,
            frames,
        }
    }

    /// 120 bpm: half a second per beat, 24 000 source frames per beat.
    const SPB: f64 = 0.5;

    fn arrangement_with_region(start: f64, end: f64) -> (ProjectArrangement, ClipId) {
        let mut arr = ProjectArrangement::empty(2);
        let source = arr.add_audio_source(source(48_000 * 8)).unwrap();
        let id = arr.allocate_clip_id().unwrap();
        arr.insert_audio_region_sorted(1, AudioRegion::new(id, source, start, end));
        (arr, id)
    }

    #[test]
    fn same_file_registers_one_source() {
        let mut arr = ProjectArrangement::empty(1);
        let a = arr.add_audio_source(source(10)).unwrap();
        let b = arr.add_audio_source(source(10)).unwrap();
        let c = arr.add_audio_source(source(11)).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(arr.audio_sources.len(), 2);
    }

    #[test]
    fn occluding_the_middle_splits_and_the_tail_keeps_its_audio() {
        let (mut arr, id) = arrangement_with_region(0.0, 8.0);
        arr.occlude_audio_span(1, 2.0, 3.0, SPB).unwrap();
        let lane = arr.audio_lane(1);
        assert_eq!(lane.len(), 2);
        assert_eq!((lane[0].id, lane[0].start_beat, lane[0].end_beat), (id, 0.0, 2.0));
        assert_eq!(lane[0].source_offset, 0);
        assert_ne!(lane[1].id, id, "a split tail gets a fresh id");
        assert_eq!((lane[1].start_beat, lane[1].end_beat), (3.0, 8.0));
        assert_eq!(lane[1].source_offset, 3 * 24_000, "the tail plays what it played");
        assert_eq!(arr.next_clip_id, lane[1].id.0 + 1);
    }

    #[test]
    fn occluding_the_head_trims_and_advances_the_offset() {
        let (mut arr, id) = arrangement_with_region(1.0, 5.0);
        arr.occlude_audio_span(1, 0.0, 2.0, SPB).unwrap();
        let lane = arr.audio_lane(1);
        assert_eq!(lane.len(), 1);
        assert_eq!((lane[0].id, lane[0].start_beat, lane[0].source_offset), (id, 2.0, 24_000));
    }

    #[test]
    fn audio_lanes_are_sparse_and_found_by_id() {
        let (arr, id) = arrangement_with_region(0.0, 1.0);
        assert!(arr.audio_lane(0).is_empty());
        assert!(arr.audio_lane(5).is_empty());
        assert_eq!(arr.find_audio_region(id).map(|(track, _)| track), Some(1));
    }

    #[test]
    fn validation_rejects_unknown_sources_and_offsets_past_the_end() {
        let (mut arr, _) = arrangement_with_region(0.0, 1.0);
        let mut seen = HashSet::new();
        arr.validate_audio_lanes(2, &mut seen).unwrap();

        arr.audio_lanes[1][0].source_offset = 48_000 * 8;
        let error = arr.validate_audio_lanes(2, &mut HashSet::new()).unwrap_err();
        assert!(error.contains("source frame"), "{error}");

        arr.audio_lanes[1][0].source_offset = 0;
        arr.audio_lanes[1][0].source = AudioSourceId(99);
        let error = arr.validate_audio_lanes(2, &mut HashSet::new()).unwrap_err();
        assert!(error.contains("audio source 99"), "{error}");
    }

    #[test]
    fn round_trips_through_json_and_old_files_have_no_audio() {
        let (arr, _) = arrangement_with_region(0.0, 1.0);
        let json = serde_json::to_string(&arr).unwrap();
        let back: ProjectArrangement = serde_json::from_str(&json).unwrap();
        assert_eq!(back, arr);

        let mut legacy = serde_json::to_value(ProjectArrangement::empty(1)).unwrap();
        legacy.as_object_mut().unwrap().remove("audio_lanes");
        let legacy: ProjectArrangement = serde_json::from_value(legacy).unwrap();
        assert!(legacy.audio_lanes.is_empty() && legacy.audio_sources.is_empty());
    }
}

#[cfg(test)]
mod loop_and_pitch_tests {
    use super::*;

    fn region() -> AudioRegion {
        AudioRegion::new(ClipId(0), AudioSourceId(0), 0.0, 8.0)
    }

    #[test]
    fn an_octave_up_plays_twice_as_fast() {
        let mut r = region();
        r.transpose = 12.0;
        // 120 bpm: one beat is half a second, 24 000 frames at 48 kHz; an
        // octave up covers twice that.
        assert_eq!(r.tail_at(1.0, 0.5, 48_000).source_offset, 48_000);
    }

    #[test]
    fn a_looping_tail_folds_back_into_the_loop() {
        let mut r = region();
        r.loop_enabled = true;
        r.loop_start = 12_000;
        r.loop_end = 36_000;
        // Two beats in: 48 000 frames of straight playback = 12 000 past the
        // loop end, which folds to 12 000 + 12 000.
        assert_eq!(r.tail_at(2.0, 0.5, 48_000).source_offset, 24_000);
        assert_eq!(r.fold_into_loop(30_000.0), 30_000.0, "inside the loop nothing folds");
    }
}
