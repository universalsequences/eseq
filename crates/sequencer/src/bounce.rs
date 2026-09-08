//! Bounded stereo float WAV export. This sink owns no project or audio-engine
//! state: callers supply master frames from a separately owned render session.
//!
//! Publication is the commit point. Before it, every error (including dropping
//! a cancelled job) removes the sibling temporary file. An overwrite permit
//! authorizes replacement of the chosen destination path; absent that explicit
//! authorization, publication is atomic and never clobbers a destination race.

mod plan;
pub mod worker;
pub(crate) mod assets;
pub(crate) mod samples;
pub use plan::{BouncePlan, BouncePhase, BounceProgress, render_to_wav};

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const HEADER_BYTES: u64 = 56;
const FRAME_BYTES: u64 = 8;
const BUFFER_BYTES: usize = 32 * 1024;
pub const MAX_WAV_FRAMES: u64 = (u32::MAX as u64 - (HEADER_BYTES - 8)) / FRAME_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    /// Also protects files created after rendering begins.
    CreateNew,
    /// Only use after the user explicitly confirms replacing this path.
    ReplaceConfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BounceSummary {
    pub frames: u64,
    pub sample_rate: u32,
    pub peak: f32,
    pub overloaded_frames: u64,
    /// Peak in the final 100 ms. A low value cannot rule out a later echo.
    pub final_peak: f32,
}

impl BounceSummary {
    pub fn tail_may_be_truncated(&self) -> bool { self.final_peak > 0.0001 }
}

/// A cancellation request is observed between bounded render/write operations.
/// Once publication succeeds the completed file belongs to the user.
#[derive(Default)]
pub struct BounceCancellation(AtomicBool);

impl BounceCancellation {
    pub const fn new() -> Self { Self(AtomicBool::new(false)) }
    pub fn cancel(&self) { self.0.store(true, Ordering::Release); }
    pub fn check(&self) -> io::Result<()> {
        if self.0.load(Ordering::Acquire) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Audio export cancelled"))
        } else {
            Ok(())
        }
    }
}

