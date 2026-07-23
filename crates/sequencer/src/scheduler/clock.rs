/*!
Deterministic snapshot clocking, swing timing, and step-delay calculations.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

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
    boundaries: [f64; MAX_STEPS + 1],
    step_ends: [f64; MAX_STEPS],
    cycle_beats: f64,
}

pub(super) struct SnapshotSequencerClock {
    pub(super) sample_rate: f64,
    pub(super) total_beats: f64,
    pub(super) track_clocks: Vec<SnapshotTrackClockState>,
    pub(super) was_playing: bool,
}

impl SnapshotSequencerClock {
    pub(super) fn new(sample_rate: u32) -> Self {
        let track_clocks = (0..MAX_TRACKS)
            .map(|_| SnapshotTrackClockState {
                last_local_step: u32::MAX,
                boundaries: [0.0; MAX_STEPS + 1],
                step_ends: [0.0; MAX_STEPS],
                cycle_beats: 4.0,
            })
            .collect();
        Self {
            sample_rate: sample_rate as f64,
            total_beats: 0.0,
            track_clocks,
            was_playing: false,
        }
    }

    pub(super) fn reset(&mut self) {
        self.total_beats = 0.0;
        self.was_playing = false;
        for track in &mut self.track_clocks {
            track.last_local_step = u32::MAX;
        }
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
        self.total_beats = (self.total_beats - ahead_samples * beats_per_sample).max(0.0);
        self.was_playing = snapshot.transport.playing;

        let num_tracks = snapshot.transport.num_tracks;
        for t in 0..num_tracks {
            self.precompute_boundaries(snapshot, t);
            let ns = snapshot.tracks[t].params.num_steps;
            let tc = &self.track_clocks[t];
            let pos_in_cycle = self.total_beats % tc.cycle_beats;
            self.track_clocks[t].last_local_step = Self::derive_local_step(tc, pos_in_cycle, ns)
                .map(|step| step as u32)
                .unwrap_or(u32::MAX);
        }
        for t in num_tracks..MAX_TRACKS {
            self.track_clocks[t].last_local_step = u32::MAX;
        }
    }

    fn precompute_boundaries(&mut self, snapshot: &SequencerSnapshot, track: usize) {
        const EPS: f64 = 1e-9;
        let track_snapshot = &snapshot.tracks[track];
        let ns = track_snapshot.params.num_steps;
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

    pub(super) fn process_chunk(
        &mut self,
        nframes: usize,
        snapshot: &SequencerSnapshot,
        state: &SequencerState,
    ) -> Vec<SnapshotTrigger> {
        if !snapshot.transport.playing {
            self.reset();
            return Vec::new();
        }

        let bpm = snapshot.transport.bpm as f64;
        let beats_per_sample = bpm / (self.sample_rate * 60.0);
        let samples_per_quarter = self.sample_rate * 60.0 / bpm;
        let num_tracks = snapshot.transport.num_tracks;

        if !self.was_playing {
            self.was_playing = true;
            self.total_beats = 0.0;
            for t in 0..MAX_TRACKS {
                self.track_clocks[t].last_local_step = u32::MAX;
            }
        }

        for t in 0..num_tracks {
            self.precompute_boundaries(snapshot, t);
        }

        let mut triggers = Vec::new();
        let mut last_global_16th = (self.total_beats / 0.25) as u32;
        let mut last_bar = (self.total_beats / 4.0) as u32;
        for offset in 0..nframes {
            self.total_beats += beats_per_sample;

            let global_16th = (self.total_beats / 0.25) as u32;
            if global_16th != last_global_16th {
                state
                    .transport
                    .playhead
                    .store(global_16th, Ordering::Relaxed);
                last_global_16th = global_16th;
            }

            let bar = (self.total_beats / 4.0) as u32;
            if bar != last_bar {
                last_bar = bar;
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
                let pos_in_cycle = self.total_beats % cycle;
                match Self::derive_local_step(tc, pos_in_cycle, ns) {
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
                                    cycle: (self.total_beats / cycle).floor().max(0.0) as u64,
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
            let position = self.total_beats % clock.cycle_beats;
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
