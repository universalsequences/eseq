//! Exact source/file intervals around fixed-size graph processing.

use super::*;

#[derive(Clone, Debug)]
pub struct BouncePlan {
    pub(super) sample_rate: u32,
    pub(super) block_frames: usize,
    pub(super) source_start: u64,
    pub(super) source_end: u64,
    pub(super) write_start: u64,
    pub(super) render_end: u64,
    pub(super) file_frames: u64,
}

impl BouncePlan {
    pub fn new(
        sample_rate: u32,
        block_frames: usize,
        bpm: u32,
        song_end_beat: f64,
        selection: Option<(f64, f64)>,
        latency_frames: u32,
        tail_seconds: f64,
    ) -> io::Result<Self> {
        if sample_rate == 0 || bpm == 0 || block_frames == 0
            || block_frames.checked_mul(2).is_none()
        {
            return Err(invalid("Invalid export engine clock or block size"));
        }
        let (start, end) = selection.unwrap_or((0.0, song_end_beat));
        if !song_end_beat.is_finite() || !start.is_finite() || !end.is_finite()
            || start < 0.0 || end <= start || end > song_end_beat
        {
            return Err(invalid("Export requires a nonempty range inside the arrangement"));
        }
        if !tail_seconds.is_finite() || !(0.0..=600.0).contains(&tail_seconds) {
            return Err(invalid("Export tail must be between 0 and 600 seconds"));
        }
        let frames = |value: f64| -> io::Result<u64> {
            // u64::MAX rounds up to 2^64 in f64; reject that endpoint too.
            if !value.is_finite() || value < 0.0 || value.ceil() >= u64::MAX as f64 {
                Err(invalid("Export timeline exceeds the supported sample clock"))
            } else { Ok(value.ceil() as u64) }
        };
        let samples_per_quarter = sample_rate as f64 * 60.0 / bpm as f64;
        let source_start = frames(start * samples_per_quarter)?;
        let source_end = frames(end * samples_per_quarter)?;
        let tail = frames(tail_seconds * sample_rate as f64)?;
        if source_start >= source_end {
            return Err(invalid("Selected export range contains no sample frames"));
        }
        let overflow = || invalid("Export timeline exceeds the supported sample clock");
        let write_start = source_start.checked_add(latency_frames as u64).ok_or_else(overflow)?;
        let render_end = source_end.checked_add(latency_frames as u64)
            .and_then(|end| end.checked_add(tail)).ok_or_else(overflow)?;
        let file_frames = render_end - write_start;
        validate_wav_size(sample_rate, file_frames)?;
        // The final complete DSP block must fit the integer cursor too.
        render_end.checked_add(block_frames as u64 - 1).ok_or_else(overflow)?;
        Ok(Self { sample_rate, block_frames, source_start, source_end, write_start, render_end, file_frames })
    }

