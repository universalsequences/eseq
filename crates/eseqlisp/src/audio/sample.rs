use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use crate::vm::Value;

static SAMPLE_REGISTRY: OnceLock<Mutex<std::collections::HashMap<String, Arc<SampleBuffer>>>> =
    OnceLock::new();

#[derive(Clone, Debug)]
pub struct SampleBuffer {
    pub id: String,
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: usize,
    pub duration_seconds: f64,
    pub peaks: Vec<WaveformMipLevel>,
}

#[derive(Clone, Debug)]
pub struct WaveformMipLevel {
    pub samples_per_bucket: usize,
    pub buckets: Vec<MinMaxPair>,
}

#[derive(Clone, Copy, Debug)]
pub struct MinMaxPair {
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Copy, Debug)]
struct WavFormat {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
}

impl SampleBuffer {
    pub fn load_wav(path: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        let (format, data) = parse_wav_bytes(&bytes)?;
        let frames = data.len() / format.block_align as usize;
        if frames == 0 {
            return Err("wav file contains no audio frames".to_string());
        }
        let samples = decode_mono_samples(data, format)?;
        let peaks = build_peak_pyramid(&samples);
        let duration_seconds = frames as f64 / format.sample_rate.max(1) as f64;
        Ok(Self {
            id: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("sample")
                .to_string(),
            path: path.to_path_buf(),
            sample_rate: format.sample_rate,
            channels: format.channels,
            frames,
            duration_seconds,
            peaks,
        })
    }

    pub fn to_value(&self) -> Value {
        map_value(vec![
            ("id", Value::String(self.id.clone())),
            ("registry-key", Value::String(self.cache_key())),
            ("path", Value::String(self.path.display().to_string())),
            ("sample-rate", Value::Number(self.sample_rate as f64)),
            ("channels", Value::Number(self.channels as f64)),
            ("frames", Value::Number(self.frames as f64)),
            ("duration", Value::Number(self.duration_seconds)),
        ])
    }

    pub fn cache_key(&self) -> String {
        self.path.display().to_string()
    }

    pub fn register(self) -> Arc<Self> {
        let sample = Arc::new(self);
        let registry = SAMPLE_REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        if let Ok(mut registry) = registry.lock() {
            registry.insert(sample.cache_key(), sample.clone());
        }
        sample
    }

    pub fn levels(&self) -> &[WaveformMipLevel] {
        &self.peaks
    }
}

impl WaveformMipLevel {
    pub fn flattened_pairs(&self) -> Vec<f32> {
        let mut data = Vec::with_capacity(self.buckets.len() * 2);
        for bucket in &self.buckets {
            data.push(bucket.min);
            data.push(bucket.max);
        }
        data
    }
}

fn parse_wav_bytes(bytes: &[u8]) -> Result<(WavFormat, &[u8]), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }

    let mut format = None;
    let mut data = None;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = read_u32_le(&bytes[offset + 4..offset + 8]) as usize;
        let data_start = offset + 8;
        let data_end = data_start.saturating_add(chunk_size);
        if data_end > bytes.len() {
            return Err("wav chunk exceeds file length".to_string());
        }
        match chunk_id {
            b"fmt " => {
                if chunk_size < 16 {
                    return Err("wav fmt chunk is too small".to_string());
                }
                format = Some(WavFormat {
                    audio_format: read_u16_le(&bytes[data_start..data_start + 2]),
                    channels: read_u16_le(&bytes[data_start + 2..data_start + 4]),
                    sample_rate: read_u32_le(&bytes[data_start + 4..data_start + 8]),
                    block_align: read_u16_le(&bytes[data_start + 12..data_start + 14]),
                    bits_per_sample: read_u16_le(&bytes[data_start + 14..data_start + 16]),
                });
            }
            b"data" => {
                data = Some(&bytes[data_start..data_end]);
            }
            _ => {}
        }
        offset = data_end + (chunk_size % 2);
    }

    let format = format.ok_or_else(|| "wav file is missing fmt chunk".to_string())?;
    let data = data.ok_or_else(|| "wav file is missing data chunk".to_string())?;
    if format.channels == 0 || format.block_align == 0 {
        return Err("wav format has invalid channel layout".to_string());
    }
    if format.audio_format != 1 && format.audio_format != 3 {
        return Err(format!(
            "unsupported wav format {}; expected PCM or IEEE float",
            format.audio_format
        ));
    }
    Ok((format, data))
}

