//! Writing a take: a thread that follows an [`InputRing`] from a chosen
//! frame, writes the selected channels to a float WAV in the audio store
//! and hands the finished source back (docs/audio-tracks-spec.md §12.1,
//! §12.7).
//!
//! The ring keeps seconds of history, so the audio thread takes no part in
//! recording: the writer starts at the frame the take began on even when it
//! is started late, and nothing is dropped unless this thread stalls for
//! the ring's whole length (then the gap is written as silence and counted,
//! so the audio after it stays in time). The take is held in memory (it is
//! what will play) and the file is written when it stops, trimmed to the
//! frame the stop names: a crash mid-take loses the take.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::ring::InputRing;

/// How long a stopped take waits for the device to deliver its last frames.
const STOP_GRACE: Duration = Duration::from_millis(400);
const POLL: Duration = Duration::from_millis(5);
/// The live preview keeps each channel's highest and lowest sample of every
/// this many frames, as two "frames" at [`preview_rate`]: enough to draw
/// the take's waveform while it records, at a fiftieth of the audio.
const PREVIEW_BUCKET_FRAMES: u64 = 100;

/// The sample rate a take's preview is registered at.
pub fn preview_rate(sample_rate: u32) -> u32 {
    (sample_rate / (PREVIEW_BUCKET_FRAMES as u32 / 2)).max(1)
}

/// A finished recording, already in the audio store.
pub struct FinishedTake {
    /// Store file name, `<sha256>.wav`.
    pub file: String,
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u16,
    pub frames: u64,
    /// The ring frame the audio starts on (the frame asked for, unless the
    /// ring had already lost it).
    pub first_frame: u64,
    /// The audio as interleaved stereo, ready to play.
    pub stereo: Vec<f32>,
    pub peak: f32,
    /// Frames written as silence because the writer fell a whole ring
    /// behind.
    pub lost_frames: u64,
}

struct Control {
    /// The ring frame to stop before; `u64::MAX` while recording.
    stop_frame: AtomicU64,
    cancel: AtomicBool,
    /// The take so far as interleaved stereo peaks (see
    /// [`PREVIEW_BUCKET_FRAMES`]), following the device with no holdback.
    preview: Mutex<Vec<f32>>,
}

/// The control thread's handle on a running take.
pub struct TakeHandle {
    control: Arc<Control>,
}

impl TakeHandle {
    /// Finish the take at ring frame `frame` (exclusive).
    pub fn stop_at(&self, frame: u64) {
        self.control.stop_frame.store(frame, Ordering::Release);
    }

    /// Throw the take away.
    pub fn cancel(&self) {
        self.control.cancel.store(true, Ordering::Release);
    }

    /// The take's waveform so far when it has grown past `known` samples:
    /// interleaved stereo at [`preview_rate`], for drawing.
    pub fn preview_past(&self, known: usize) -> Option<Vec<f32>> {
        let preview = self.control.preview.lock().unwrap();
        (preview.len() > known).then(|| preview.clone())
    }
}

/// Start writing `ring`'s channel `left` (and `right`, for a stereo take)
/// from ring frame `first_frame` into `store`. `done` gets the result,
/// tagged `tag`, when the take stops; a cancelled take sends nothing.
pub fn spawn_take<T: Send + 'static>(
    ring: Arc<InputRing>,
    left: usize,
    right: Option<usize>,
    first_frame: u64,
    store: PathBuf,
    tag: T,
    done: Sender<(T, Result<FinishedTake, String>)>,
) -> TakeHandle {
    let control = Arc::new(Control {
        stop_frame: AtomicU64::new(u64::MAX),
        cancel: AtomicBool::new(false),
        preview: Mutex::new(Vec::new()),
    });
    let thread_control = Arc::clone(&control);
    let spawned = std::thread::Builder::new().name("audio-take".to_string()).spawn(move || {
        let result = write_take(&ring, left, right, first_frame, &store, &thread_control);
        if let Some(result) = result {
            let _ = done.send((tag, result));
        }
    });
    if let Err(error) = spawned {
        eprintln!("audio-take: could not start the writer: {error}");
    }
    TakeHandle { control }
}

