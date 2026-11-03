//! Recorded scene pushes on the arrangement (epic eseq-vum0).
//!
//! The transport's Shift/Cmd scene push morphs every differing parameter of
//! the playing scene toward another scene. An arrangement take records each
//! push gesture as one region on the **push lane** (drawn under the scene
//! lane): the scene it pushed toward plus the push amount over time. Like
//! audio regions, push regions live in `ProjectArrangement` beside the clip
//! lanes, so they share its validation, undo patches and the `ClipId`
//! allocator, and are never compiled into song rows: playback reads them
//! directly and replays them through the same `begin/set/end_scene_push`
//! entry points the live gesture uses (`app::scene_push_lane`).

use super::*;

/// One sample of a push curve: `value` (0 = the playing scene, 1 = the
/// target scene) at `beat` beats after the region's start.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenePushPoint {
    pub beat: f64,
    pub value: f32,
}

/// One push gesture over `[start_beat, end_beat)` toward `target_scene`.
/// The curve is linear between its points and holds its last value to the
/// region's end; before the first point it holds the first value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenePushRegion {
    /// Drawn from `ProjectArrangement::next_clip_id`.
    pub id: ClipId,
    pub start_beat: f64,
    /// Exclusive; must be `> start_beat`.
    pub end_beat: f64,
    pub target_scene: usize,
    /// Sorted by `beat`, offsets from `start_beat`, values in `0..=1`.
    pub points: Vec<ScenePushPoint>,
}

impl LaneSpan for ScenePushRegion {
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
        let length = end - self.start_beat;
        // Points past the new end can never play; keep the curve's value at
        // the cut so a later extension holds where it was trimmed.
        if self.points.iter().any(|point| point.beat > length) {
            let at_end = self.value_at_offset(length);
            self.points.retain(|point| point.beat <= length);
            self.points.push(ScenePushPoint { beat: length, value: at_end });
        }
    }
}

impl ScenePushRegion {
    /// The push amount `offset` beats after the region's start.
    pub fn value_at_offset(&self, offset: f64) -> f32 {
        let Some(first) = self.points.first() else {
            return 0.0;
        };
        if offset <= first.beat {
            return first.value;
        }
        let next = self.points.partition_point(|point| point.beat <= offset);
        let Some(after) = self.points.get(next) else {
            return self.points.last().map_or(0.0, |point| point.value);
        };
        let before = self.points[next - 1];
        let span = after.beat - before.beat;
        if span <= 0.0 {
            return after.value;
        }
        let t = ((offset - before.beat) / span) as f32;
        before.value + (after.value - before.value) * t
    }

    /// The push amount at arrangement beat `beat`.
    pub fn value_at(&self, beat: f64) -> f32 {
        self.value_at_offset(beat - self.start_beat)
    }

    /// The part of this region from arrangement beat `beat` on, its curve
    /// rebased to the new start (`None` when nothing is left).
    pub fn tail_from(&self, beat: f64) -> Option<Self> {
        if beat >= self.end_beat {
            return None;
        }
        let cut = (beat - self.start_beat).max(0.0);
        let mut points = vec![ScenePushPoint { beat: 0.0, value: self.value_at_offset(cut) }];
        points.extend(
            (self.points.iter())
                .filter(|point| point.beat > cut)
                .map(|point| ScenePushPoint { beat: point.beat - cut, value: point.value }),
        );
        Some(Self { start_beat: beat.max(self.start_beat), points, ..self.clone() })
    }

    /// This region over `[start, end)` instead, its curve staying where it
    /// was in time: a later start cuts it there, an earlier one holds the
    /// first value back to it, and the end trims or holds the same way.
    pub fn resized(&self, start: f64, end: f64) -> Self {
        let mut region = if start >= self.start_beat {
            self.tail_from(start).unwrap_or_else(|| Self {
                start_beat: start,
                end_beat: start,
                points: vec![ScenePushPoint { beat: 0.0, value: self.value_at(start) }],
                ..self.clone()
            })
        } else {
            let lead = self.start_beat - start;
            let points = (self.points.iter())
                .map(|point| ScenePushPoint { beat: point.beat + lead, value: point.value })
                .collect();
            Self { start_beat: start, points, ..self.clone() }
        };
        if end < region.end_beat {
            region.set_end_beat(end);
        } else {
            region.end_beat = end;
        }
        region
    }

