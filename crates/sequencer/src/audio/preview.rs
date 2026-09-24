/*!
Standalone sample preview for the browser.

The UI thread posts play/stop commands into a process-global slot; the audio
callback drains it once per block (`try_lock`, so a contended lock only delays
the command one block) and mixes the clip straight into the interleaved output
after recorder capture — the same seam as the metronome — so previews never
reach a track, the graph, or exported masters. Playback state flows back to
the UI through atomics (`is_playing` / `position_seconds`).
*/

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Interleaved stereo PCM at a source sample rate; resampled to the device
/// rate by linear interpolation while mixing.
pub struct PreviewClip {
    pub samples: Arc<Vec<f32>>,
    pub sample_rate: f64,
    /// Source frames played, `[start, end)`; wraps to `start` when looping.
    pub start: usize,
    pub end: usize,
    pub looping: bool,
}

enum Command {
    Play(PreviewClip),
    Stop,
}

static COMMAND: Mutex<Option<Command>> = Mutex::new(None);
static PLAYING: AtomicBool = AtomicBool::new(false);
static POSITION_SECONDS_BITS: AtomicU64 = AtomicU64::new(0);

/// UI thread: start previewing a clip (replaces any preview in flight).
pub fn play(samples: Arc<Vec<f32>>, sample_rate: u32) {
    let end = samples.len() / 2;
    play_region(samples, sample_rate, 0, end, false);
}

/// UI thread: preview source frames `[start, end)`, optionally looping.
/// `position_seconds` then reports time from the start of the whole clip.
pub fn play_region(samples: Arc<Vec<f32>>, sample_rate: u32, start: usize, end: usize, looping: bool) {
    let end = end.min(samples.len() / 2);
    let start = start.min(end);
    let sample_rate = f64::from(sample_rate.max(1));
    if let Ok(mut slot) = COMMAND.lock() {
        *slot = Some(Command::Play(PreviewClip { samples, sample_rate, start, end, looping }));
    }
    // Optimistic, so the play button flips before the next audio block; the
    // callback re-asserts it every block from the real voice state.
    PLAYING.store(true, Ordering::Release);
    POSITION_SECONDS_BITS.store((start as f64 / sample_rate).to_bits(), Ordering::Release);
}

/// UI thread: stop any preview in flight.
pub fn stop() {
    if let Ok(mut slot) = COMMAND.lock() {
        *slot = Some(Command::Stop);
    }
    PLAYING.store(false, Ordering::Release);
}

pub fn is_playing() -> bool {
    PLAYING.load(Ordering::Acquire)
}

pub fn position_seconds() -> f64 {
    f64::from_bits(POSITION_SECONDS_BITS.load(Ordering::Acquire))
}

/// RT-side voice state, owned by `AudioCallbackData`.
#[derive(Default)]
pub(super) struct PreviewVoice {
    samples: Option<Arc<Vec<f32>>>,
    src_rate: f64,
    /// Fractional source frame position.
    pos: f64,
    start: usize,
    end: usize,
    looping: bool,
}

/// Drain at most one pending command, then mix the active clip into the
/// interleaved output. Runs after `master_recorder.capture` so exports stay
/// preview-free.
pub(super) fn mix_preview(
    voice: &mut PreviewVoice,
    output: &mut [f32],
    num_channels: usize,
    device_rate: f64,
) {
    if let Ok(mut slot) = COMMAND.try_lock() {
        match slot.take() {
            Some(Command::Play(clip)) => {
                voice.samples = Some(clip.samples);
                voice.src_rate = clip.sample_rate;
                voice.pos = clip.start as f64;
                voice.start = clip.start;
                voice.end = clip.end;
                voice.looping = clip.looping;
            }
            Some(Command::Stop) => voice.samples = None,
            None => {}
        }
    }
    let Some(samples) = voice.samples.as_ref() else {
        publish(voice);
        return;
    };
    if output.is_empty() || num_channels == 0 || device_rate <= 0.0 {
        return;
    }
    let end = voice.end.min(samples.len() / 2);
    let step = voice.src_rate / device_rate;
    let nframes = output.len() / num_channels;
    for frame in 0..nframes {
        if voice.looping && voice.pos >= end as f64 && end > voice.start {
            voice.pos -= (end - voice.start) as f64;
        }
        let idx = voice.pos as usize;
        if idx >= end {
            voice.samples = None;
            break;
        }
        // A loop interpolates across its seam; a one-shot holds its last frame.
        let next = if idx + 1 < end { idx + 1 } else if voice.looping { voice.start } else { idx };
        let frac = (voice.pos - idx as f64) as f32;
        let l = samples[idx * 2] + (samples[next * 2] - samples[idx * 2]) * frac;
        let r = samples[idx * 2 + 1] + (samples[next * 2 + 1] - samples[idx * 2 + 1]) * frac;
        if num_channels > 1 {
            output[frame * num_channels] += l;
            output[frame * num_channels + 1] += r;
        } else {
            output[frame * num_channels] += (l + r) * 0.5;
        }
        voice.pos += step;
    }
    publish(voice);
}

fn publish(voice: &PreviewVoice) {
    let playing = voice.samples.is_some();
    PLAYING.store(playing, Ordering::Release);
    let seconds = if playing && voice.src_rate > 0.0 {
        voice.pos / voice.src_rate
    } else {
        0.0
    };
    POSITION_SECONDS_BITS.store(seconds.to_bits(), Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looping_region_wraps_inside_its_bounds() {
        // Frames 0..8 carry their index on both channels.
        let clip: Vec<f32> = (0..8).flat_map(|i| [i as f32, i as f32]).collect();
        play_region(Arc::new(clip), 10, 2, 5, true);
        let mut voice = PreviewVoice::default();
        let mut out = vec![0.0; 2 * 7];
        mix_preview(&mut voice, &mut out, 2, 10.0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left, vec![2.0, 3.0, 4.0, 2.0, 3.0, 4.0, 2.0]);
        assert!(is_playing());
        play_region(Arc::new(vec![0.0; 16]), 10, 0, 8, false);
        stop();
    }
}