/// Validate before allocating a graph, compiling instruments, or creating a file.
pub fn validate_wav_size(sample_rate: u32, frames: u64) -> io::Result<()> {
    if sample_rate == 0 || sample_rate.checked_mul(FRAME_BYTES as u32).is_none() {
        return Err(invalid("Invalid export sample rate"));
    }
    if frames == 0 || frames > MAX_WAV_FRAMES {
        return Err(invalid("Export exceeds WAV RIFF limits or contains no frames; choose a shorter range or tail"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

struct FloatWav<W: Write + Seek> {
    output: W,
    expected: u64,
    summary: BounceSummary,
    failed: bool,
}

impl<W: Write + Seek> FloatWav<W> {
    fn new(mut output: W, sample_rate: u32, frames: u64) -> io::Result<Self> {
        validate_wav_size(sample_rate, frames)?;
        // IEEE float format with its required fact chunk. Sizes are committed
        // only after exactly the promised frames have been written.
        output.write_all(b"RIFF")?;
        output.write_all(&0u32.to_le_bytes())?;
        output.write_all(b"WAVEfmt ")?;
        output.write_all(&16u32.to_le_bytes())?;
        output.write_all(&3u16.to_le_bytes())?;
        output.write_all(&2u16.to_le_bytes())?;
        output.write_all(&sample_rate.to_le_bytes())?;
        output.write_all(&(sample_rate * FRAME_BYTES as u32).to_le_bytes())?;
        output.write_all(&(FRAME_BYTES as u16).to_le_bytes())?;
        output.write_all(&32u16.to_le_bytes())?;
        output.write_all(b"fact")?;
        output.write_all(&4u32.to_le_bytes())?;
        output.write_all(&0u32.to_le_bytes())?;
        output.write_all(b"data")?;
        output.write_all(&0u32.to_le_bytes())?;
        Ok(Self {
            output, expected: frames, failed: false,
            summary: BounceSummary {
                frames: 0, sample_rate, peak: 0.0, overloaded_frames: 0, final_peak: 0.0,
            },
        })
    }

    fn write(&mut self, samples: &[f32], cancel: &BounceCancellation) -> io::Result<()> {
        if self.failed { return Err(invalid("Export writer has already failed")); }
        // Poison on any failure, including invalid input or cancellation. A
        // caller cannot accidentally finalize a partially accepted block.
        self.failed = true;
        cancel.check()?;
        if samples.len() % 2 != 0 {
            return Err(invalid("Export requires interleaved stereo frames"));
        }
        let count = samples.len() as u64 / 2;
        if count > self.expected - self.summary.frames {
            return Err(invalid("Export produced more frames than its declared range"));
        }
        let final_start = self.expected.saturating_sub((self.summary.sample_rate as u64).div_ceil(10));
        for (offset, frame) in samples.chunks_exact(2).enumerate() {
            if offset % 1024 == 0 { cancel.check()?; }
            let position = self.summary.frames;
            if !frame[0].is_finite() || !frame[1].is_finite() {
                return Err(invalid(format!("Non-finite audio at export frame {position}")));
            }
            let peak = frame[0].abs().max(frame[1].abs());
            self.output.write_all(&frame[0].to_le_bytes())?;
            self.output.write_all(&frame[1].to_le_bytes())?;
            self.summary.peak = self.summary.peak.max(peak);
            self.summary.overloaded_frames += u64::from(peak > 1.0);
            if position >= final_start { self.summary.final_peak = self.summary.final_peak.max(peak); }
            self.summary.frames += 1;
        }
        self.failed = false;
        Ok(())
    }

    fn finalize(mut self) -> io::Result<(W, BounceSummary)> {
        if self.failed || self.summary.frames != self.expected {
            return Err(invalid("Cannot finalize an incomplete or failed audio export"));
        }
        let bytes = self.expected * FRAME_BYTES;
        self.output.seek(SeekFrom::Start(4))?;
        self.output.write_all(&((HEADER_BYTES - 8 + bytes) as u32).to_le_bytes())?;
        self.output.seek(SeekFrom::Start(44))?;
        self.output.write_all(&(self.expected as u32).to_le_bytes())?;
        self.output.seek(SeekFrom::Start(52))?;
        self.output.write_all(&(bytes as u32).to_le_bytes())?;
        self.output.flush()?;
        Ok((self.output, self.summary))
    }
}

/// Streaming job sink, with a fixed 32 KiB I/O buffer regardless of duration.
/// Drop the sink on graph/generator errors or cancellation to abort the job.
pub struct BounceWriter {
    // Drop the open writer before its temporary path (also works on Windows).
    wav: FloatWav<BufWriter<File>>,
    temporary: tempfile::TempPath,
    destination: PathBuf,
    publication: Publication,
}

impl BounceWriter {
    pub fn create(
        destination: &Path,
        publication: Publication,
        sample_rate: u32,
        frames: u64,
        cancel: &BounceCancellation,
    ) -> io::Result<Self> {
        cancel.check()?;
        validate_wav_size(sample_rate, frames)?;
        if destination.file_name().is_none() { return Err(invalid("Choose an export filename")); }
        let parent = destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        // Resolve the directory once: later working-directory changes must not
        // redirect either cleanup or the final publication.
        let parent = parent.canonicalize()?;
        let destination = parent.join(destination.file_name().unwrap());
        if publication == Publication::CreateNew {
            match destination.symlink_metadata() {
                Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Export destination exists; confirm overwrite or choose another filename")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let temporary = tempfile::Builder::new().prefix(".eseq-bounce-").suffix(".tmp").tempfile_in(parent)?;
        let (file, temporary) = temporary.into_parts();
        let wav = FloatWav::new(BufWriter::with_capacity(BUFFER_BYTES, file), sample_rate, frames)?;
        Ok(Self { wav, temporary, destination, publication })
    }

    pub fn write(&mut self, samples: &[f32], cancel: &BounceCancellation) -> io::Result<()> {
        self.wav.write(samples, cancel)
    }

    pub fn finish(self, cancel: &BounceCancellation) -> io::Result<BounceSummary> {
        cancel.check()?;
        let (writer, summary) = self.wav.finalize()?;
        let file = writer.into_inner().map_err(|error| error.into_error())?;
        file.sync_all()?;
        drop(file);
        cancel.check()?;
        match self.publication {
            Publication::CreateNew => self.temporary.persist_noclobber(&self.destination),
            Publication::ReplaceConfirmed => self.temporary.persist(&self.destination),
        }.map_err(|error| error.error)?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn float_wav_preserves_samples_and_reports_exact_tail_window() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("song.wav");
        let cancel = BounceCancellation::default();
        let mut writer = BounceWriter::create(&path, Publication::CreateNew, 100, 12, &cancel).unwrap();
        writer.write(&[2.5, -3.25, 0.2, -0.2], &cancel).unwrap();
        writer.write(&[0.0; 20], &cancel).unwrap();
        assert!(!path.exists());
        let summary = writer.finish(&cancel).unwrap();
        assert_eq!(summary.frames, 12);
        assert_eq!(summary.peak, 3.25);
        assert_eq!(summary.overloaded_frames, 1);
        assert_eq!(summary.final_peak, 0.0);
        assert!(!summary.tail_may_be_truncated());
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec(), hound::WavSpec {
            channels: 2, sample_rate: 100, bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        });
        assert_eq!(reader.duration(), 12);
        let samples = reader.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(&samples[..4], &[2.5, -3.25, 0.2, -0.2]);
        assert_eq!(std::fs::metadata(path).unwrap().len(), HEADER_BYTES + 12 * FRAME_BYTES);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_errors_and_destination_races_never_publish_partial_audio() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("song.wav");
        for failure in 0..6 {
            let cancel = BounceCancellation::default();
            let mut writer = BounceWriter::create(&path, Publication::CreateNew, 48000, 2, &cancel).unwrap();
            match failure {
                0 => { writer.write(&[0.0; 4], &cancel).unwrap(); cancel.cancel(); }
                1 => { writer.write(&[0.0; 2], &cancel).unwrap(); }
                2 => { assert!(writer.write(&[0.0, f32::NAN], &cancel).unwrap_err().to_string().contains("frame 0")); }
                3 => { assert!(writer.write(&[0.0; 6], &cancel).is_err()); }
                4 => { assert!(writer.write(&[0.0], &cancel).is_err()); }
                5 => {
                    writer.write(&[0.0; 4], &cancel).unwrap();
                    std::fs::write(&path, b"racing file").unwrap();
                }
                _ => unreachable!(),
            }
            assert!(writer.finish(&cancel).is_err());
            if failure == 5 {
                assert_eq!(std::fs::read(&path).unwrap(), b"racing file");
                std::fs::remove_file(&path).unwrap();
            }
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn confirmed_replacement_preserves_original_until_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("song.wav");
        std::fs::write(&path, b"original").unwrap();
        let cancel = BounceCancellation::default();
        assert!(BounceWriter::create(&path, Publication::CreateNew, 48000, 1, &cancel).is_err());
        let writer = BounceWriter::create(&path, Publication::ReplaceConfirmed, 48000, 1, &cancel).unwrap();
        drop(writer); // graph/load failure
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let mut writer = BounceWriter::create(&path, Publication::ReplaceConfirmed, 48000, 1, &cancel).unwrap();
        writer.write(&[0.25, -0.25], &cancel).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(writer.finish(&cancel).unwrap().tail_may_be_truncated());
        assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 1);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    struct FailingOutput {
        cursor: Cursor<Vec<u8>>,
        remaining: usize,
        fail_flush: bool,
    }
    impl Write for FailingOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 { return Err(io::Error::other("simulated disk full")); }
            let count = bytes.len().min(self.remaining);
            let written = self.cursor.write(&bytes[..count])?;
            self.remaining -= written;
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail_flush { Err(io::Error::other("simulated flush failure")) } else { Ok(()) }
        }
    }
    impl Seek for FailingOutput {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> { self.cursor.seek(position) }
    }

    #[test]
    fn io_failures_poison_the_writer_including_finalization() {
        let cancel = BounceCancellation::default();
        for (budget, fail_flush) in [(60, false), (64, false), (usize::MAX, true)] {
            let output = FailingOutput { cursor: Cursor::new(Vec::new()), remaining: budget, fail_flush };
            let mut wav = FloatWav::new(output, 48000, 1).unwrap();
            let result = wav.write(&[0.5, -0.5], &cancel);
            if budget == 60 {
                assert!(result.is_err());
                assert!(wav.write(&[0.5, -0.5], &cancel).is_err());
            } else { result.unwrap(); }
            assert!(wav.finalize().is_err());
        }
    }

    #[test]
    fn riff_limits_and_bounded_streaming_do_not_depend_on_duration() {
        assert!(validate_wav_size(48000, MAX_WAV_FRAMES).is_ok());
        for frames in [0, MAX_WAV_FRAMES + 1, u64::MAX] {
            assert!(validate_wav_size(48000, frames).is_err());
        }
        assert!(validate_wav_size(0, 1).is_err());
        assert!(validate_wav_size(u32::MAX, 1).is_err());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("long.wav");
        let cancel = BounceCancellation::default();
        let mut writer = BounceWriter::create(&path, Publication::CreateNew, 48000, MAX_WAV_FRAMES, &cancel).unwrap();
        let block = [0.0; 1024];
        for _ in 0..1000 { writer.write(&block, &cancel).unwrap(); }
        assert_eq!(writer.wav.output.capacity(), BUFFER_BYTES);
        assert!(writer.wav.output.buffer().len() <= BUFFER_BYTES);
        drop(writer);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
