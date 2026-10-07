use super::*;

/// What reading a sampler track's playhead needs, copied out of the `App`
/// (the host kinds' `device.playhead` reads it without the `App`).
#[derive(Clone)]
pub(crate) struct SamplerPlayhead {
    lg: sequencer::audiograph::LiveGraphPtr,
    sampler_ids: Vec<i32>,
    /// The sample's path and registered key, when the track has one.
    path: Option<std::path::PathBuf>,
    sample: Option<String>,
    sample_rate: u32,
}

impl SamplerPlayhead {
    /// `None` when the track has no sampler voices.
    pub(crate) fn of(app: &app::App, track: usize) -> Option<Self> {
        let ids = app.graph.track_node_ids.get(track)?;
        if ids.sampler_ids.is_empty() {
            return None;
        }
        let path = app.sampler_path_ref_for_track(track).cloned();
        Some(Self {
            lg: app.graph.lg,
            sampler_ids: ids.sampler_ids.clone(),
            sample: path.as_ref().map(|path| path.display().to_string()),
            path,
            sample_rate: app.graph.sample_rate,
        })
    }

    /// Whether `other` reads the same voices of the same sample.
    pub(crate) fn same_voices(&self, other: &Self) -> bool {
        self.sampler_ids == other.sampler_ids
            && self.path == other.path
            && self.sample_rate == other.sample_rate
    }

    /// Whether `of(app, track)` would read the same voices of the same
    /// sample (`None` for a track with no sampler voices); allocates
    /// nothing. What keeps a cold `device.playhead` read current.
    pub(crate) fn current(this: Option<&Self>, app: &app::App, track: usize) -> bool {
        let ids = app.graph.track_node_ids.get(track);
        let voices = ids.map_or(&[][..], |ids| &ids.sampler_ids[..]);
        match this {
            None => voices.is_empty(),
            Some(this) => {
                this.sampler_ids == voices
                    && this.path.as_ref() == app.sampler_path_ref_for_track(track)
                    && this.sample_rate == app.graph.sample_rate
            }
        }
    }

    /// The newest playing voice's position in seconds; 0 when none plays.
    pub(crate) fn seconds(&self) -> f64 {
        let min_state_bytes =
            sequencer::instruments::sampler::SAMPLER_STATE_SIZE * std::mem::size_of::<f32>();
        let sample = self
            .sample
            .as_deref()
            .and_then(eseqlisp::audio::sample::get_registered_sample);

        let mut best_playhead: f64 = 0.0;
        let mut best_gate_counter: f32 = f32::INFINITY;
        let mut found_playing = false;

        for &node_id in &self.sampler_ids {
            if node_id < 0 {
                continue;
            }
            let mut state_size = 0usize;
            let mut state = [0.0_f32; sequencer::instruments::sampler::SAMPLER_STATE_SIZE];
            let copied = unsafe {
                sequencer::audiograph::get_node_state_into(
                    self.lg.0,
                    node_id,
                    state.as_mut_ptr().cast(),
                    std::mem::size_of_val(&state),
                    &mut state_size as *mut usize,
                )
            };
            if !copied || state_size < min_state_bytes {
                continue;
            }
            let playing = state[sequencer::instruments::sampler::PARAM_TRIGGER as usize] > 0.0;
            if !playing {
                continue;
            }

            let gate_counter = state[sequencer::instruments::sampler::PARAM_GATE_COUNTER as usize];
            if gate_counter >= best_gate_counter {
                continue;
            }

            let sample_frames = sample
                .as_ref()
                .map(|s| s.frames as f32)
                .filter(|frames| *frames > 0.0)
                .unwrap_or(self.sample_rate.max(1) as f32);
            best_playhead = sampler_visual_playhead_frame(&state, sample_frames) as f64;
            best_gate_counter = gate_counter;
            found_playing = true;
        }

        if !found_playing {
            return 0.0;
        }

        if best_playhead <= 0.0 {
            return 0.0;
        }

        // Convert frame index to seconds using the registered sample's metadata.
        match sample {
            Some(s) if s.frames > 0 => {
                let duration = s.duration_seconds;
                let seconds = best_playhead * duration / s.frames as f64;
                seconds.clamp(0.0, duration)
            }
            _ => {
                let sr = self.sample_rate.max(1) as f64;
                best_playhead / sr
            }
        }
    }
}