fn decode_mono_samples(data: &[u8], format: WavFormat) -> Result<Vec<f32>, String> {
    let frame_stride = format.block_align as usize;
    let channel_count = format.channels as usize;
    let bytes_per_sample = (format.bits_per_sample as usize).div_ceil(8);
    if bytes_per_sample == 0 || channel_count * bytes_per_sample > frame_stride {
        return Err("wav format uses an invalid block alignment".to_string());
    }

    let mut samples = Vec::with_capacity(data.len() / frame_stride);
    for frame in data.chunks_exact(frame_stride) {
        let mut sum = 0.0f32;
        for channel in 0..channel_count {
            let sample_offset = channel * bytes_per_sample;
            let raw = &frame[sample_offset..sample_offset + bytes_per_sample];
            sum += decode_sample(raw, format.audio_format, format.bits_per_sample)?;
        }
        samples.push((sum / channel_count as f32).clamp(-1.0, 1.0));
    }
    Ok(samples)
}

fn decode_sample(bytes: &[u8], audio_format: u16, bits_per_sample: u16) -> Result<f32, String> {
    match (audio_format, bits_per_sample) {
        (1, 8) => Ok((bytes[0] as f32 - 128.0) / 128.0),
        (1, 16) => Ok(read_i16_le(bytes) as f32 / i16::MAX as f32),
        (1, 24) => Ok(read_i24_le(bytes) as f32 / 8_388_607.0),
        (1, 32) => Ok(read_i32_le(bytes) as f32 / i32::MAX as f32),
        (3, 32) => {
            Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).clamp(-1.0, 1.0))
        }
        _ => Err(format!(
            "unsupported wav sample format: audio_format={audio_format}, bits_per_sample={bits_per_sample}"
        )),
    }
}

fn build_peak_pyramid(samples: &[f32]) -> Vec<WaveformMipLevel> {
    let mut levels = Vec::new();
    let mut samples_per_bucket = 1usize;
    let max_bucket = 4096usize.min(samples.len().max(1));
    while samples_per_bucket <= max_bucket {
        let mut buckets = Vec::new();
        for chunk in samples.chunks(samples_per_bucket) {
            let mut min = 1.0f32;
            let mut max = -1.0f32;
            for sample in chunk {
                min = min.min(*sample);
                max = max.max(*sample);
            }
            buckets.push(MinMaxPair { min, max });
        }
        levels.push(WaveformMipLevel {
            samples_per_bucket,
            buckets,
        });
        samples_per_bucket = samples_per_bucket.saturating_mul(2);
        if samples_per_bucket == 0 {
            break;
        }
    }
    levels
}

pub fn get_registered_sample(key: &str) -> Option<Arc<SampleBuffer>> {
    let registry = SAMPLE_REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    registry.lock().ok()?.get(key).cloned()
}

pub fn register_sample(sample: SampleBuffer) -> Arc<SampleBuffer> {
    sample.register()
}

fn read_u16_le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_i16_le(bytes: &[u8]) -> i16 {
    i16::from_le_bytes([bytes[0], bytes[1]])
}

fn read_i24_le(bytes: &[u8]) -> i32 {
    let extended = if bytes[2] & 0x80 != 0 { 0xFF } else { 0x00 };
    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], extended])
}

fn read_i32_le(bytes: &[u8]) -> i32 {
    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn map_value(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_wav_loads_basic_metadata_and_peaks() {
        let mut bytes = Vec::new();
        let samples = [0_i16, i16::MAX / 2, -i16::MAX / 2, i16::MAX / 4];
        let data_len = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&44_100_u32.to_le_bytes());
        bytes.extend_from_slice(&(44_100_u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let path = std::env::temp_dir().join("eseqlisp-waveform-test.wav");
        fs::write(&path, &bytes).expect("write wav");
        let sample = SampleBuffer::load_wav(&path).expect("load wav");
        assert_eq!(sample.sample_rate, 44_100);
        assert_eq!(sample.channels, 1);
        assert_eq!(sample.frames, 4);
        assert!(sample.peaks.len() >= 2);
        assert_eq!(sample.peaks[0].samples_per_bucket, 1);
        assert!(
            sample.peaks[0]
                .buckets
                .iter()
                .any(|bucket| bucket.max > 0.4)
        );
        assert!(
            sample.peaks[0]
                .buckets
                .iter()
                .any(|bucket| bucket.min < -0.4)
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn registered_sample_can_be_looked_up_by_key() {
        let sample = SampleBuffer {
            id: "sample.wav".to_string(),
            path: PathBuf::from("/tmp/sample.wav"),
            sample_rate: 48_000,
            channels: 1,
            frames: 48_000,
            duration_seconds: 1.0,
            peaks: build_peak_pyramid(&vec![0.0; 1024]),
        };
        let registered = register_sample(sample);
        let fetched = get_registered_sample(&registered.cache_key()).expect("registered sample");
        assert_eq!(fetched.sample_rate, 48_000);
        assert_eq!(fetched.frames, 48_000);
    }
}
