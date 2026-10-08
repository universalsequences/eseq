//! Per-track step lists, playheads and the step a p-lock display shows.

use super::*;

pub(crate) fn track_playheads_snapshot(state: &Arc<SequencerState>, app: &app::App) -> Vec<u32> {
    (0..app.tracks.len())
        .map(|t| state.transport.track_playheads[t].load(Ordering::Relaxed))
        .collect()
}

/// The step a length lane (`length!`) last set the track's length to, as a
/// 0-based index, while the transport plays; `None` when no lane drives it.
pub(crate) fn track_process_length_step(state: &Arc<SequencerState>, track: usize) -> Option<usize> {
    if !state.transport.playing.load(Ordering::Relaxed) {
        return None;
    }
    let steps = state.transport.track_process_lengths.get(track)?.load(Ordering::Relaxed) as usize;
    (steps > 0).then(|| steps.min(MAX_STEPS) - 1)
}

pub(crate) fn track_active_playhead_step(state: &Arc<SequencerState>, track: usize) -> usize {
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .max(1)
        .min(MAX_STEPS);
    let playhead = state.transport.track_playheads[track].load(Ordering::Relaxed) as usize;
    playhead.min(num_steps.saturating_sub(1))
}

pub(crate) fn selected_plock_step(selected_steps: &Arc<Mutex<HashSet<usize>>>) -> Option<usize> {
    selected_steps.lock().unwrap().iter().copied().min()
}

pub(crate) fn displayed_plock_step(
    state: &Arc<SequencerState>,
    track: usize,
    selected_step: Option<usize>,
) -> Option<usize> {
    selected_step.or_else(|| {
        state
            .transport
            .playing
            .load(Ordering::Relaxed)
            .then(|| track_active_playhead_step(state, track))
    })
}
