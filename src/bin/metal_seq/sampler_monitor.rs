use super::*;

/// Read the current sampler playhead position (in seconds) for a track.
/// Scans all voices and returns the most recently triggered one (smallest
/// non-zero playhead, meaning it just started playing).
pub(crate) fn read_sampler_playhead_seconds(app: &ui::App, track: usize) -> f64 {
    let sampler_ids = match app.graph.track_node_ids.get(track) {
        Some(ids) => &ids.sampler_ids,
        None => return 0.0,
    };
    let min_state_bytes = sequencer::sampler::SAMPLER_STATE_SIZE * std::mem::size_of::<f32>();

    // Find the voice with the smallest positive playhead (most recently triggered)
    let mut best_playhead: f64 = 0.0;
    let mut best_is_playing = false;

    for &node_id in sampler_ids {
        if node_id < 0 {
            continue;
        }
        let mut state_size = 0usize;
        let mut state = [0.0_f32; sequencer::sampler::SAMPLER_STATE_SIZE];
        let copied = unsafe {
            sequencer::audiograph::get_node_state_into(
                app.graph.lg.0,
                node_id,
                state.as_mut_ptr().cast(),
                std::mem::size_of_val(&state),
                &mut state_size as *mut usize,
            )
        };
        if !copied || state_size < min_state_bytes {
            continue;
        }
        let ph = state[sequencer::sampler::PARAM_PLAYHEAD as usize] as f64;
        let playing = state[sequencer::sampler::PARAM_TRIGGER as usize] > 0.0;
        // Prefer playing voices; among those, pick the smallest playhead (most recent trigger)
        if playing && (!best_is_playing || ph < best_playhead) {
            best_playhead = ph;
            best_is_playing = true;
        } else if !best_is_playing && ph > best_playhead {
            // No playing voice found yet — pick the largest playhead (last to finish)
            best_playhead = ph;
        }
    }

    if best_playhead <= 0.0 {
        return 0.0;
    }

    // Convert frame index to seconds using the registered sample's metadata
    let sample = app
        .sampler_paths
        .get(track)
        .and_then(|p| p.as_ref())
        .and_then(|p| eseqlisp::audio::sample::get_registered_sample(&p.display().to_string()));
    match sample {
        Some(s) if s.frames > 0 => best_playhead * s.duration_seconds / s.frames as f64,
        _ => {
            let sr = app.graph.sample_rate.max(1) as f64;
            best_playhead / sr
        }
    }
}

pub(crate) fn sync_watched_sampler_voices(
    app: &ui::App,
    current_track: usize,
    watched_track: &mut Option<usize>,
    watched_voice_ids: &mut Vec<i32>,
) {
    let desired_voice_ids =
        if current_track < app.tracks.len() && app.is_sampler_track(current_track) {
            app.graph
                .track_node_ids
                .get(current_track)
                .map(|ids| ids.sampler_ids.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    if *watched_track == Some(current_track) && *watched_voice_ids == desired_voice_ids {
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
        Some(current_track)
    };
    *watched_voice_ids = desired_voice_ids;
}

/// Register a WAV file with eseqlisp's sample registry so the waveform widget can display it.
pub(crate) fn register_waveform_sample(path: &Path) {
    match eseqlisp::audio::sample::SampleBuffer::load_wav(path) {
        Ok(sample) => {
            sample.register();
        }
        Err(e) => {
            eprintln!(
                "waveform: failed to register sample {}: {e}",
                path.display()
            );
        }
    }
}