/// `None`: cancelled.
fn write_take(
    ring: &InputRing,
    left: usize,
    right: Option<usize>,
    first_frame: u64,
    store: &Path,
    control: &Control,
) -> Option<Result<FinishedTake, String>> {
    let channels: u16 = if right.is_some() { 2 } else { 1 };
    // A take can start before the oldest frame the ring still holds (the
    // writer was started very late): it starts there instead.
    let first_frame = first_frame.max(ring.oldest());
    let mut cursor = first_frame;
    // The take, as it will play: the file is written from it at the end,
    // once the stop frame says exactly where it ends.
    let mut stereo: Vec<f32> = Vec::new();
    let mut lost = 0u64;
    let mut stopped_at: Option<Instant> = None;
    let mut previewed = first_frame;
    let mut fresh: Vec<f32> = Vec::new();
    let stop = loop {
        if control.cancel.load(Ordering::Acquire) {
            return None;
        }
        let stop = control.stop_frame.load(Ordering::Acquire);
        let oldest = ring.oldest();
        if cursor < oldest {
            // Fell a whole ring behind: keep time with silence.
            let gap = (oldest - cursor).min(stop.saturating_sub(cursor));
            stereo.resize(stereo.len() + gap as usize * 2, 0.0);
            lost += gap;
            cursor += gap;
        }
        let end = ring.written().min(stop);
        while cursor < end {
            let l = ring.sample(cursor, left);
            let r = right.map_or(l, |right| ring.sample(cursor, right));
            stereo.extend([l, r]);
            cursor += 1;
        }
        // The live waveform, in whole buckets.
        while previewed + PREVIEW_BUCKET_FRAMES <= cursor {
            let from = (previewed - first_frame) as usize * 2;
            let bucket = &stereo[from..from + PREVIEW_BUCKET_FRAMES as usize * 2];
            let (mut high, mut low) = ([f32::MIN; 2], [f32::MAX; 2]);
            for frame in bucket.chunks_exact(2) {
                high = [high[0].max(frame[0]), high[1].max(frame[1])];
                low = [low[0].min(frame[0]), low[1].min(frame[1])];
            }
            fresh.extend([high[0], high[1], low[0], low[1]]);
            previewed += PREVIEW_BUCKET_FRAMES;
        }
        if !fresh.is_empty() {
            control.preview.lock().unwrap().append(&mut fresh);
        }
        if stop != u64::MAX {
            let since = *stopped_at.get_or_insert_with(Instant::now);
            // Done at the stop frame, or once the device has had its
            // chance to deliver it.
            if cursor >= stop || since.elapsed() > STOP_GRACE {
                break stop;
            }
        }
        std::thread::sleep(POLL);
    };

    // The stop can name a frame the take had already passed.
    let frames = cursor.min(stop.max(first_frame)) - first_frame;
    stereo.truncate(frames as usize * 2);
    if frames == 0 {
        return Some(Err("Nothing was recorded".to_string()));
    }
    let peak = crate::instruments::sample_core::peak(&stereo);
    let stored = store_wav(store, &stereo, channels, ring.sample_rate());
    let file = match stored {
        Ok(file) => file,
        Err(error) => return Some(Err(error)),
    };
    Some(Ok(FinishedTake {
        file,
        sample_rate: ring.sample_rate(),
        channels,
        frames,
        first_frame,
        stereo,
        peak,
        lost_frames: lost,
    }))
}

/// Write interleaved-stereo `stereo` as a float WAV of `channels` channels
/// (1: its left) into `store`, and return its file name.
fn store_wav(store: &Path, stereo: &[f32], channels: u16, sample_rate: u32) -> Result<String, String> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut bytes = std::io::Cursor::new(Vec::with_capacity(stereo.len() * 4 + 64));
    let encoded = (|| -> Result<(), hound::Error> {
        let mut writer = hound::WavWriter::new(&mut bytes, spec)?;
        for frame in stereo.chunks_exact(2) {
            writer.write_sample(frame[0])?;
            if channels == 2 {
                writer.write_sample(frame[1])?;
            }
        }
        writer.finalize()
    })();
    encoded.map_err(|error| format!("Failed to encode the take: {error}"))?;
    store_audio_bytes(store, &bytes.into_inner())
}

/// Put a WAV file's `bytes` in the content-addressed audio store `store`
/// (spec §9.4) and return its file name, `<sha256>.wav`. The same bytes
/// stored twice, even at once from two threads, are one file.
pub fn store_audio_bytes(store: &Path, bytes: &[u8]) -> Result<String, String> {
    let file = format!("{:x}.wav", Sha256::digest(bytes));
    let path = store.join(&file);
    if path.exists() {
        return Ok(file);
    }
    // Write beside, then rename, so a crash never leaves a truncated file
    // under a content hash.
    let partial = store.join(format!("{file}.{:?}.partial", std::thread::current().id()));
    let stored = std::fs::create_dir_all(store)
        .and_then(|()| std::fs::write(&partial, bytes))
        .and_then(|()| std::fs::rename(&partial, &path));
    match stored {
        Ok(()) => Ok(file),
        Err(_) if path.exists() => {
            let _ = std::fs::remove_file(&partial);
            Ok(file)
        }
        Err(error) => Err(format!("Failed to store audio in {}: {error}", store.display())),
    }
}
