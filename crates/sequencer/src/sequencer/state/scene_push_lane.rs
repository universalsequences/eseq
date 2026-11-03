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