fn sampler_visual_playhead_frame(
    state: &[f32; sequencer::instruments::sampler::SAMPLER_STATE_SIZE],
    sample_frames: f32,
) -> f32 {
    let start = state[sequencer::instruments::sampler::PARAM_START_POINT as usize].clamp(0.0, 1.0);
    let end = state[sequencer::instruments::sampler::PARAM_END_POINT as usize].clamp(0.0, 1.0);
    let start_frame = start * sample_frames;
    let end_frame = if end > start {
        end * sample_frames
    } else {
        sample_frames
    };
    let region_len = (end_frame - start_frame).max(1.0);
    let playhead = state[sequencer::instruments::sampler::PARAM_PLAYHEAD as usize];
    let scrub = state[sequencer::instruments::sampler::PARAM_SCRUB_SMOOTH as usize].clamp(-1.0, 1.0);

    (playhead + scrub * region_len).clamp(start_frame, end_frame.max(start_frame))
}

#[cfg(test)]
mod tests {
    use super::sampler_visual_playhead_frame;

    #[test]
    fn sampler_visual_playhead_includes_scrub_smooth() {
        let mut state = [0.0_f32; sequencer::instruments::sampler::SAMPLER_STATE_SIZE];
        state[sequencer::instruments::sampler::PARAM_START_POINT as usize] = 0.25;
        state[sequencer::instruments::sampler::PARAM_END_POINT as usize] = 0.75;
        state[sequencer::instruments::sampler::PARAM_PLAYHEAD as usize] = 500.0;
        state[sequencer::instruments::sampler::PARAM_SCRUB_SMOOTH as usize] = 0.5;

        let frame = sampler_visual_playhead_frame(&state, 1_000.0);

        assert_eq!(frame, 750.0);
    }

    #[test]
    fn sampler_visual_playhead_matches_playhead_without_scrub() {
        let mut state = [0.0_f32; sequencer::instruments::sampler::SAMPLER_STATE_SIZE];
        state[sequencer::instruments::sampler::PARAM_START_POINT as usize] = 0.0;
        state[sequencer::instruments::sampler::PARAM_END_POINT as usize] = 1.0;
        state[sequencer::instruments::sampler::PARAM_PLAYHEAD as usize] = 500.0;

        let frame = sampler_visual_playhead_frame(&state, 1_000.0);

        assert_eq!(frame, 500.0);
    }
}

pub(crate) fn sync_watched_sampler_voices(
    app: &app::App,
    current_track: Option<usize>,
    watched_track: &mut Option<usize>,
    watched_voice_ids: &mut Vec<i32>,
) {
    let sampler_track = current_track.filter(|&track| track < app.tracks.len() && app.is_sampler_track(track));
    let desired_voice_ids =
        if let Some(current_track) = sampler_track {
            app.graph
                .track_node_ids
                .get(current_track)
                .map(|ids| ids.sampler_ids.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    if *watched_track == sampler_track && *watched_voice_ids == desired_voice_ids {
        return;
    }

    for node_id in watched_voice_ids.drain(..) {
        unsafe {
            sequencer::audiograph::remove_node_from_watchlist(app.graph.lg.0, node_id);
        }
    }

    for &node_id in &desired_voice_ids {
        if node_id >= 0 {
            unsafe {
                sequencer::audiograph::add_node_to_watchlist(app.graph.lg.0, node_id);
            }
        }
    }

    *watched_track = if desired_voice_ids.is_empty() {
        None
    } else {
        current_track
    };
    *watched_voice_ids = desired_voice_ids;
}

/// Return the waveform sample registered under the persisted sample reference.
///
/// Content-addressed references are resolved through the sample store for I/O,
/// while the registry key remains the reference carried by the project model.
/// Both standalone samplers and rack samplers must cross this boundary through
/// the same function so their UI behavior cannot diverge from playback.
pub(crate) fn load_waveform_sample(
    path: &Path,
) -> Result<std::sync::Arc<eseqlisp::audio::sample::SampleBuffer>, String> {
    let key = path.display().to_string();
    if let Some(sample) = eseqlisp::audio::sample::get_registered_sample(&key) {
        return Ok(sample);
    }

    let resolved = sequencer::app_paths::resolve_sample_ref(path);
    let mut sample = eseqlisp::audio::sample::SampleBuffer::load_wav(&resolved)?;
    sample.path = path.to_path_buf();
    Ok(sample.register())
}

/// Register a WAV file with eseqlisp's sample registry so the waveform widget can display it.
pub(crate) fn register_waveform_sample(path: &Path) {
    if let Err(error) = load_waveform_sample(path) {
        eprintln!(
            "waveform: failed to register sample {}: {error}",
            path.display()
        );
    }
}
