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
    pub(super) samples_per_step: f32,
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

    /// Track-local (step, sub-step delay, step length in beats) for an
    /// absolute transport beat, from the same precomputed boundary geometry
    /// that scheduled the chunk. Used to stamp rolled hits for recording
    /// (docs/rolling-core-spec.md 6): the roll grid can be finer than the
    /// track timebase, so the remainder lands as a 0..1 step-unit delay.
    pub(super) fn roll_record_position(
        &self,
        track: usize,
        total_beats: f64,
        num_steps: usize,
    ) -> (usize, f32, f64) {
        const EPS: f64 = 1.0e-6;
        let tc = &self.track_clocks[track];
        let num_steps = num_steps.max(1);
        let pos = Self::anchored_local_beats(tc, total_beats, num_steps)
            .rem_euclid(tc.cycle_beats.max(EPS));
        let idx = tc.boundaries[..num_steps + 1].partition_point(|&b| b <= pos + EPS);
        let step = idx.saturating_sub(1).min(num_steps - 1);
        let step_dur = (tc.step_ends[step] - tc.boundaries[step]).max(EPS);
        let delay = ((pos - tc.boundaries[step]).max(0.0) / step_dur).clamp(0.0, 1.0) as f32;
        // A hit an epsilon shy of the next boundary IS that boundary.
        if delay >= 1.0 - 1.0e-4 {
            let next = (step + 1) % num_steps;
            return (next, 0.0, (tc.step_ends[next] - tc.boundaries[next]).max(EPS));
        }
        (step, delay, step_dur)
    }

    /// Swung sample time for a live roll hit (eseq-767.10): delay the audible
    /// event exactly as the step scheduler delays a sequenced step at this
    /// position — the swing bucket is keyed to the track-local beat, the same
    /// frame as the `cycle_start_beats` the lookahead feeds
    /// `swing_bucket_index`. Track-level swing only: roll hits are live
    /// events with no per-step overrides. The caller records the STRAIGHT
    /// boundary, so playback through the track swing reproduces this feel and
    /// nothing is printed.
    pub(super) fn roll_swung_sample_time(
        &self,
        snapshot: &SequencerSnapshot,
        track: usize,
        boundary_beats: f64,
        sample_time: u64,
        samples_per_quarter: f64,
    ) -> u64 {
        let Some(params) = snapshot.tracks.get(track).map(|t| &t.params) else {
            return sample_time;
        };
        if params.swing <= 50.0 {
            return sample_time;
        }
        let tc = &self.track_clocks[track];
        let local_beats = Self::anchored_local_beats(tc, boundary_beats, params.num_steps)
            .rem_euclid(tc.cycle_beats.max(1.0e-6));
        if swing_bucket_index(local_beats, params.swing_resolution) % 2 == 0 {
            return sample_time;
        }
        let swing_delay = swing_delay_samples_from_quarter(
            samples_per_quarter,
            params.swing,
            params.swing_resolution,
        )
        .round();
        sample_time.saturating_add(swing_delay.max(0.0) as u64)
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
        self.seek_beats((self.total_beats - ahead_samples * beats_per_sample).max(0.0));
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

    /// Capture one sequence-roll anchor per track from the current live clock
    /// position (docs/rolling-core-spec.md 5.1). Each track uses its own
    /// precomputed cycle, including timebase overrides and Sync padding.
    pub(super) fn capture_roll_windows(
        &mut self,
        snapshot: &SequencerSnapshot,
        grid_beats: f64,
    ) -> [Option<f64>; MAX_TRACKS] {
        let at_beats = self.total_beats;
        self.capture_roll_windows_at(snapshot, grid_beats, at_beats)
    }

    /// `capture_roll_windows` anchored at an explicit absolute transport beat
    /// instead of the frontier: a process roll (`roll!`) anchors on the step
    /// that fired it, which the lookahead frontier may already be past.
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
        // These indices describe the last evaluated sample, not the next
        // sample at total_beats. Keep them across chunk boundaries.
        for offset in 0..nframes {
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
                                    samples_per_step,
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
            // Sample zero observes beat zero. Only after evaluating this
            // sample do we advance to the next sample's musical position.
            self.tempo_frames += 1;
            self.total_beats = self.tempo_origin_beats
                + self.tempo_frames as f64 / samples_per_quarter;
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
