/*!
Deterministic snapshot clocking, swing timing, and step-delay calculations.
*/

#[allow(unused_imports)]
use super::*;

pub(super) use crate::sequencer::ceil_to_grid;

pub(super) fn snap_near_grid_down(value: f64, grid: f64, tolerance: f64) -> f64 {
    let rem = value.rem_euclid(grid);
    if rem <= tolerance {
        value - rem
    } else {
        value
    }
}

#[derive(Clone, Copy)]
pub(super) struct SnapshotTrigger {
    pub(super) track: usize,
    pub(super) step: usize,
    pub(super) offset: usize,
    pub(super) cycle: u64,
    pub(super) cycle_start_beats: f64,
    pub(super) absolute_beats: f64,
    /// The step's STRAIGHT boundary in transport beats: `absolute_beats` (the
    /// first sample at or after the boundary) minus how far past the boundary
    /// that sample sits in the track's read position. It resolves anchors,
    /// clip offsets, length re-phase and roll windows through the same
    /// projection that picked the step, so a rack groove keys every step on
    /// the transport bar exactly (docs/rack-groove-spec.md, "Beat frame").
    pub(super) boundary_beats: f64,
    pub(super) samples_per_step: f32,
    /// `Some(lag)` for a trig RECOVERED by the first chunk after a mid-play
    /// resync (rack groove spec §Early hits, eseq-groove.8): a grooved step
    /// whose straight boundary the resync already passed, `lag` samples
    /// before the chunk start (`offset` is 0), re-found so a LATE groove
    /// offset that has not sounded yet is not lost with the cleared queue.
    /// The lookahead keeps it only when its grooved sample is at or after
    /// the audio frontier; everything else about it already happened.
    pub(super) recovered_lag: Option<u64>,
}

impl SnapshotTrigger {
    /// The first sample at or after the step's straight boundary, for a
    /// chunk starting at `chunk_start_sample`.
    pub(super) fn straight_sample(&self, chunk_start_sample: u64) -> u64 {
        (chunk_start_sample + self.offset as u64).saturating_sub(self.recovered_lag.unwrap_or(0))
    }
}