    /// This region moved to start at `start_beat`, length and curve kept.
    pub fn moved_to(&self, start_beat: f64) -> Self {
        Self {
            start_beat,
            end_beat: start_beat + (self.end_beat - self.start_beat),
            ..self.clone()
        }
    }
}

/// Drop interior points that the kept curve already reproduces within
/// `tolerance` (Ramer-Douglas-Peucker), so a gesture sampled every frame
/// stores its shape, not its frame rate. Each point is measured against the
/// straight line between the points kept around it, so a slow bend made of
/// many tiny steps keeps its turning points. Endpoints are always kept.
pub fn thin_scene_push_points(points: &[ScenePushPoint], tolerance: f32) -> Vec<ScenePushPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut spans = vec![(0, points.len() - 1)];
    while let Some((first, last)) = spans.pop() {
        let (a, b) = (points[first], points[last]);
        let line = |point: &ScenePushPoint| {
            let span = b.beat - a.beat;
            if span > 0.0 {
                a.value + (b.value - a.value) * ((point.beat - a.beat) / span) as f32
            } else {
                b.value
            }
        };
        let worst = (first + 1..last)
            .map(|index| (index, (line(&points[index]) - points[index].value).abs()))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((index, error)) = worst {
            if error > tolerance {
                keep[index] = true;
                spans.push((first, index));
                spans.push((index, last));
            }
        }
    }
    (points.iter().zip(keep))
        .filter_map(|(point, kept)| kept.then_some(*point))
        .collect()
}

/// The recorded pushes the scheduler is about to play through, handed to it
/// ahead of time (on the live snapshot, `SequencerSnapshot::
/// scene_push_schedule`). A note-on stamps its device params when it is
/// SCHEDULED, up to a lookahead before it sounds, while replay drives the
/// macro engine at the RENDERED position; without this the onsets in that
/// window kept the previous values — the start of a push missed them, and
/// at its end they kept the pushed values after the push let go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScenePushSchedule {
    pub regions: Vec<ScheduledScenePush>,
}

/// One push region with the params it moves.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledScenePush {
    pub region: ScenePushRegion,
    pub params: Vec<crate::macro_engine::ScenePushParam>,
}

impl ScenePushSchedule {
    /// The region playing at `beat` and its push amount there.
    fn playing_at(&self, beat: f64) -> Option<(&ScheduledScenePush, f32)> {
        let push = self.regions.iter().find(|push| push.region.contains(beat))?;
        Some((push, push.region.value_at(beat)))
    }

    /// The macro table to stamp at song beat `beat`: `live` with every
    /// scheduled push param replaced by the schedule's value — the region
    /// playing at `beat` sets it, and outside every region it is dropped so
    /// the row's own value plays. The replay's copy in `live` may lag or
    /// lead the beat being scheduled; the schedule never does.
    pub fn fold_at(
        &self,
        live: &HashMap<crate::macro_engine::MacroParamKey, f32>,
        beat: f64,
    ) -> HashMap<crate::macro_engine::MacroParamKey, f32> {
        let mut table = live.clone();
        for push in &self.regions {
            for param in &push.params {
                table.remove(&param.key);
            }
        }
        if let Some((push, amount)) = self.playing_at(beat) {
            for param in &push.params {
                table.insert(param.key.clone(), param.at(amount));
            }
        }
        table
    }
}

impl ProjectArrangement {
    /// The push region playing at `beat`, if any (the lane is sorted and
    /// non-overlapping, so only the last region starting by `beat` can).
    pub fn scene_push_at(&self, beat: f64) -> Option<&ScenePushRegion> {
        let after = self.push_lane.partition_point(|region| region.start_beat <= beat);
        let region = self.push_lane.get(after.checked_sub(1)?)?;
        region.contains(beat).then_some(region)
    }

    /// Insert `region` keeping the lane sorted by start beat. The caller is
    /// responsible for having made room (`occlude_scene_push_span`).
    pub fn insert_scene_push_sorted(&mut self, region: ScenePushRegion) {
        insert_sorted(&mut self.push_lane, region);
    }