    pub fn source_range(&self) -> std::ops::Range<u64> { self.source_start..self.source_end }
    pub fn file_frames(&self) -> u64 { self.file_frames }
    pub fn sample_rate(&self) -> u32 { self.sample_rate }
    pub fn block_frames(&self) -> usize { self.block_frames }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BouncePhase { Prefix, Rendering, Tail, Finalizing }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BounceProgress {
    pub phase: BouncePhase,
    pub rendered_frames: u64,
    pub total_render_frames: u64,
    pub written_frames: u64,
    pub total_file_frames: u64,
}

/// Drive a prepared musical renderer and the transactional WAV sink. The
/// renderer must stop admitting new notes at plan.source_range().end, release
/// gates there, and keep its DSP history through the remaining blocks. This
/// loop owns interval trimming only; it never resizes a DSP block or seeks.
pub fn render_to_wav(
    plan: &BouncePlan,
    destination: &Path,
    publication: Publication,
    cancel: &BounceCancellation,
    mut render_block: impl FnMut(u64, &mut [f32]) -> io::Result<()>,
    mut progress: impl FnMut(BounceProgress),
) -> io::Result<BounceSummary> {
    cancel.check()?;
    let mut writer = BounceWriter::create(destination, publication, plan.sample_rate, plan.file_frames, cancel)?;
    let mut block = vec![0.0; plan.block_frames * 2];
    let mut rendered = 0;
    let mut written = 0;
    while rendered < plan.render_end {
        cancel.check()?;
        render_block(rendered, &mut block)?;
        cancel.check()?;
        let block_end = (rendered + plan.block_frames as u64).min(plan.render_end);
        let valid_samples = (block_end - rendered) as usize * 2;
        if let Some(index) = block[..valid_samples].iter().position(|sample| !sample.is_finite()) {
            return Err(invalid(format!("Non-finite export audio at source frame {}", rendered + index as u64 / 2)));
        }
        let from = rendered.max(plan.write_start);
        if from < block_end {
            writer.write(&block[(from - rendered) as usize * 2..valid_samples], cancel)?;
            written += block_end - from;
        }
        rendered = block_end;
        progress(BounceProgress {
            phase: if rendered < plan.write_start { BouncePhase::Prefix }
                else if rendered < plan.source_end { BouncePhase::Rendering }
                else { BouncePhase::Tail },
            rendered_frames: rendered, total_render_frames: plan.render_end,
            written_frames: written, total_file_frames: plan.file_frames,
        });
    }
    progress(BounceProgress {
        phase: BouncePhase::Finalizing,
        rendered_frames: rendered, total_render_frames: plan.render_end,
        written_frames: written, total_file_frames: plan.file_frames,
    });
    writer.finish(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_blocks_trim_prefix_latency_and_tail_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        for tail in [0.0, 0.01] {
            let plan = BouncePlan::new(1000, 128, 60, 2.0, Some((0.1301, 0.7311)), 17, tail).unwrap();
            assert_eq!(plan.source_range(), 131..732);
            assert_eq!(plan.file_frames(), 601 + (tail * 1000.0) as u64);
            let path = dir.path().join(format!("{tail}.wav"));
            let mut calls = Vec::new();
            let mut updates = Vec::new();
            let summary = render_to_wav(&plan, &path, Publication::CreateNew,
                &BounceCancellation::default(), |start, output| {
                    calls.push((start, output.len()));
                    for (i, frame) in output.chunks_exact_mut(2).enumerate() {
                        let value = (start + i as u64) as f32;
                        frame.copy_from_slice(&[value, -value]);
                    }
                    Ok(())
                }, |progress| updates.push(progress)).unwrap();
            assert_eq!(summary.frames, plan.file_frames());
            assert!(calls.iter().enumerate().all(|(i, call)| *call == (i as u64 * 128, 256)));
            let mut wav = hound::WavReader::open(path).unwrap();
            let samples = wav.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(samples.len(), plan.file_frames() as usize * 2);
            assert_eq!(&samples[..2], &[148.0, -148.0]);
            assert_eq!(samples[samples.len() - 2], (plan.render_end - 1) as f32);
            assert_eq!(updates.last().unwrap().phase, BouncePhase::Finalizing);
            assert_eq!(updates.last().unwrap().written_frames, plan.file_frames());
        }
    }

    #[test]
    fn render_failure_and_nonfinite_prefix_preserve_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.wav");
        let plan = BouncePlan::new(1000, 128, 60, 2.0, Some((1.0, 1.5)), 0, 0.0).unwrap();
        for failure in 0..3 {
            std::fs::write(&path, b"original").unwrap();
            let cancel = BounceCancellation::default();
            let result = render_to_wav(&plan, &path, Publication::ReplaceConfirmed, &cancel,
                |_, block| {
                    match failure {
                        0 => return Err(io::Error::other("DSP failed")),
                        1 => block[7] = f32::NAN,
                        _ => cancel.cancel(),
                    }
                    Ok(())
                }, |_| {});
            let error = result.unwrap_err();
            if failure == 1 { assert!(error.to_string().contains("source frame 3")); }
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn invalid_ranges_and_oversize_files_fail_before_rendering() {
        for selection in [Some((-1.0, 1.0)), Some((1.0, 1.0)), Some((0.0, 3.0)), Some((f64::NAN, 1.0))] {
            assert!(BouncePlan::new(48000, 512, 120, 2.0, selection, 0, 10.0).is_err());
        }
        for tail in [-1.0, 600.1, f64::INFINITY, f64::NAN] {
            assert!(BouncePlan::new(48000, 512, 120, 2.0, None, 0, tail).is_err());
        }
        assert!(BouncePlan::new(48000, 512, 120, 1e10, None, 0, 0.0).is_err());
        assert!(BouncePlan::new(48000, 0, 120, 2.0, None, 0, 0.0).is_err());
        assert!(BouncePlan::new(48000, 512, 120, 0.0, None, 0, 0.0).is_err());
    }
}
