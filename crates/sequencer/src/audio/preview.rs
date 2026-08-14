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
    if let Ok(mut slot) = COMMAND.lock() {
        *slot = Some(Command::Play(PreviewClip {
            samples,
            sample_rate: f64::from(sample_rate.max(1)),
        }));
    }
    // Optimistic, so the play button flips before the next audio block; the
    // callback re-asserts it every block from the real voice state.
    PLAYING.store(true, Ordering::Release);
    POSITION_SECONDS_BITS.store(0f64.to_bits(), Ordering::Release);
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
                voice.pos = 0.0;
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
    let total_frames = samples.len() / 2;
    let step = voice.src_rate / device_rate;
    let nframes = output.len() / num_channels;
    for frame in 0..nframes {
        let idx = voice.pos as usize;
        if idx + 1 >= total_frames {
            voice.samples = None;
            break;
        }
        let frac = (voice.pos - idx as f64) as f32;
        let l = samples[idx * 2] + (samples[idx * 2 + 2] - samples[idx * 2]) * frac;
        let r = samples[idx * 2 + 1] + (samples[idx * 2 + 3] - samples[idx * 2 + 1]) * frac;
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