    /// Clear `[start, end)` of the push lane: covered regions go, straddling
    /// ones are trimmed or split (the right fragment takes a fresh id).
    pub fn occlude_scene_push_span(&mut self, start: f64, end: f64) -> Result<(), String> {
        let lane = std::mem::take(&mut self.push_lane);
        let kept = occlude_lane(
            lane,
            start,
            end,
            |region, beat| region.tail_from(beat),
            || self.allocate_clip_id(),
        );
        self.push_lane = kept?;
        Ok(())
    }

    /// The parts of the push regions inside `[start, end)`, cut at its
    /// edges and rebased to `start` (what a region copy lifts; ids are
    /// reassigned on paste).
    pub fn scene_pushes_in(&self, start: f64, end: f64) -> Vec<ScenePushRegion> {
        (self.push_lane.iter())
            .filter(|region| region.start_beat < end && region.end_beat > start)
            .map(|region| {
                let part = region.resized(region.start_beat.max(start), region.end_beat.min(end));
                part.moved_to(part.start_beat - start)
            })
            .collect()
    }

    /// Paste `pushes` (beats relative to `dest`) into `[dest, dest + len)`,
    /// replacing whatever pushes were there.
    pub fn paste_scene_pushes(
        &mut self,
        pushes: &[ScenePushRegion],
        dest: f64,
        len: f64,
    ) -> Result<(), String> {
        self.occlude_scene_push_span(dest, dest + len)?;
        for push in pushes {
            let mut pasted = push.moved_to(dest + push.start_beat);
            pasted.id = self.allocate_clip_id()?;
            self.insert_scene_push_sorted(pasted);
        }
        Ok(())
    }

    /// Open `[insert, insert + len)` on the push lane: pushes from `insert`
    /// on slide right by `len`. One straddling `insert` is split there,
    /// its tail sliding with the rest; with `hold_gap` (nothing will be
    /// pasted into the opening) its head instead holds its value at
    /// `insert` across the opening, so the push does not drop out there.
    pub fn ripple_scene_pushes_right(
        &mut self,
        insert: f64,
        len: f64,
        hold_gap: bool,
    ) -> Result<(), String> {
        let mut tails = Vec::new();
        for region in &mut self.push_lane {
            if region.start_beat >= insert {
                *region = region.moved_to(region.start_beat + len);
            } else if region.end_beat > insert {
                tails.extend(region.tail_from(insert));
                *region = region.resized(region.start_beat, insert);
                if hold_gap {
                    *region = region.resized(region.start_beat, insert + len);
                }
            }
        }
        for tail in tails {
            let mut moved = tail.moved_to(insert + len);
            moved.id = self.allocate_clip_id()?;
            if hold_gap {
                // The head now reaches the tail: join them back into one.
                if let Some(head) = self.push_lane.iter_mut().find(|region| region.end_beat == moved.start_beat && region.target_scene == moved.target_scene) {
                    let offset = moved.start_beat - head.start_beat;
                    head.points.extend(
                        moved.points.iter().map(|point| ScenePushPoint { beat: point.beat + offset, value: point.value }),
                    );
                    head.end_beat = moved.end_beat;
                    continue;
                }
            }
            self.insert_scene_push_sorted(moved);
        }
        Ok(())
    }

    /// Remove push region `id`.
    pub fn take_scene_push(&mut self, id: ClipId) -> Option<ScenePushRegion> {
        let index = self.push_lane.iter().position(|region| region.id == id)?;
        Some(self.push_lane.remove(index))
    }

