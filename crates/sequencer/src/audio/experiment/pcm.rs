//! Bounded, optional audio evidence for the experiment harness. Only push runs
//! on the audio thread; allocation and WAV writing belong to the caller.
use super::{ArrayQueue, AtomicBool, Ordering, Result, Sha256, Digest};
use std::{fs::OpenOptions, io::BufWriter, path::Path};

const CHUNK_SAMPLES: usize = 1024;

struct Chunk {
    samples: [f32; CHUNK_SAMPLES],
    len: usize,
}

pub(super) struct PcmCapture {
    chunks: ArrayQueue<Chunk>,
    overflow: AtomicBool,
}

impl PcmCapture {
    pub(super) fn new(sample_rate: u32, channels: usize, block_frames: usize,
        seconds: f64) -> Result<Self>
    {
        if sample_rate == 0 || channels == 0 || block_frames == 0
            || !seconds.is_finite() || !(0.0..=60.0).contains(&seconds)
        { return Err("Invalid PCM capture dimensions".into()); }
        // Account for partial packets in every graph block, plus the two
        // possible blocks straddling the control-thread interval boundaries.
        let blocks = (seconds * sample_rate as f64 / block_frames as f64).ceil() as usize + 2;
        let chunks_per_block = block_frames.checked_mul(channels)
            .ok_or("PCM capture size overflow")?.div_ceil(CHUNK_SAMPLES);
        let capacity = blocks.checked_mul(chunks_per_block).ok_or("PCM capture size overflow")?;
        Ok(Self { chunks: ArrayQueue::new(capacity), overflow: AtomicBool::new(false) })
    }

    pub(super) fn push(&self, samples: &[f32]) {
        for source in samples.chunks(CHUNK_SAMPLES) {
            let mut chunk = Chunk { samples: [0.0; CHUNK_SAMPLES], len: source.len() };
            chunk.samples[..source.len()].copy_from_slice(source);
            if self.chunks.push(chunk).is_err() {
                self.overflow.store(true, Ordering::Release);
                return;
            }
        }
    }

    pub(super) fn write_wav(&self, path: &Path, sample_rate: u32, channels: usize,
        expected_samples: usize) -> Result<serde_json::Value>
    {
        if self.overflow.load(Ordering::Acquire) { return Err("PCM capture overflow".into()); }
        let spec = hound::WavSpec {
            channels: channels.try_into()?, sample_rate, bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut writer = hound::WavWriter::new(BufWriter::new(file), spec)?;
        let mut hash = Sha256::new();
        let mut samples = 0;
        while let Some(chunk) = self.chunks.pop() {
            for &sample in &chunk.samples[..chunk.len] {
                // Preserve the callback's exact floats, including values
                // outside [-1, 1]; never normalize or clip profiling evidence.
                writer.write_sample(sample)?;
                hash.update(sample.to_le_bytes());
                samples += 1;
            }
        }
        writer.finalize()?;
        if samples != expected_samples {
            return Err(format!("PCM capture length mismatch: {samples} != {expected_samples}").into());
        }
        Ok(serde_json::json!({
            "path": path, "frames": samples / channels, "sample_rate": sample_rate,
            "channels": channels, "pcm_sha256": format!("{:x}", hash.finalize()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_preserves_partial_packets_and_unclipped_float_samples() {
        let capture = PcmCapture::new(48_000, 3, 512, 0.1).unwrap();
        let block: Vec<f32> = (0..1536).map(|i| (i as f32 - 768.0) / 128.0).collect();
        let (_, heap) = crate::heap_audit::measure(|| {
            for _ in 0..10 { capture.push(&block); }
        });
        assert_eq!(heap, crate::heap_audit::Counts::default());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.wav");
        let report = capture.write_wav(&path, 48_000, 3, block.len() * 10).unwrap();
        let reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.spec().channels, 3);
        assert_eq!(reader.spec().sample_rate, 48_000);
        let samples: Vec<f32> = reader.into_samples().map(|sample| sample.unwrap()).collect();
        assert_eq!(samples, block.repeat(10));
        assert_eq!(report["frames"], 5120);
    }

    #[test]
    fn overflow_and_length_mismatch_fail_the_capture() {
        let dir = tempfile::tempdir().unwrap();
        let capture = PcmCapture::new(48_000, 2, 512, 0.0).unwrap();
        for _ in 0..3 { capture.push(&[0.0; 1024]); }
        assert!(capture.write_wav(&dir.path().join("overflow.wav"), 48_000, 2, 3072).is_err());
        let capture = PcmCapture::new(48_000, 2, 512, 0.0).unwrap();
        capture.push(&[0.0; 1024]);
        assert!(capture.write_wav(&dir.path().join("short.wav"), 48_000, 2, 2048).is_err());
    }
}
