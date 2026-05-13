use std::sync::atomic::Ordering;

use super::data::{sync_beats, StepParam, Trigger, MAX_STEPS, MAX_TRACKS};
use super::state::SequencerState;

fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

pub struct TrackClockState {
    pub last_local_step: u32,
    pub cached_sps: f64,
    pub boundaries: [f64; MAX_STEPS + 1],
    pub step_ends: [f64; MAX_STEPS],
    pub cycle_beats: f64,
}

pub struct SequencerClock {
    sample_rate: f64,
    total_beats: f64,
    pub track_clocks: Vec<TrackClockState>,
    was_playing: bool,
}

impl SequencerClock {
    pub fn new(sample_rate: u32, _bpm: u32) -> Self {
        let track_clocks = (0..MAX_TRACKS)
            .map(|_| TrackClockState {
                last_local_step: u32::MAX,
                cached_sps: 0.0,
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

    pub fn samples_per_step_for_track(&self, track: usize) -> f64 {
        self.track_clocks[track].cached_sps
    }

    pub fn current_samples_per_step(&self) -> f64 {
        self.sample_rate * 60.0 / 120.0 / 4.0
    }

    fn precompute_boundaries(&mut self, state: &SequencerState, track: usize) {
        const EPS: f64 = 1e-9;

        let tp = &state.pattern.track_params[track];
        let ns = tp.get_num_steps();
        let default_tb = tp.get_timebase();
        let tc = &mut self.track_clocks[track];

        let mut accum = 0.0;
        let sd = &state.pattern.step_data[track];

        for s in 0..ns {
            let tb = state.pattern.timebase_plocks[track].resolve(s, default_tb);
            let step_dur = tb.step_beats(ns);

            let sync_b = sync_beats(sd.get(s, StepParam::Sync));
            if sync_b > EPS {
                accum = ceil_to_grid(accum, sync_b);
            }

            tc.boundaries[s] = accum;
            tc.step_ends[s] = accum + step_dur;
            accum += step_dur;
        }

        tc.boundaries[ns] = accum;

        let sync0_b = sync_beats(sd.get(0, StepParam::Sync));
        tc.cycle_beats = if sync0_b > EPS {
            ceil_to_grid(accum, sync0_b).max(EPS)
        } else {
            accum.max(EPS)
        };
    }

    fn derive_local_step(
        tc: &TrackClockState,
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

    pub fn process_block(&mut self, nframes: usize, state: &SequencerState) -> Vec<Trigger> {
        if !state.is_playing() {
            self.was_playing = false;
            return Vec::new();
        }

        let bpm = state.transport.bpm.load(Ordering::Relaxed) as f64;
        let beats_per_sample = bpm / (self.sample_rate * 60.0);
        let samples_per_quarter = self.sample_rate * 60.0 / bpm;
        let num_tracks = state.active_track_count();

        if !self.was_playing {
            self.was_playing = true;
            self.total_beats = 0.0;
            for t in 0..MAX_TRACKS {
                self.track_clocks[t].last_local_step = u32::MAX;
            }
        }

        for t in 0..num_tracks {
            self.precompute_boundaries(state, t);
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
                let ns = state.pattern.track_params[t].get_num_steps();
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

                            let default_tb = state.pattern.track_params[t].get_timebase();
                            let tb = state.pattern.timebase_plocks[t].resolve(step, default_tb);
                            tc.cached_sps = tb.step_beats(ns) * samples_per_quarter;

                            state.transport.track_playheads[t].store(step_u32, Ordering::Relaxed);

                            triggers.push(Trigger {
                                track: t,
                                step,
                                offset,
                                cycle_start_beats: tc.boundaries[step],
                            });
                        }
                    }
                    None => {
                        self.track_clocks[t].last_local_step = u32::MAX;
                    }
                }
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