    /// Spans as for every lane, each target a real scene and each curve
    /// sorted, inside its region and in `0..=1`.
    pub(super) fn validate_push_lane(
        &self,
        scene_count: usize,
        seen_ids: &mut HashSet<ClipId>,
    ) -> Result<(), String> {
        self.validate_lane_spans(&self.push_lane, "Scene push", seen_ids)?;
        for (idx, region) in self.push_lane.iter().enumerate() {
            let at = || format!("Scene push {}", idx + 1);
            if region.target_scene >= scene_count {
                return Err(format!(
                    "{} targets scene {} but the project has {} scene(s)",
                    at(),
                    region.target_scene + 1,
                    scene_count
                ));
            }
            if region.points.is_empty() {
                return Err(format!("{} has no push curve", at()));
            }
            let length = region.end_beat - region.start_beat;
            for (point_idx, point) in region.points.iter().enumerate() {
                if !point.beat.is_finite() || point.beat < 0.0 || point.beat > length {
                    return Err(format!(
                        "{} point {} at beat {} is outside the region (length {length})",
                        at(),
                        point_idx + 1,
                        point.beat
                    ));
                }
                if !(0.0..=1.0).contains(&point.value) {
                    return Err(format!(
                        "{} point {} value {} must be between 0 and 1",
                        at(),
                        point_idx + 1,
                        point.value
                    ));
                }
            }
            if region.points.windows(2).any(|pair| pair[1].beat < pair[0].beat) {
                return Err(format!("{} curve points are not sorted by beat", at()));
            }
        }
        Ok(())
    }
}

/// Keep push regions pointed at the same scene after a scene move.
pub(super) fn remap_push_lane_after_scene_move(
    arrangement: &mut ProjectArrangement,
    source: usize,
    target: usize,
) {
    for region in &mut arrangement.push_lane {
        region.target_scene = remap_scene_index_after_move(region.target_scene, source, target);
    }
}