pub(super) struct SnapshotTrackClockState {
    last_local_step: u32,
    /// Last substituted read position. A sequence-roll window can jump back
    /// to the start while remaining inside the same pattern step; detecting
    /// that backward edge is what retriggers one-step windows.
    last_read_position: f64,
    boundaries: [f64; MAX_STEPS + 1],
    step_ends: [f64; MAX_STEPS],
    cycle_beats: f64,
    /// Anchored clip phase (takes spec 7.1): the clock-domain beat at which
    /// the track's active lane clip starts, plus the clip's stored start
    /// offset in fractional pattern steps. The track's position in its cycle
    /// is `(total_beats - anchor_beat + offset)` instead of the historical
    /// free-running `total_beats`; the defaults (0, 0) reproduce free-run
    /// exactly, so session-mode playback is untouched. Song playback
    /// installs the current row's anchor every chunk.
    anchor_beat: f64,
    offset_steps: f64,
    /// Process-driven pattern length (`length!`, docs/default-process-lanes-spec.md
    /// length lane). `Some(n)` while the track plays `n` steps instead of its
    /// authored `num_steps`; the lookahead patches the chunk snapshot with it
    /// so every reader agrees. Pattern data is never written.
    length_override: Option<usize>,
    /// The latest `length!` request, due at the end of the cycle it fired in.
    pending_length: Option<PendingPatternLength>,
    /// Re-phase applied when a length change takes effect, so the new cycle
    /// starts on step 0 at the boundary even when its length does not divide
    /// the transport position (fixed timebases). Zero under Prh, whose cycle
    /// is always one bar.
    length_phase_beats: f64,
    /// Where the current length took effect and what it replaced, so a
    /// re-seek back across that boundary restores the previous length.
    length_applied: Option<AppliedPatternLength>,
    /// The length the last applied `length!` asked for, kept even when it
    /// equals the authored length (no override then), for the grid marker.
    length_marker: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingPatternLength {
    steps: usize,
    at_beats: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AppliedPatternLength {
    at_beats: f64,
    previous_override: Option<usize>,
    previous_phase_beats: f64,
    previous_marker: Option<usize>,
}

/// A due length change lands within this many beats of its boundary; well
/// under one sample at any tempo, so chunk-clamp rounding never defers it.
pub(super) const PATTERN_LENGTH_EPS_BEATS: f64 = 1.0e-6;

pub(super) struct SnapshotSequencerClock {
    pub(super) sample_rate: f64,
    pub(super) total_beats: f64,
    pub(super) track_clocks: Vec<SnapshotTrackClockState>,
    pub(super) was_playing: bool,
    tempo_origin_beats: f64,
    tempo_frames: u64,
    tempo_bpm: u32,
    last_global_16th: u32,
    last_bar: u32,
    /// Per-track copies of the chunk snapshot's track with `num_steps` set to
    /// the length override, keyed by the source `Arc`, so a held override
    /// costs one track clone per source publish rather than one per chunk.
    length_patch_cache: Vec<Option<LengthPatchedTrack>>,
    /// The scheduling frontier at the last mid-play resync
    /// ([`seek_to_rendered_position`](Self::seek_to_rendered_position)),
    /// zero after a transport start or any other seek. Rack groove early
    /// hits discovered before that resync already sounded; see
    /// [`groove_floor`](Self::groove_floor).
    groove_replayed_until: u64,
    /// Set by a mid-play resync, taken by the next chunk: grooved steps whose
    /// straight boundary lies within their groove's late reach before the
    /// resync point are re-found as [`SnapshotTrigger::recovered_lag`] trigs
    /// (eseq-groove.8). Any other seek clears it.
    late_recovery_pending: bool,
    /// The scheduling frontier's beat at the last mid-play resync (zero
    /// after any other seek). Graph runtimes are not rewound by a resync:
    /// they already consumed every boundary up to here, and the lookahead
    /// replays their enqueued emissions instead. A step re-found below this
    /// beat already seeded the graphs before the resync, so it does not
    /// seed them again (eseq-groove.8).
    graph_seeded_until_beats: f64,
    /// Bumped by every seek. Graph emissions the lookahead retains for a
    /// resync replay are tagged with it, so a replay never re-plays
    /// emissions from before a transport start, song wrap or other seek.
    pub(super) seek_generation: u64,
    /// The generation a mid-play resync left (the tag of the emissions it
    /// discarded with the queue), until the next lookahead replays them.
    graph_replay_generation: Option<u64>,
    /// Step trigs the lookahead scheduled at or after the audio frontier,
    /// i.e. still in the queue when a mid-play resync clears it. A trig the
    /// resync re-finds below `rendered` ([`SnapshotTrigger::recovered_lag`])
    /// is replayed only when it is here: the groove it was scheduled with
    /// put it at or after `rendered`, so it has not sounded. Without this a
    /// groove change between the hit and the resync could move a hit that
    /// already sounded straight into the late window and play it twice
    /// (eseq-groove.8). Pruned by the lookahead to what can still be queued;
    /// cleared by any seek other than a resync.
    queued_step_hits: Vec<QueuedStepHit>,
}

/// One scheduled step trig, for [`SnapshotSequencerClock::queued_step_hits`].
#[derive(Clone, Copy, Debug)]
struct QueuedStepHit {
    track: usize,
    step: usize,
    /// The first sample at or after the step's straight boundary.
    straight_sample: u64,
    /// The sample it was scheduled at (after Step Delay and the groove).
    sample: u64,
}

struct LengthPatchedTrack {
    source: Arc<crate::sequencer::SequencerTrackSnapshot>,
    steps: usize,
    patched: Arc<crate::sequencer::SequencerTrackSnapshot>,
}

impl SnapshotSequencerClock {
    pub(super) fn new(sample_rate: u32) -> Self {
        let track_clocks = (0..MAX_TRACKS)
            .map(|_| SnapshotTrackClockState {
                last_local_step: u32::MAX,
                last_read_position: f64::NAN,
                boundaries: [0.0; MAX_STEPS + 1],
                step_ends: [0.0; MAX_STEPS],
                cycle_beats: 4.0,
                anchor_beat: 0.0,
                offset_steps: 0.0,
                length_override: None,
                pending_length: None,
                length_phase_beats: 0.0,
                length_applied: None,
                length_marker: None,
            })
            .collect();
        Self {
            sample_rate: sample_rate as f64,
            total_beats: 0.0,
            track_clocks,
            was_playing: false,
            tempo_origin_beats: 0.0,
            tempo_frames: 0,
            tempo_bpm: 0,
            last_global_16th: 0,
            last_bar: 0,
            length_patch_cache: (0..MAX_TRACKS).map(|_| None).collect(),
            groove_replayed_until: 0,
            late_recovery_pending: false,
            graph_seeded_until_beats: 0.0,
            seek_generation: 0,
            graph_replay_generation: None,
            queued_step_hits: Vec::new(),
        }
    }

    /// Remember a step trig the lookahead scheduled at `sample` (see
    /// [`queued_step_hits`](Self::queued_step_hits)).
    pub(super) fn record_queued_step_hit(
        &mut self,
        track: usize,
        step: usize,
        straight_sample: u64,
        sample: u64,
    ) {
        self.queued_step_hits.push(QueuedStepHit {
            track,
            step,
            straight_sample,
            sample,
        });
    }

    /// Forget the scheduled step trigs the audio has reached: they sounded.
    pub(super) fn forget_sounded_step_hits(&mut self, rendered: u64) {
        self.queued_step_hits.retain(|hit| hit.sample >= rendered);
    }

    /// Whether the step trig of `track`/`step` whose straight boundary is at
    /// `straight_sample` was scheduled at or after `rendered`, so it was
    /// still queued when a resync cleared the queue. The resync re-derives
    /// the boundary from beats, so one sample of slack absorbs float noise;
    /// two passes of the same step are a whole cycle apart.
    pub(super) fn step_hit_was_queued(
        &self,
        track: usize,
        step: usize,
        straight_sample: u64,
        rendered: u64,
    ) -> bool {
        self.queued_step_hits.iter().any(|hit| {
            hit.track == track
                && hit.step == step
                && hit.straight_sample.abs_diff(straight_sample) <= 1
                && hit.sample >= rendered
        })
    }

    /// Whether a step at `seed_beats` should seed the graph runtimes: false
    /// for a step the clock re-found below the last mid-play resync's old
    /// frontier, which seeded them before the resync (the graphs are not
    /// rewound; eseq-groove.8). Half a sample of slack absorbs the resync's
    /// floating-point beat re-derivation, so the step AT the old frontier
    /// (not yet found then) still seeds.
    pub(super) fn graph_seed_is_new(&self, seed_beats: f64, samples_per_quarter: f64) -> bool {
        let slack = if samples_per_quarter > 0.0 {
            0.5 / samples_per_quarter
        } else {
            0.0
        };
        seed_beats >= self.graph_seeded_until_beats - slack
    }

    /// Whether a mid-play resync left graph emissions for the lookahead to
    /// replay (see [`Self::take_graph_replay_generation`]).
    pub(super) fn graph_replay_pending(&self) -> bool {
        self.graph_replay_generation.is_some()
    }

    /// The generation whose retained graph emissions the last mid-play
    /// resync discarded with the queue, once: the lookahead replays them
    /// (eseq-groove.8).
    pub(super) fn take_graph_replay_generation(&mut self) -> Option<u64> {
        self.graph_replay_generation.take()
    }

    /// The rack groove floor for a lookahead call extending from the audio
    /// frontier `rendered` (docs/rack-groove-spec.md §Early hits): early
    /// hits clamp at `rendered`, except those already played before the last
    /// mid-play resync, which are dropped.
    pub(super) fn groove_floor(&self, rendered: u64) -> crate::groove::GrooveFloor {
        crate::groove::GrooveFloor {
            not_before: rendered,
            replayed_until: self.groove_replayed_until,
        }
    }

    pub(super) fn reset(&mut self) {
        self.seek_beats(0.0);
        self.was_playing = false;
        for track in &mut self.track_clocks {
            track.last_local_step = u32::MAX;
            track.last_read_position = f64::NAN;
            track.anchor_beat = 0.0;
            track.offset_steps = 0.0;
        }
        self.clear_pattern_lengths();
    }

    /// Establish a new musical clock origin after a seek or tempo change.
    /// Between origins, time advances by integer samples rather than adding a
    /// rounded beat increment on every sample.
    pub(super) fn seek_beats(&mut self, beat: f64) {
        self.total_beats = beat;
        self.tempo_origin_beats = beat;
        self.tempo_frames = 0;
        self.last_global_16th = (beat / 0.25) as u32;
        self.last_bar = (beat / 4.0) as u32;
        self.groove_replayed_until = 0;
        self.late_recovery_pending = false;
        self.graph_seeded_until_beats = 0.0;
        self.seek_generation = self.seek_generation.wrapping_add(1);
        self.graph_replay_generation = None;
        self.queued_step_hits.clear();
    }

    /// Install the active song row's per-lane phase anchors (takes spec
    /// 7.3): every track's clip starts at `anchor_beat` (the row start in
    /// this clock's beat domain) with its lane's stored step offset. Called
    /// once per planned song chunk; cleared by `reset` and
    /// `clear_track_anchors` so session-mode playback keeps free-running.
    pub(super) fn set_song_row_anchors(&mut self, anchor_beat: f64, lane_offsets: &[f64]) {
        for (track, clock) in self.track_clocks.iter_mut().enumerate() {
            let offset_steps = lane_offsets.get(track).copied().unwrap_or(0.0);
            if clock.anchor_beat != anchor_beat || clock.offset_steps != offset_steps {
                // A new clip anchor restarts the lane's phase; a length
                // override re-phased against the old anchor would be wrong.
                Self::clear_track_length(clock);
            }
            clock.anchor_beat = anchor_beat;
            clock.offset_steps = offset_steps;
        }
    }

    pub(super) fn clear_track_anchors(&mut self) {
        for clock in &mut self.track_clocks {
            if clock.anchor_beat != 0.0 || clock.offset_steps != 0.0 {
                Self::clear_track_length(clock);
            }
            clock.anchor_beat = 0.0;
            clock.offset_steps = 0.0;
        }
    }

    /// Adopt a changed source after installing its clip anchor. A fractional
    /// offset can enter an already sounding step; that is not a new onset.
    /// Still allow an onset whose first representable sample is this frame,
    /// even when the outgoing source last played the same step index.
    pub(super) fn adopt_track_source(&mut self, track: usize, snapshot: &SequencerSnapshot) {
        // A new source is a new pattern: its authored length governs.
        self.clear_pattern_length(track);
        self.precompute_boundaries(snapshot, track);
        let ns = snapshot.tracks[track].params.num_steps;
        let clock = &mut self.track_clocks[track];
        let local_beats = Self::anchored_local_beats(clock, self.total_beats, ns);
        let position = local_beats.rem_euclid(clock.cycle_beats);
        clock.last_local_step = Self::derive_local_step(clock, position, ns)
            .filter(|step| {
                let onset_beat = clock.anchor_beat - Self::offset_beats(clock, ns)
                    + (local_beats / clock.cycle_beats).floor() * clock.cycle_beats
                    + clock.boundaries[*step];
                let onset_frame = ((onset_beat - self.tempo_origin_beats)
                    * self.sample_rate * 60.0 / snapshot.transport.bpm as f64).ceil();
                onset_frame < self.tempo_frames as f64
            })
            .map(|step| step as u32)
            .unwrap_or(u32::MAX);
        clock.last_read_position = f64::NAN;
    }

    /// Clear one track's anchor (manual-override latch, takes spec 10): the
    /// latched track free-runs against the clock like session playback while
    /// every other lane keeps its song-row anchor.
    pub(super) fn clear_track_anchor(&mut self, track: usize) {
        if let Some(clock) = self.track_clocks.get_mut(track) {
            if clock.anchor_beat != 0.0 || clock.offset_steps != 0.0 {
                Self::clear_track_length(clock);
            }
            clock.anchor_beat = 0.0;
            clock.offset_steps = 0.0;
        }
    }

    /// The track's clip-local beat position (takes spec 7.1):
    /// `steps(beat - start_beat) + offset`, expressed in cycle beats. The
    /// stored step offset converts to beats through the precomputed
    /// boundaries so per-step timebase overrides resolve consistently.
    fn anchored_local_beats(
        tc: &SnapshotTrackClockState,
        total_beats: f64,
        num_steps: usize,
    ) -> f64 {
        total_beats - tc.anchor_beat + Self::offset_beats(tc, num_steps) - tc.length_phase_beats
    }

    /// Track-local (step, sub-step delay, step length in beats) to RECORD a
    /// roll hit heard at `heard_beats` (absolute transport beats), from the
    /// same precomputed boundary geometry that scheduled the chunk. Used to
    /// stamp rolled hits for recording (docs/rolling-core-spec.md 6): the
    /// roll grid can be finer than the track timebase, so the remainder
    /// lands as a 0..1 step-unit delay.
    ///
    /// The position is unwound through the feel playback applies to the
    /// step it lands on (rack groove pocket, or track swing with per-step
    /// overrides; docs/rack-groove-spec.md §Sites 5, eseq-groove.6): the
    /// stored phase is the straight one that playback moves back onto
    /// `heard_beats`, so the feel is never applied twice. A roll hit on a
    /// straight grid line heard through the feel of the step it starts
    /// records that step's straight boundary with no delay.
    pub(super) fn roll_record_position(
        &self,
        snapshot: &SequencerSnapshot,
        track: usize,
        heard_beats: f64,
        num_steps: usize,
    ) -> (usize, f32, f64) {
        const EPS: f64 = 1.0e-6;
        let tc = &self.track_clocks[track];
        let num_steps = num_steps.max(1);
        let cycle = tc.cycle_beats.max(EPS);
        let pos = Self::anchored_local_beats(tc, heard_beats, num_steps).rem_euclid(cycle);
        let (step, delay) =
            match self.unwind_roll_feel(snapshot, track, heard_beats, pos, num_steps) {
                Some(unwound) => (unwound.step, unwound.phase.clamp(0.0, 1.0) as f32),
                None => {
                    let idx = tc.boundaries[..num_steps + 1].partition_point(|&b| b <= pos + EPS);
                    let step = idx.saturating_sub(1).min(num_steps - 1);
                    let step_dur = (tc.step_ends[step] - tc.boundaries[step]).max(EPS);
                    let delay =
                        ((pos - tc.boundaries[step]).max(0.0) / step_dur).clamp(0.0, 1.0) as f32;
                    (step, delay)
                }
            };
        // A hit an epsilon shy of the next boundary IS that boundary.
        if delay >= 1.0 - 1.0e-4 {
            let next = (step + 1) % num_steps;
            return (next, 0.0, (tc.step_ends[next] - tc.boundaries[next]).max(EPS));
        }
        (
            step,
            delay,
            (tc.step_ends[step] - tc.boundaries[step]).max(EPS),
        )
    }

    /// The feel-unwound (step, phase) for a roll hit heard at local cycle
    /// position `pos`, or `None` for a track that plays straight.
    fn unwind_roll_feel(
        &self,
        snapshot: &SequencerSnapshot,
        track: usize,
        heard_beats: f64,
        pos: f64,
        num_steps: usize,
    ) -> Option<crate::groove::UnwoundPosition> {
        let tc = &self.track_clocks[track];
        let boundaries = &tc.boundaries[..num_steps];
        let step_ends = &tc.step_ends[..num_steps];
        let cycle = tc.cycle_beats.max(1.0e-6);
        if let Some(groove) = snapshot.track_groove(track) {
            // Transport beat of the heard cycle's start: the frame the
            // scheduler's `boundary_beats` (and so the groove) is keyed in.
            let cycle_start = heard_beats - pos;
            return crate::groove::unwind_step_feel(
                pos,
                cycle,
                boundaries,
                step_ends,
                |step, base| groove.pocket_offset_beats(cycle_start + base + boundaries[step]),
            );
        }
        let track_snapshot = snapshot.tracks.get(track)?;
        let params = &track_snapshot.params;
        let swings: Vec<f64> = boundaries
            .iter()
            .enumerate()
            .map(|(step, &boundary)| {
                let step_snapshot = track_snapshot.steps.get(step);
                crate::groove::swing_shift_beats(
                    step_snapshot
                        .and_then(|s| s.swing_override)
                        .unwrap_or(params.swing),
                    step_snapshot
                        .and_then(|s| s.swing_resolution_override)
                        .unwrap_or(params.swing_resolution),
                    boundary,
                )
            })
            .collect();
        if swings.iter().all(|&shift| shift == 0.0) {
            return None;
        }
        crate::groove::unwind_step_feel(pos, cycle, boundaries, step_ends, |step, _| swings[step])
    }

    /// Where a live roll hit on the straight roll-grid line
    /// `boundary_beats` is HEARD, in transport beats, minus any groove
    /// Random (the recorder unwinds the pocket, not the jitter): the
    /// straight line plus the member's groove pocket at that line, or,
    /// with no groove, the swing [`Self::roll_swung_sample_time`] adds.
    pub(super) fn roll_heard_beats(
        &self,
        snapshot: &SequencerSnapshot,
        track: usize,
        boundary_beats: f64,
    ) -> f64 {
        if let Some(groove) = snapshot.track_groove(track) {
            return boundary_beats + groove.pocket_offset_beats(boundary_beats);
        }
        let Some(params) = snapshot.tracks.get(track).map(|t| &t.params) else {
            return boundary_beats;
        };
        let tc = &self.track_clocks[track];
        let local_beats = Self::anchored_local_beats(tc, boundary_beats, params.num_steps)
            .rem_euclid(tc.cycle_beats.max(1.0e-6));
        boundary_beats
            + crate::groove::swing_shift_beats(params.swing, params.swing_resolution, local_beats)
    }

    /// Sample time for a live roll hit (eseq-767.10): delay the audible
    /// event exactly as the step scheduler delays a sequenced step at this
    /// position.
    ///
    /// A rack member with a groove plays the hit through it, keyed on the
    /// hit's straight transport beat like every other trig source (rack
    /// groove spec §Sites 5): the groove REPLACES the swing, an early offset
    /// never lands before the audio frontier, and `None` drops an early hit
    /// that already sounded before a mid-play resync ([`GrooveFloor`]).
    ///
    /// Otherwise it is the track swing: the swing bucket is keyed to the
    /// track-local beat, the same frame as the `cycle_start_beats` the
    /// lookahead feeds `swing_bucket_index`. Track-level swing only: roll
    /// hits are live events with no per-step overrides.
    ///
    /// Either way the caller records the position unwound through the feel
    /// ([`Self::roll_record_position`]), so playback reproduces what was
    /// heard and the feel is not printed.
    ///
    /// [`GrooveFloor`]: crate::groove::GrooveFloor
    pub(super) fn roll_swung_sample_time(
        &self,
        snapshot: &SequencerSnapshot,
        track: usize,
        boundary_beats: f64,
        sample_time: u64,
        samples_per_quarter: f64,
        groove_floor: crate::groove::GrooveFloor,
    ) -> Option<u64> {
        if let Some(groove) = snapshot.track_groove(track) {
            return crate::groove::grooved_sample_time(
                groove,
                boundary_beats,
                sample_time,
                samples_per_quarter,
                groove_floor,
            );
        }
        let Some(params) = snapshot.tracks.get(track).map(|t| &t.params) else {
            return Some(sample_time);
        };
        if params.swing <= 50.0 {
            return Some(sample_time);
        }
        let tc = &self.track_clocks[track];
        let local_beats = Self::anchored_local_beats(tc, boundary_beats, params.num_steps)
            .rem_euclid(tc.cycle_beats.max(1.0e-6));
        if swing_bucket_index(local_beats, params.swing_resolution) % 2 == 0 {
            return Some(sample_time);
        }
        let swing_delay = swing_delay_samples_from_quarter(
            samples_per_quarter,
            params.swing,
            params.swing_resolution,
        )
        .round();
        Some(sample_time.saturating_add(swing_delay.max(0.0) as u64))
    }

    fn offset_beats(tc: &SnapshotTrackClockState, num_steps: usize) -> f64 {
        if tc.offset_steps == 0.0 || num_steps == 0 {
            return 0.0;
        }
        // Pattern offsets resolve modulo the pattern length (takes spec 6.3).
        // Fractional positions interpolate across the whole inter-boundary
        // span (sync waits and the padded cycle tail included), so the
        // mapping is the exact inverse of `PatternStepGeometry` stamping —
        // an offset stamped mid-wait resolves to that beat, not to a point
        // inside the step's sounding span. For gapless patterns the span
        // equals the step duration and this is unchanged.
        let steps = tc.offset_steps.rem_euclid(num_steps as f64);
        let step = (steps.floor() as usize).min(num_steps - 1);
        let frac = steps - step as f64;
        let span_end = if step + 1 < num_steps {
            tc.boundaries[step + 1]
        } else {
            tc.cycle_beats
        };
        tc.boundaries[step] + frac * (span_end - tc.boundaries[step])
    }

    pub(super) fn seek_to_rendered_position(
        &mut self,
        snapshot: &SequencerSnapshot,
        rendered_sample: u64,
        scheduled_until_sample: u64,
    ) {
        let bpm = snapshot.transport.bpm as f64;
        let beats_per_sample = bpm / (self.sample_rate * 60.0);
        let ahead_samples = scheduled_until_sample.saturating_sub(rendered_sample) as f64;
        let previous_replayed_until = self.groove_replayed_until;
        let previous_seeded_until = self.graph_seeded_until_beats;
        let previous_frontier_beats = self.total_beats;
        let previous_replay_generation =
            self.graph_replay_generation.unwrap_or(self.seek_generation);
        // What the cleared queue held survives the seek: the next chunk
        // replays a recovered late hit only if it was still queued.
        let queued_step_hits = std::mem::take(&mut self.queued_step_hits);
        self.seek_beats((self.total_beats - ahead_samples * beats_per_sample).max(0.0));
        self.queued_step_hits = queued_step_hits;
        // Late groove hits (eseq-groove.8): a grooved step whose straight
        // boundary is before `rendered_sample` but whose grooved sample is
        // not was queued and just cleared; the rewound clock would only find
        // boundaries from here on, so the next chunk re-finds it.
        self.late_recovery_pending = true;
        // Graph runtimes already ran to the old frontier; the lookahead
        // replays what they emitted (tagged with the pre-seek generation,
        // or the first resync's when two land before a lookahead runs), and
        // steps re-found below the old frontier do not seed them twice.
        self.graph_seeded_until_beats = previous_frontier_beats.max(previous_seeded_until);
        self.graph_replay_generation = Some(previous_replay_generation);
        // Everything with a straight boundary below the old frontier was
        // already discovered (and enqueued); the queue was just cleared, so
        // the clock will find those boundaries again from `rendered_sample`.
        // An early groove hit among them that sounded before `rendered` must
        // not be clamped to `rendered` and played twice. Back-to-back
        // resyncs (the second one sees the frontier already at `rendered`)
        // keep the first one's window.
        self.groove_replayed_until = scheduled_until_sample.max(previous_replayed_until);
        self.was_playing = snapshot.transport.playing;

        let num_tracks = snapshot.transport.num_tracks;
        for t in 0..num_tracks {
            self.rewind_pattern_length(t, snapshot.tracks[t].params.num_steps);
            self.precompute_boundaries(snapshot, t);
            let ns = self.effective_num_steps(snapshot, t);
            let tc = &self.track_clocks[t];
            let pos_in_cycle =
                Self::anchored_local_beats(tc, self.total_beats, ns).rem_euclid(tc.cycle_beats);
            self.track_clocks[t].last_local_step = Self::derive_local_step(tc, pos_in_cycle, ns)
                .map(|step| step as u32)
                .unwrap_or(u32::MAX);
            self.track_clocks[t].last_read_position = f64::NAN;
        }
        for t in num_tracks..MAX_TRACKS {
            self.track_clocks[t].last_local_step = u32::MAX;
        }
    }

    fn precompute_boundaries(&mut self, snapshot: &SequencerSnapshot, track: usize) {
        let ns = self.effective_num_steps(snapshot, track);
        self.precompute_boundaries_for(&snapshot.tracks[track], ns, track);
    }

    /// Boundaries for `ns` steps of `track_snapshot`. `ns` may exceed the
    /// authored length under a `length!` override; snapshots carry every
    /// step up to `MAX_STEPS`.
    fn precompute_boundaries_for(
        &mut self,
        track_snapshot: &crate::sequencer::SequencerTrackSnapshot,
        ns: usize,
        track: usize,
    ) {
        const EPS: f64 = 1e-9;
        let default_tb = track_snapshot.params.timebase;
        let tc = &mut self.track_clocks[track];

        let mut accum = 0.0;
        for s in 0..ns {
            let tb = track_snapshot.steps[s]
                .timebase_override
                .unwrap_or(default_tb);
            let step_dur = tb.step_beats(ns);

            let sync_b = sync_beats(track_snapshot.steps[s].params[StepParam::Sync.index()]);
            if sync_b > EPS {
                accum = ceil_to_grid(accum, sync_b);
            }

            tc.boundaries[s] = accum;
            tc.step_ends[s] = accum + step_dur;
            accum += step_dur;
        }
        tc.boundaries[ns] = accum;

        let sync0_b = sync_beats(track_snapshot.steps[0].params[StepParam::Sync.index()]);
        tc.cycle_beats = if sync0_b > EPS {
            ceil_to_grid(accum, sync0_b).max(EPS)
        } else {
            accum.max(EPS)
        };
    }

    fn derive_local_step(
        tc: &SnapshotTrackClockState,
        pos_in_cycle: f64,
        num_steps: usize,
    ) -> Option<usize> {
        if pos_in_cycle >= tc.boundaries[num_steps] {
            return None;
        }
        let idx = tc.boundaries[..num_steps + 1].partition_point(|&b| b <= pos_in_cycle);
        let s = if idx > 0 { idx - 1 } else { 0 };
        if pos_in_cycle < tc.step_ends[s] {
            Some(s)
        } else {
            None
        }
    }

    /// The step `track` is on at absolute transport `beats`, through the same
    /// anchored projection and boundary geometry that fires its steps, so an
    /// emitted hit (generator, process, graph, cross-track neural fire)
    /// resolves the p-locks of the step it actually lands on. `tolerance`
    /// absorbs sample rounding: a hit a hair before a boundary lands on the
    /// step that boundary starts. `None` past the track's last step (Sync
    /// padding) or on a track with no cycle. Sequence-roll windows are not
    /// applied: the live position is the step grid a hit lands on.
    pub(super) fn track_step_at_beats(
        &self,
        snapshot: &SequencerSnapshot,
        track: usize,
        beats: f64,
        tolerance: f64,
    ) -> Option<usize> {
        if track >= snapshot.transport.num_tracks {
            return None;
        }
        let tc = self.track_clocks.get(track)?;
        let num_steps = snapshot.tracks.get(track)?.params.num_steps;
        if num_steps == 0 || tc.cycle_beats <= 0.0 {
            return None;
        }
        let position = Self::anchored_local_beats(tc, beats + tolerance.max(0.0), num_steps)
            .rem_euclid(tc.cycle_beats);
        Self::derive_local_step(tc, position, num_steps)
    }

    /// Capture one sequence-roll anchor per track at an absolute transport
    /// beat (docs/rolling-core-spec.md 5.1). Each track uses its own
    /// precomputed cycle, including timebase overrides and Sync padding. The
    /// beat is never the bare frontier: a manual sequence roll anchors on the
    /// audible position (the frontier less its lead over the render head),
    /// and a process roll (`roll!`) on the step that fired it, which the
    /// lookahead frontier may already be past.
    pub(super) fn capture_roll_windows_at(
        &mut self,
        snapshot: &SequencerSnapshot,
        grid_beats: f64,
        at_beats: f64,
    ) -> [Option<f64>; MAX_TRACKS] {
        const EPS: f64 = 1.0e-9;
        let mut windows = [None; MAX_TRACKS];
        if grid_beats <= EPS {
            return windows;
        }
        for track in 0..snapshot.transport.num_tracks.min(MAX_TRACKS) {
            self.precompute_boundaries(snapshot, track);
            let num_steps = snapshot.tracks[track].params.num_steps;
            let tc = &self.track_clocks[track];
            let cycle = tc.cycle_beats;
            if cycle <= EPS {
                continue;
            }
            let live = Self::anchored_local_beats(tc, at_beats, num_steps).rem_euclid(cycle);
            windows[track] = Some(Self::snap_roll_window(live, grid_beats, cycle));
        }
        windows
    }

    fn snap_roll_window(position: f64, grid_beats: f64, cycle_beats: f64) -> f64 {
        const SIXTEENTH: f64 = 0.25;
        const EPS: f64 = 1.0e-9;
        let down = position - position.rem_euclid(grid_beats);
        let wrapped = down.rem_euclid(cycle_beats);
        let sixteenth_phase = wrapped.rem_euclid(SIXTEENTH);
        if sixteenth_phase <= EPS || SIXTEENTH - sixteenth_phase <= EPS {
            wrapped
        } else {
            (wrapped / SIXTEENTH).ceil() * SIXTEENTH
        }
    }

    pub(super) fn roll_read_position(
        live_pos_in_cycle: f64,
        window_start: Option<f64>,
        grid_beats: f64,
        cycle_beats: f64,
    ) -> f64 {
        match window_start {
            Some(start) if grid_beats > 1.0e-9 => {
                (start + live_pos_in_cycle.rem_euclid(grid_beats)).rem_euclid(cycle_beats)
            }
            _ => live_pos_in_cycle,
        }
    }

    /// How many samples after the one just evaluated at `beats` provably
    /// evaluate the same way: no track's derived step changes (step start,
    /// step end, cycle wrap) and neither the global 16th nor the bar does.
    /// A three-sample margin keeps float rounding in the edge distances from
    /// ever skipping an edge sample.
    fn quiet_frames_after(
        &self,
        snapshot: &SequencerSnapshot,
        num_tracks: usize,
        beats: f64,
        samples_per_quarter: f64,
    ) -> usize {
        if !(beats >= 0.0) {
            return 0;
        }
        let mut headroom = ((beats / 0.25).floor() + 1.0) * 0.25 - beats;
        headroom = headroom.min(((beats / 4.0).floor() + 1.0) * 4.0 - beats);
        for t in 0..num_tracks {
            let tc = &self.track_clocks[t];
            let cycle = tc.cycle_beats;
            if cycle <= 0.0 {
                continue;
            }
            let ns = snapshot.tracks[t].params.num_steps;
            let pos = Self::anchored_local_beats(tc, beats, ns).rem_euclid(cycle);
            let mut edge = cycle;
            if pos < tc.boundaries[ns] {
                let idx = tc.boundaries[..ns + 1].partition_point(|&b| b <= pos);
                if idx <= ns {
                    edge = edge.min(tc.boundaries[idx]);
                }
                let step = idx.saturating_sub(1);
                if pos < tc.step_ends[step] {
                    edge = edge.min(tc.step_ends[step]);
                }
            }
            headroom = headroom.min(edge - pos);
        }
        let frames = (headroom * samples_per_quarter).floor() - 3.0;
        if frames.is_finite() && frames > 0.0 {
            frames as usize
        } else {
            0
        }
    }

    pub(super) fn process_chunk(
        &mut self,
        nframes: usize,
        snapshot: &SequencerSnapshot,
        state: &SequencerState,
    ) -> Vec<SnapshotTrigger> {
        self.process_chunk_with_roll(nframes, snapshot, state, None, 0.0)
    }

    pub(super) fn process_chunk_with_roll(
        &mut self,
        nframes: usize,
        snapshot: &SequencerSnapshot,
        state: &SequencerState,
        mut window_start: Option<&mut [Option<f64>; MAX_TRACKS]>,
        grid_beats: f64,
    ) -> Vec<SnapshotTrigger> {
        if !snapshot.transport.playing {
            self.reset();
            return Vec::new();
        }

        let bpm = snapshot.transport.bpm as f64;
        let samples_per_quarter = self.sample_rate * 60.0 / bpm;
        let num_tracks = snapshot.transport.num_tracks;

        if !self.was_playing {
            self.was_playing = true;
            self.seek_beats(0.0);
            for t in 0..MAX_TRACKS {
                self.track_clocks[t].last_local_step = u32::MAX;
                self.track_clocks[t].last_read_position = f64::NAN;
            }
        }
        if self.tempo_bpm != snapshot.transport.bpm {
            self.tempo_origin_beats = self.total_beats;
            self.tempo_frames = 0;
            self.tempo_bpm = snapshot.transport.bpm;
        }

        for t in 0..num_tracks {
            self.precompute_boundaries(snapshot, t);
            if let Some(start) = window_start.as_deref_mut().and_then(|starts| starts[t]) {
                let cycle = self.track_clocks[t].cycle_beats;
                // Per-chunk idempotent correction (§5.3): enforce both the
                // current roll grid and F6's never-off-1/16 anchor rule, then
                // let the read remap wrap against the current pattern cycle.
                window_start.as_deref_mut().unwrap()[t] = Some(Self::snap_roll_window(
                    start.rem_euclid(cycle),
                    grid_beats,
                    cycle,
                ));
            }
        }

        let mut triggers = Vec::new();
        if std::mem::take(&mut self.late_recovery_pending) {
            self.recover_late_groove_triggers(
                snapshot,
                window_start.as_deref(),
                samples_per_quarter,
                &mut triggers,
            );
        }
        // Under a sequence-roll window reads remap per sample, so every
        // sample is evaluated; otherwise quiet stretches are skipped below.
        let may_skip = !window_start
            .as_deref()
            .is_some_and(|starts| starts.iter().take(num_tracks).any(Option::is_some));
        // These indices describe the last evaluated sample, not the next
        // sample at total_beats. Keep them across chunk boundaries.
        let mut offset = 0;
        while offset < nframes {
            let global_16th = (self.total_beats / 0.25) as u32;
            if offset == 0 || global_16th != self.last_global_16th {
                state
                    .transport
                    .playhead
                    .store(global_16th, Ordering::Relaxed);
                self.last_global_16th = global_16th;
            }

            let bar = (self.total_beats / 4.0) as u32;
            if bar != self.last_bar {
                self.last_bar = bar;
                if state
                    .transport
                    .pending_mod_resync
                    .swap(false, Ordering::Relaxed)
                {
                    state
                        .transport
                        .mod_reset_counter
                        .fetch_add(1, Ordering::Relaxed);
                }
            }

            for t in 0..num_tracks {
                let track = &snapshot.tracks[t];
                let ns = track.params.num_steps;
                let tc = &self.track_clocks[t];
                let cycle = tc.cycle_beats;
                if cycle <= 0.0 {
                    continue;
                }
                // Anchored per-lane projection (takes spec 7.1): the clip's
                // anchor and offset replace the free-running global clock;
                // defaults make this `total_beats % cycle` exactly.
                let local_beats = Self::anchored_local_beats(tc, self.total_beats, ns);
                let live_pos_in_cycle = local_beats.rem_euclid(cycle);
                let rolling_window = window_start.as_deref().and_then(|starts| starts[t]);
                let pos_in_cycle = Self::roll_read_position(
                    live_pos_in_cycle,
                    rolling_window,
                    grid_beats,
                    cycle,
                );
                let derived_step = Self::derive_local_step(tc, pos_in_cycle, ns);
                if rolling_window.is_some()
                    && pos_in_cycle + 1.0e-9 < self.track_clocks[t].last_read_position
                {
                    // A window wrap is a real replay boundary even when its
                    // first and last positions derive to the same pattern
                    // step (the common 1/16-window case).
                    self.track_clocks[t].last_local_step = u32::MAX;
                }
                self.track_clocks[t].last_read_position = pos_in_cycle;
                match derived_step {
                    Some(step) => {
                        let step_u32 = step as u32;
                        if step_u32 != self.track_clocks[t].last_local_step {
                            let tc = &mut self.track_clocks[t];
                            tc.last_local_step = step_u32;
                            let tb = track.steps[step]
                                .timebase_override
                                .unwrap_or(track.params.timebase);
                            let samples_per_step = (tb.step_beats(ns) * samples_per_quarter) as f32;
                            if !track.scene_silenced {
                                triggers.push(SnapshotTrigger {
                                    track: t,
                                    step,
                                    offset,
                                    // Cycle count is clip-local so step
                                    // processes see the clip's own
                                    // repetition index, not the global one.
                                    cycle: (local_beats / cycle).floor().max(0.0) as u64,
                                    cycle_start_beats: tc.boundaries[step],
                                    absolute_beats: self.total_beats,
                                    boundary_beats: self.total_beats
                                        - (pos_in_cycle - tc.boundaries[step]).max(0.0),
                                    samples_per_step,
                                    recovered_lag: None,
                                });
                            }
                            state.transport.track_playheads[t].store(step_u32, Ordering::Relaxed);
                        }
                    }
                    None => {
                        self.track_clocks[t].last_local_step = u32::MAX;
                    }
                }
            }
            // Samples that cannot reach a step, 16th or bar edge evaluate
            // exactly like this one (no trigger, no store changes value), so
            // jump over them. Positions derive from the frame count, never
            // accumulate, so a jump lands on the same beats as stepping. The
            // chunk's last sample is always evaluated: it leaves the
            // per-track read positions a later roll window compares against.
            let quiet = if may_skip && offset + 2 < nframes {
                self.quiet_frames_after(snapshot, num_tracks, self.total_beats, samples_per_quarter)
                    .min(nframes - offset - 2)
            } else {
                0
            };
            // Sample zero observes beat zero. Only after evaluating this
            // sample do we advance to the next sample's musical position.
            self.tempo_frames += 1 + quiet as u64;
            self.total_beats = self.tempo_origin_beats
                + self.tempo_frames as f64 / samples_per_quarter;
            offset += 1 + quiet;
        }

        // Publish the local phase every scheduler block, not only on a step
        // transition. The UI record path needs a phase in the track's own
        // timebase (including per-step overrides), not the global 16th phase.
        for t in 0..num_tracks {
            let track = &snapshot.tracks[t];
            let num_steps = track.params.num_steps;
            let clock = &self.track_clocks[t];
            if clock.cycle_beats <= 0.0 {
                continue;
            }
            let live_position = Self::anchored_local_beats(clock, self.total_beats, num_steps)
                .rem_euclid(clock.cycle_beats);
            let position = Self::roll_read_position(
                live_position,
                window_start.as_deref().and_then(|starts| starts[t]),
                grid_beats,
                clock.cycle_beats,
            );
            if let Some(step) = Self::derive_local_step(clock, position, num_steps) {
                let step_beats = (clock.step_ends[step] - clock.boundaries[step]).max(1.0e-9);
                let phase = ((position - clock.boundaries[step]) / step_beats).clamp(0.0, 1.0);
                state.transport.track_playheads[t].store(step as u32, Ordering::Relaxed);
                state.transport.track_playhead_phases[t]
                    .store((phase as f32).to_bits(), Ordering::Relaxed);
            }
        }

        let phase_16th = (self.total_beats / 0.25).fract() as f32;
        state
            .transport
            .playhead_phase
            .store(phase_16th.to_bits(), Ordering::Relaxed);

        triggers
    }
}

impl SnapshotSequencerClock {
    /// The first chunk after a mid-play resync (eseq-groove.8): re-find
    /// every ACTIVE step of a grooved track whose straight boundary lies
    /// within that track's groove late reach ([`max_late_beats`]) before the
    /// resync point, at or before it. The resync marked the step under the
    /// playhead as already triggered and never looks further back, so these
    /// are exactly the boundaries it passed. The lookahead keeps one only
    /// when its grooved sample has not sounded yet (the mirror of the early
    /// drop, [`GrooveFloor::replayed_until`]).
    ///
    /// Skipped for a scene-silenced track, a track under a sequence-roll
    /// window (the roll remaps its reads) and boundaries before the
    /// transport start or the lane's clip anchor. Recovered trigs come
    /// first, earliest first, each with `offset` 0 and its `recovered_lag`.
    ///
    /// [`max_late_beats`]: crate::groove::TrackGrooveSnapshot::max_late_beats
    /// [`GrooveFloor::replayed_until`]: crate::groove::GrooveFloor::replayed_until
    fn recover_late_groove_triggers(
        &self,
        snapshot: &SequencerSnapshot,
        window_start: Option<&[Option<f64>; MAX_TRACKS]>,
        samples_per_quarter: f64,
        out: &mut Vec<SnapshotTrigger>,
    ) {
        const EPS: f64 = 1.0e-9;
        let now = self.total_beats;
        let first = out.len();
        for t in 0..snapshot.transport.num_tracks.min(snapshot.tracks.len()) {
            let Some(groove) = snapshot.track_groove(t) else {
                continue;
            };
            let reach = groove.max_late_beats();
            let track = &snapshot.tracks[t];
            if !(reach > 0.0)
                || track.scene_silenced
                || window_start.is_some_and(|starts| starts[t].is_some())
            {
                continue;
            }
            let ns = track.params.num_steps;
            let tc = &self.track_clocks[t];
            let cycle = tc.cycle_beats;
            if ns == 0 || cycle <= EPS {
                continue;
            }
            let local = Self::anchored_local_beats(tc, now, ns);
            let position = local.rem_euclid(cycle);
            let mut cycle_base = local - position;
            let mut step = tc.boundaries[..ns + 1]
                .partition_point(|&b| b <= position)
                .saturating_sub(1)
                .min(ns - 1);
            let earliest = (now - reach).max(tc.anchor_beat).max(0.0);
            // Walk back one step at a time; a cycle has at most `ns` steps,
            // so the reach (under one cycle for any real groove) is bounded.
            for _ in 0..(2 * MAX_STEPS) {
                let boundary_local = cycle_base + tc.boundaries[step];
                let boundary = now - (local - boundary_local);
                if boundary + EPS < earliest || boundary > now + EPS {
                    break;
                }
                // The first sample at or after the boundary, `lag` samples
                // back; the epsilon keeps a boundary exactly on a sample
                // (float noise in the resync's re-derived beat) on it.
                let lag = ((now - boundary) * samples_per_quarter + 1.0e-6)
                    .floor()
                    .max(0.0);
                if track.steps[step].active && lag.is_finite() {
                    let lag = lag as u64;
                    let tb = track.steps[step]
                        .timebase_override
                        .unwrap_or(track.params.timebase);
                    out.push(SnapshotTrigger {
                        track: t,
                        step,
                        offset: 0,
                        cycle: (boundary_local / cycle).floor().max(0.0) as u64,
                        cycle_start_beats: tc.boundaries[step],
                        absolute_beats: now - lag as f64 / samples_per_quarter,
                        boundary_beats: boundary,
                        samples_per_step: (tb.step_beats(ns) * samples_per_quarter) as f32,
                        recovered_lag: Some(lag),
                    });
                }
                if step == 0 {
                    step = ns - 1;
                    cycle_base -= cycle;
                } else {
                    step -= 1;
                }
            }
        }
        out[first..].sort_by(|a, b| {
            b.recovered_lag
                .cmp(&a.recovered_lag)
                .then(a.track.cmp(&b.track))
        });
    }
}

/// Process-driven pattern length (`length!`). The override lives on the
/// clock because it is timing state: it changes the cycle, and the re-phase
/// that starts the new cycle on step 0 is a term of the track's position.
impl SnapshotSequencerClock {
    /// The step count the track plays: its `length!` override, else the
    /// authored length.
    pub(super) fn effective_num_steps(&self, snapshot: &SequencerSnapshot, track: usize) -> usize {
        self.track_clocks[track]
            .length_override
            .unwrap_or(snapshot.tracks[track].params.num_steps)
    }

    /// The step count the track's length lane last set, for the UI marker.
    pub(super) fn pattern_length_marker(&self, track: usize) -> Option<usize> {
        self.track_clocks[track].length_marker
    }

    #[cfg(test)]
    pub(super) fn pattern_length_override(&self, track: usize) -> Option<usize> {
        self.track_clocks[track].length_override
    }

    fn clear_track_length(clock: &mut SnapshotTrackClockState) {
        clock.length_override = None;
        clock.pending_length = None;
        clock.length_phase_beats = 0.0;
        clock.length_applied = None;
        clock.length_marker = None;
    }

    /// Drop one track's override and any pending request: its authored
    /// length governs again from the current position.
    pub(super) fn clear_pattern_length(&mut self, track: usize) {
        if let Some(clock) = self.track_clocks.get_mut(track) {
            Self::clear_track_length(clock);
        }
        if let Some(entry) = self.length_patch_cache.get_mut(track) {
            *entry = None;
        }
    }

    pub(super) fn clear_pattern_lengths(&mut self) {
        for track in 0..self.track_clocks.len() {
            self.clear_pattern_length(track);
        }
    }

    /// Record a `length!` request from a step that fired at `fired_beats`.
    /// It takes effect at the end of the cycle that step belongs to; when that
    /// boundary is already behind the frontier (the last step fired in the
    /// same chunk its cycle ended), at the end of the following cycle. The
    /// last request before a boundary wins.
    pub(super) fn request_pattern_length(
        &mut self,
        snapshot: &SequencerSnapshot,
        track: usize,
        steps: usize,
        fired_beats: f64,
    ) {
        let steps = steps.clamp(1, MAX_STEPS);
        let total_beats = self.total_beats;
        if track >= snapshot.tracks.len() {
            return;
        }
        let ns = self.effective_num_steps(snapshot, track);
        let Some(tc) = self.track_clocks.get_mut(track) else {
            return;
        };
        let cycle = tc.cycle_beats;
        if cycle <= PATTERN_LENGTH_EPS_BEATS {
            return;
        }
        let local = Self::anchored_local_beats(tc, fired_beats, ns);
        let mut at_beats = fired_beats - local.rem_euclid(cycle) + cycle;
        while at_beats <= total_beats + PATTERN_LENGTH_EPS_BEATS {
            at_beats += cycle;
        }
        tc.pending_length = Some(PendingPatternLength { steps, at_beats });
    }

    /// Earliest pending length boundary ahead of the frontier. The lookahead
    /// clamps chunks to it so a change always lands on a chunk start.
    pub(super) fn next_pattern_length_boundary(&self, num_tracks: usize) -> Option<f64> {
        self.track_clocks
            .iter()
            .take(num_tracks)
            .filter_map(|clock| clock.pending_length.map(|pending| pending.at_beats))
            .reduce(f64::min)
    }

    /// Apply every pending length whose boundary the frontier has reached.
    /// `snapshot` is the chunk's authored snapshot (before length patching).
    pub(super) fn apply_due_pattern_lengths(&mut self, snapshot: &SequencerSnapshot) {
        let num_tracks = snapshot.transport.num_tracks.min(snapshot.tracks.len());
        for track in 0..num_tracks {
            let Some(pending) = self.track_clocks[track].pending_length else {
                continue;
            };
            if pending.at_beats > self.total_beats + PATTERN_LENGTH_EPS_BEATS {
                continue;
            }
            self.track_clocks[track].pending_length = None;
            let authored = snapshot.tracks[track].params.num_steps;
            let next = (pending.steps != authored).then_some(pending.steps);
            if next == self.track_clocks[track].length_override {
                self.track_clocks[track].length_marker = Some(pending.steps);
                continue;
            }
            let tc = &mut self.track_clocks[track];
            tc.length_applied = Some(AppliedPatternLength {
                at_beats: pending.at_beats,
                previous_override: tc.length_override,
                previous_phase_beats: tc.length_phase_beats,
                previous_marker: tc.length_marker,
            });
            tc.length_marker = Some(pending.steps);
            tc.length_override = next;
            tc.length_phase_beats = 0.0;
            self.precompute_boundaries_for(&snapshot.tracks[track], pending.steps, track);
            // Re-phase so the boundary is position 0 of the new cycle.
            let tc = &mut self.track_clocks[track];
            let local = Self::anchored_local_beats(tc, pending.at_beats, pending.steps);
            tc.length_phase_beats = local.rem_euclid(tc.cycle_beats);
            // The boundary sample has not been evaluated yet (the chunk was
            // clamped to it): fire step 0 even if the old cycle's last step
            // had the same index (a one-step pattern).
            tc.last_local_step = u32::MAX;
            tc.last_read_position = f64::NAN;
        }
    }

    /// A re-seek that lands before the boundary where the current length took
    /// effect restores the previous one and re-arms the change at that
    /// boundary, so the replayed tail of the old cycle keeps its geometry.
    fn rewind_pattern_length(&mut self, track: usize, authored_steps: usize) {
        let total_beats = self.total_beats;
        let tc = &mut self.track_clocks[track];
        let Some(applied) = tc.length_applied else {
            return;
        };
        if total_beats + PATTERN_LENGTH_EPS_BEATS >= applied.at_beats {
            return;
        }
        let current = tc.length_override.unwrap_or(authored_steps);
        let steps = tc.pending_length.map_or(current, |pending| pending.steps);
        tc.length_override = applied.previous_override;
        tc.length_phase_beats = applied.previous_phase_beats;
        tc.length_marker = applied.previous_marker;
        tc.length_applied = None;
        tc.pending_length = Some(PendingPatternLength {
            steps,
            at_beats: applied.at_beats,
        });
    }

    /// The chunk snapshot with each overridden track's `num_steps` replaced,
    /// or `None` when no track holds an override. Patched tracks are cached
    /// per source `Arc`.
    pub(super) fn patch_pattern_lengths(
        &mut self,
        snapshot: &SequencerSnapshot,
    ) -> Option<Arc<SequencerSnapshot>> {
        let num_tracks = snapshot.tracks.len().min(MAX_TRACKS);
        if !self.track_clocks[..num_tracks]
            .iter()
            .any(|clock| clock.length_override.is_some())
        {
            return None;
        }
        let mut patched = snapshot.clone();
        for track in 0..num_tracks {
            let Some(steps) = self.track_clocks[track].length_override else {
                continue;
            };
            let source = &snapshot.tracks[track];
            let cached = self.length_patch_cache[track].as_ref().filter(|entry| {
                entry.steps == steps && Arc::ptr_eq(&entry.source, source)
            });
            let track_snapshot = match cached {
                Some(entry) => Arc::clone(&entry.patched),
                None => {
                    let mut copy = (**source).clone();
                    copy.params.num_steps = steps;
                    let copy = Arc::new(copy);
                    self.length_patch_cache[track] = Some(LengthPatchedTrack {
                        source: Arc::clone(source),
                        steps,
                        patched: Arc::clone(&copy),
                    });
                    copy
                }
            };
            patched.tracks[track] = track_snapshot;
        }
        Some(Arc::new(patched))
    }
}

pub(super) fn swing_bucket_index(cycle_start_beats: f64, resolution: SwingResolution) -> u64 {
    const EPS: f64 = 1e-9;
    ((cycle_start_beats + EPS) / resolution.step_beats()).floor() as u64
}

pub(super) fn swing_delay_samples(
    sample_rate: f64,
    bpm: f64,
    swing_pct: f32,
    resolution: SwingResolution,
) -> f64 {
    let samples_per_quarter = sample_rate * 60.0 / bpm;
    swing_delay_samples_from_quarter(samples_per_quarter, swing_pct, resolution)
}

pub(super) fn swing_delay_samples_from_quarter(
    samples_per_quarter: f64,
    swing_pct: f32,
    resolution: SwingResolution,
) -> f64 {
    let resolution_samples = resolution.step_beats() * samples_per_quarter;
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution_samples
}

/// A step trig's sample time: the straight boundary plus its step Delay
/// (chord steps apply their per-note delays later), plus the feel.
///
/// The feel is the member's rack groove when it has one (keyed on the
/// trig's straight transport boundary), which REPLACES the track swing and
/// any per-step swing override (docs/rack-groove-spec.md §Sites 1). Otherwise
/// it is today's swing, bit for bit. An early groove offset never lands
/// before the audio frontier, and `None` drops an early hit that already
/// sounded before a mid-play resync (§Early hits, [`GrooveFloor`]).
///
/// [`GrooveFloor`]: crate::groove::GrooveFloor
pub(super) fn step_trigger_sample_time(
    snapshot: &SequencerSnapshot,
    trigger: &SnapshotTrigger,
    step_boundary_sample_time: u64,
    sample_rate: u32,
    samples_per_quarter: f64,
    groove_floor: crate::groove::GrooveFloor,
) -> Option<u64> {
    let track = &snapshot.tracks[trigger.track];
    let step_snapshot = &track.steps[trigger.step];
    let mut sample_time = if step_snapshot.chord.is_empty() {
        delayed_step_sample_time(
            step_boundary_sample_time,
            &step_snapshot.params,
            trigger.samples_per_step,
        )
    } else {
        step_boundary_sample_time
    };
    if let Some(groove) = snapshot.track_groove(trigger.track) {
        return crate::groove::grooved_sample_time(
            groove,
            trigger.boundary_beats,
            sample_time,
            samples_per_quarter,
            groove_floor,
        );
    }
    let swing_pct = step_snapshot.swing_override.unwrap_or(track.params.swing);
    let swing_resolution = step_snapshot
        .swing_resolution_override
        .unwrap_or(track.params.swing_resolution);
    let swing_step = swing_bucket_index(trigger.cycle_start_beats, swing_resolution);
    let is_odd_step = swing_step % 2 == 1;
    if is_odd_step && swing_pct > 50.0 {
        let swing_delay = swing_delay_samples(
            sample_rate as f64,
            snapshot.transport.bpm as f64,
            swing_pct,
            swing_resolution,
        )
        .round();
        sample_time = sample_time.saturating_add(swing_delay.max(0.0) as u64);
    }
    Some(sample_time)
}

/// An emitted trig (graph, generator or process emission) aimed at `track`,
/// moved by the track's rack groove when it has one. `boundary_beats` is the
/// emission's straight transport beat; an early offset never lands before
/// the audio frontier, and `None` drops an early hit that already sounded
/// before a mid-play resync. No groove: `sample_time` unchanged.
pub(super) fn grooved_emission_sample_time(
    snapshot: &SequencerSnapshot,
    track: Option<usize>,
    sample_time: u64,
    boundary_beats: f64,
    samples_per_quarter: f64,
    groove_floor: crate::groove::GrooveFloor,
) -> Option<u64> {
    match track.and_then(|track| snapshot.track_groove(track)) {
        Some(groove) => crate::groove::grooved_sample_time(
            groove,
            boundary_beats,
            sample_time,
            samples_per_quarter,
            groove_floor,
        ),
        None => Some(sample_time),
    }
}

/// A trig's resolved `velocity` through `track`'s rack groove accent
/// (docs/rack-groove-spec.md §Application: "Velocity multiplies the source's
/// resolved velocity and is clamped to its valid range"), keyed on the same
/// straight transport beat as its timing. No groove, or a zero velocity
/// amount: `velocity` unchanged, bit for bit.
pub(super) fn grooved_velocity(
    snapshot: &SequencerSnapshot,
    track: Option<usize>,
    velocity: f32,
    boundary_beats: f64,
) -> f32 {
    match track.and_then(|track| snapshot.track_groove(track)) {
        Some(groove) => groove.apply_velocity(velocity, boundary_beats),
        None => velocity,
    }
}

/// Legacy neural outputs: the target member's rack groove when it has one
/// (replacing the track swing, never earlier than the audio frontier;
/// `None` = already played before a mid-play resync), else the track swing
/// exactly as before.
pub(super) fn grooved_or_swung_network_sample_time(
    snapshot: &SequencerSnapshot,
    event: &StepEvent,
    sample_time: u64,
    event_beats: f64,
    samples_per_quarter: f64,
    groove_floor: crate::groove::GrooveFloor,
) -> Option<u64> {
    match snapshot.track_groove(event.track) {
        Some(groove) => crate::groove::grooved_sample_time(
            groove,
            event_beats,
            sample_time,
            samples_per_quarter,
            groove_floor,
        ),
        None => Some(swung_network_sample_time(
            snapshot,
            event,
            sample_time,
            event_beats,
            samples_per_quarter,
        )),
    }
}

pub(super) fn swung_network_sample_time(
    snapshot: &SequencerSnapshot,
    event: &StepEvent,
    sample_time: u64,
    event_beats: f64,
    samples_per_quarter: f64,
) -> u64 {
    let Some(track) = snapshot.tracks.get(event.track) else {
        return sample_time;
    };
    let swing_pct = track.params.swing;
    if swing_pct <= 50.0 {
        return sample_time;
    }
    let swing_step = swing_bucket_index(event_beats, track.params.swing_resolution);
    if swing_step % 2 == 0 {
        return sample_time;
    }
    let swing_delay = swing_delay_samples_from_quarter(
        samples_per_quarter,
        swing_pct,
        track.params.swing_resolution,
    )
    .round();
    sample_time.saturating_add(swing_delay.max(0.0) as u64)
}

pub(super) fn step_delay_samples(step_params: &[f32], samples_per_step: f32) -> u64 {
    let delay = step_params
        .get(StepParam::Delay.index())
        .copied()
        .unwrap_or_else(|| StepParam::Delay.default_value())
        .clamp(StepParam::Delay.min(), StepParam::Delay.max());
    (delay as f64 * samples_per_step.max(0.0) as f64).round() as u64
}