/// A deleted scene takes the pushes toward it along; pushes toward later
/// scenes follow the index shift.
pub(super) fn remap_push_lane_after_scene_delete(
    arrangement: &mut ProjectArrangement,
    deleted_scene: usize,
) {
    arrangement.push_lane.retain(|region| region.target_scene != deleted_scene);
    for region in &mut arrangement.push_lane {
        if region.target_scene > deleted_scene {
            region.target_scene -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: f64, end: f64, points: &[(f64, f32)]) -> ScenePushRegion {
        ScenePushRegion {
            id: ClipId(0),
            start_beat: start,
            end_beat: end,
            target_scene: 1,
            points: points.iter().map(|&(beat, value)| ScenePushPoint { beat, value }).collect(),
        }
    }

    #[test]
    fn value_interpolates_between_points_and_holds_at_the_ends() {
        let push = region(4.0, 12.0, &[(1.0, 0.2), (3.0, 1.0)]);
        assert_eq!(push.value_at(4.0), 0.2, "holds the first value before it");
        assert!((push.value_at(6.0) - 0.6).abs() < 1.0e-6);
        assert_eq!(push.value_at(11.0), 1.0, "holds the last value after it");
    }

    #[test]
    fn tail_and_trim_keep_the_curve_where_it_was_cut() {
        let push = region(0.0, 8.0, &[(0.0, 0.0), (4.0, 1.0), (8.0, 0.0)]);
        let tail = push.tail_from(2.0).unwrap();
        assert_eq!(tail.start_beat, 2.0);
        assert!((tail.value_at(2.0) - 0.5).abs() < 1.0e-6);
        assert!((tail.value_at(4.0) - 1.0).abs() < 1.0e-6);
        let mut head = push.clone();
        head.set_end_beat(6.0);
        assert_eq!(head.points.last().unwrap().beat, 6.0);
        assert!((head.value_at(5.9) - push.value_at(5.9)).abs() < 1.0e-3);
    }

    #[test]
    fn thinning_keeps_shape_not_frame_rate() {
        let ramp: Vec<_> = (0..=60)
            .map(|frame| ScenePushPoint { beat: frame as f64 / 60.0, value: frame as f32 / 60.0 })
            .collect();
        let thinned = thin_scene_push_points(&ramp, 1.0e-3);
        assert_eq!(thinned.len(), 2, "a straight ramp is its two endpoints");
        let mut bend = ramp.clone();
        bend.extend((1..=60).map(|frame| ScenePushPoint {
            beat: 1.0 + frame as f64 / 60.0,
            value: 1.0 - frame as f32 / 60.0,
        }));
        assert_eq!(thin_scene_push_points(&bend, 1.0e-3).len(), 3, "up then down keeps the peak");
        // A slow bend: every frame moves less than the tolerance.
        let slow: Vec<_> = (0..=2000)
            .map(|frame| {
                let beat = frame as f64 / 1000.0;
                ScenePushPoint { beat, value: 1.0 - (beat as f32 - 1.0).abs() }
            })
            .collect();
        let thinned = thin_scene_push_points(&slow, 2.0e-3);
        assert_eq!(thinned.len(), 3, "a slow up-then-down keeps its peak: {thinned:?}");
        assert!((thinned[1].value - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn region_copy_paste_and_ripple_cut_pushes_at_the_edges() {
        let mut arrangement = ProjectArrangement::new(0, 64.0);
        let mut push = region(2.0, 10.0, &[(0.0, 0.0), (8.0, 1.0)]);
        push.id = arrangement.allocate_clip_id().unwrap();
        arrangement.push_lane = vec![push.clone()];

        let lifted = arrangement.scene_pushes_in(4.0, 8.0);
        assert_eq!(lifted.len(), 1);
        assert_eq!((lifted[0].start_beat, lifted[0].end_beat), (0.0, 4.0));
        assert!((lifted[0].value_at(0.0) - push.value_at(4.0)).abs() < 1.0e-6);
        assert!((lifted[0].value_at(3.9) - push.value_at(7.9)).abs() < 1.0e-3);

        arrangement.paste_scene_pushes(&lifted, 20.0, 4.0).unwrap();
        let spans: Vec<_> = arrangement.push_lane.iter().map(|p| (p.start_beat, p.end_beat)).collect();
        assert_eq!(spans, vec![(2.0, 10.0), (20.0, 24.0)]);

        arrangement.ripple_scene_pushes_right(6.0, 4.0, false).unwrap();
        let spans: Vec<_> = arrangement.push_lane.iter().map(|p| (p.start_beat, p.end_beat)).collect();
        assert_eq!(spans, vec![(2.0, 6.0), (10.0, 14.0), (24.0, 28.0)]);
        assert!((arrangement.push_lane[1].value_at(10.0) - push.value_at(6.0)).abs() < 1.0e-6);
        let mut ids: Vec<_> = arrangement.push_lane.iter().map(|p| p.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 3, "every part has its own id");

        // Nothing pasted into the opening: the straddler holds across it.
        let mut held = ProjectArrangement::new(0, 64.0);
        held.push_lane = vec![push.clone()];
        held.ripple_scene_pushes_right(6.0, 4.0, true).unwrap();
        let spans: Vec<_> = held.push_lane.iter().map(|p| (p.start_beat, p.end_beat)).collect();
        assert_eq!(spans, vec![(2.0, 14.0)], "one push, opened by the gap");
        let joined = &held.push_lane[0];
        assert!((joined.value_at(8.0) - push.value_at(6.0)).abs() < 1.0e-6, "holds across the gap");
        assert!((joined.value_at(12.0) - push.value_at(8.0)).abs() < 1.0e-6, "the tail kept its curve");
    }

    #[test]
    fn resizing_keeps_the_curve_in_time() {
        let push = region(4.0, 12.0, &[(0.0, 0.0), (8.0, 1.0)]);
        let cut = push.resized(6.0, 10.0);
        assert_eq!((cut.start_beat, cut.end_beat), (6.0, 10.0));
        for beat in [6.0, 8.0, 9.9] {
            assert!((cut.value_at(beat) - push.value_at(beat)).abs() < 1.0e-3, "beat {beat}");
        }
        let grown = push.resized(2.0, 16.0);
        assert_eq!((grown.start_beat, grown.end_beat), (2.0, 16.0));
        assert_eq!(grown.value_at(2.0), 0.0, "holds the first value back");
        assert!((grown.value_at(8.0) - push.value_at(8.0)).abs() < 1.0e-6);
        assert_eq!(grown.value_at(15.0), 1.0, "holds the last value on");
    }

    #[test]
    fn scene_delete_drops_its_pushes_and_shifts_later_ones() {
        let mut arrangement = ProjectArrangement::new(0, 64.0);
        let mut first = region(0.0, 4.0, &[(0.0, 1.0)]);
        first.target_scene = 1;
        let mut second = region(8.0, 12.0, &[(0.0, 1.0)]);
        second.target_scene = 3;
        arrangement.push_lane = vec![first, second];
        remap_push_lane_after_scene_delete(&mut arrangement, 1);
        assert_eq!(arrangement.push_lane.len(), 1);
        assert_eq!(arrangement.push_lane[0].target_scene, 2);
    }
}
