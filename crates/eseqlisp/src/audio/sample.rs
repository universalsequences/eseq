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

#[derive(Clone, Debug)]
pub struct DecodedWav {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: usize,
    pub samples: Vec<f32>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WavCodec {
    Pcm,
    IeeeFloat,
}

#[derive(Clone, Copy, Debug)]
struct WavFormat {
    codec: WavCodec,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
}

impl SampleBuffer {
    pub fn load_wav(path: &Path) -> Result<Self, String> {
        let decoded = load_wav_file(path)?;
        for warning in &decoded.warnings {
            eprintln!("wav decode warning for {}: {warning}", path.display());
        }
        let samples = decoded_mono_samples(&decoded);
        let peaks = build_peak_pyramid(&samples);
        let duration_seconds = decoded.frames as f64 / decoded.sample_rate.max(1) as f64;
        Ok(Self {
            id: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("sample")
                .to_string(),
            path: path.to_path_buf(),
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            frames: decoded.frames,
            duration_seconds,
            peaks,
        })
    }

    /// A waveform for audio that has no file yet (a frozen resample). `path`
    /// is only the registry key; nothing is read from it.
    pub fn from_pcm(path: PathBuf, sample_rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        let channels = channels.max(1);
        let frames = samples.len() / usize::from(channels);
        let decoded = DecodedWav { sample_rate, channels, frames, samples, warnings: Vec::new() };
        let peaks = build_peak_pyramid(&decoded_mono_samples(&decoded));
        Self {
            id: path.file_name().and_then(|name| name.to_str()).unwrap_or("sample").to_string(),
            path,
            sample_rate,
            channels,
            frames,
            duration_seconds: frames as f64 / sample_rate.max(1) as f64,
            peaks,
        }
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

    /// Register stereo-folded `stereo` (interleaved L/R) under `path`, plus
    /// its two channel waveforms when the source had two or more channels
    /// (a mono source folded to two equal channels draws as one).
    pub fn register_stereo(path: &Path, sample_rate: u32, source_channels: u16, stereo: Vec<f32>) {
        if source_channels >= 2 {
            Self::register_channels(path, sample_rate, 2, &stereo);
        }
        Self::from_pcm(path.to_path_buf(), sample_rate, 2, stereo).register();
    }

    /// Drop the sample registered under `key` and its channel waveforms.
    pub fn unregister(key: &str) {
        let registry = SAMPLE_REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        if let Ok(mut registry) = registry.lock() {
            registry.remove(key);
            for channel in 0..2 {
                registry.remove(&channel_key(key, channel));
            }
        }
    }

    /// Register one waveform per channel of interleaved `samples` beside
    /// the sample registered under `path`, at [`channel_key`], so a view can
    /// draw a stereo file as two waveforms (Ableton's arrangement does).
    /// Channel peaks start at eight frames per bucket, an eighth of the
    /// mixed waveform's memory: only the deepest arrangement zoom (a few
    /// frames per pixel) shows the difference, as slightly blocky peaks.
    pub fn register_channels(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) {
        let channels = usize::from(channels.max(1));
        let frames = samples.len() / channels;
        for channel in 0..channels {
            let mono: Vec<f32> = samples.iter().skip(channel).step_by(channels).copied().collect();
            let key = channel_key(&path.display().to_string(), channel);
            Self {
                id: key.clone(),
                path: PathBuf::from(&key),
                sample_rate,
                channels: 1,
                frames,
                duration_seconds: frames as f64 / sample_rate.max(1) as f64,
                peaks: build_peak_pyramid_from(&mono, CHANNEL_FIRST_BUCKET),
            }
            .register();
        }
    }
}

/// Registry key of drag preview `id`: a sample's waveform registered while
/// it is dragged over a timeline, before it is dropped (the host decodes it;
/// the timeline's `:drop-ghost-sample` names the id).
pub fn drag_preview_key(id: u64) -> String {
    format!("drag-preview#{id}")
}

/// Frames per bucket of a channel waveform's finest level.
pub const CHANNEL_FIRST_BUCKET: usize = 8;

/// Registry key of channel `channel` (0 = left) of the sample registered
/// under `key`, when [`SampleBuffer::register_channels`] registered it.
pub fn channel_key(key: &str, channel: usize) -> String {
    format!("{key}#ch{channel}")
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

pub fn load_wav_file(path: &Path) -> Result<DecodedWav, String> {
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    decode_wav_bytes(&bytes).map_err(|err| format!("failed to decode {}: {err}", path.display()))
}

pub fn decode_wav_bytes(bytes: &[u8]) -> Result<DecodedWav, String> {
    let (format, data, mut warnings) = parse_wav_bytes(bytes)?;
    let frame_stride = format.block_align as usize;
    let full_data_len = data.len() / frame_stride * frame_stride;
    let trailing = data.len() - full_data_len;
    if trailing > 0 {
        warnings.push(format!(
            "ignored {trailing} trailing byte(s) that do not make a complete WAV frame"
        ));
    }
    let data = &data[..full_data_len];
    let frames = data.len() / frame_stride;
    if frames == 0 {
        return Err("wav file contains no complete audio frames".to_string());
    }
    let samples = decode_interleaved_samples(data, format)?;
    Ok(DecodedWav {
        sample_rate: format.sample_rate,
        channels: format.channels,
        frames,
        samples,
        warnings,
    })
}

/// A WAV file's shape, read from its headers alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: usize,
}

impl WavInfo {
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate.max(1) as f64
    }
}

/// [`WavInfo`] for the file at `path`, reading only its chunk headers and
/// format (never the audio), under the same rules [`decode_wav_bytes`]
/// decodes by: a data chunk running past the end counts what is there.
pub fn wav_info(path: &Path) -> Result<WavInfo, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).map_err(|err| format!("failed to open {}: {err}", path.display()))?;
    let file_len = file.metadata().map_err(|err| err.to_string())?.len();
    let mut header = [0u8; 12];
    file.read_exact(&mut header).map_err(|_| "not a RIFF/WAVE file".to_string())?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let (mut format, mut data_len) = (None, None);
    let mut offset = 12u64;
    let mut chunk = [0u8; 8];
    while offset + 8 <= file_len && (format.is_none() || data_len.is_none()) {
        file.seek(SeekFrom::Start(offset)).map_err(|err| err.to_string())?;
        file.read_exact(&mut chunk).map_err(|err| err.to_string())?;
        let size = read_u32_le(&chunk[4..8]) as u64;
        match &chunk[0..4] {
            b"fmt " => {
                if size < 16 {
                    return Err("wav fmt chunk is too small".to_string());
                }
                let mut fmt = vec![0u8; size.min(64) as usize];
                file.read_exact(&mut fmt).map_err(|err| err.to_string())?;
                format = Some(parse_wav_format(&fmt)?);
            }
            b"data" => data_len = Some(size.min(file_len - offset - 8)),
            _ => {}
        }
        offset += 8 + size + (size % 2);
    }
    let format = format.ok_or_else(|| "wav file is missing fmt chunk".to_string())?;
    let data_len = data_len.ok_or_else(|| "wav file is missing data chunk".to_string())?;
    if format.channels == 0 || format.block_align == 0 {
        return Err("wav format has invalid channel layout".to_string());
    }
    Ok(WavInfo {
        sample_rate: format.sample_rate,
        channels: format.channels,
        frames: data_len as usize / format.block_align as usize,
    })
}

/// Decode the WAV at `path` piece by piece (the same decoder as
/// [`decode_wav_bytes`]), handing `progress` the interleaved samples
/// decoded so far, with the file's [`WavInfo`], each time they have grown by
/// half (and once at the end), so a long file is shown a handful of times.
/// `keep_going` is asked between pieces of about 64k frames; `Ok(true)`
/// when the whole file was decoded, `Ok(false)` when it was cancelled.
pub fn decode_wav_progressively(
    path: &Path,
    keep_going: impl Fn() -> bool,
    mut progress: impl FnMut(&[f32], WavInfo),
) -> Result<bool, String> {
    const PIECE_FRAMES: usize = 1 << 16;
    let bytes = fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let (format, data, _warnings) = parse_wav_bytes(&bytes)?;
    let stride = format.block_align as usize;
    let frames = data.len() / stride;
    let info = WavInfo { sample_rate: format.sample_rate, channels: format.channels, frames };
    let channels = usize::from(format.channels);
    let mut samples = Vec::with_capacity(frames * channels);
    // The first look comes after a quarter second of audio.
    let mut next_show = format.sample_rate as usize / 4;
    for piece in data[..frames * stride].chunks(PIECE_FRAMES * stride) {
        if !keep_going() {
            return Ok(false);
        }
        samples.extend(decode_interleaved_samples(piece, format)?);
        let decoded = samples.len() / channels;
        if decoded >= next_show && decoded < frames {
            progress(&samples, info);
            next_show = decoded + decoded.max(format.sample_rate as usize) / 2;
        }
    }
    progress(&samples, info);
    Ok(true)
}

fn parse_wav_bytes(bytes: &[u8]) -> Result<(WavFormat, &[u8], Vec<String>), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }

    let mut warnings = Vec::new();
    let declared_len = read_u32_le(&bytes[4..8]) as usize + 8;
    if declared_len != bytes.len() {
        warnings.push(format!(
            "RIFF declared length {declared_len} byte(s), actual file length {} byte(s)",
            bytes.len()
        ));
    }

    let mut format = None;
    let mut data = None;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = read_u32_le(&bytes[offset + 4..offset + 8]) as usize;
        let data_start = offset + 8;
        let Some(mut data_end) = data_start.checked_add(chunk_size) else {
            return Err("wav chunk size overflows file offset".to_string());
        };
        if data_end > bytes.len() {
            if chunk_id == b"data" {
                warnings.push(format!(
                    "data chunk declares {chunk_size} byte(s) but only {} byte(s) remain",
                    bytes.len().saturating_sub(data_start)
                ));
                data_end = bytes.len();
            } else {
                warnings.push(format!(
                    "chunk {} declares {chunk_size} byte(s) past end of file; remaining chunks ignored",
                    chunk_name(chunk_id)
                ));
                break;
            }
        }
        match chunk_id {
            b"fmt " => {
                if chunk_size < 16 {
                    return Err("wav fmt chunk is too small".to_string());
                }
                format = Some(parse_wav_format(&bytes[data_start..data_end])?);
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
    let bytes_per_sample = (format.bits_per_sample as usize).div_ceil(8);
    if bytes_per_sample == 0
        || format.channels as usize * bytes_per_sample > format.block_align as usize
    {
        return Err("wav format uses an invalid block alignment".to_string());
    }
    Ok((format, data, warnings))
}

fn parse_wav_format(bytes: &[u8]) -> Result<WavFormat, String> {
    let format_code = read_u16_le(&bytes[0..2]);
    let channels = read_u16_le(&bytes[2..4]);
    let sample_rate = read_u32_le(&bytes[4..8]);
    let block_align = read_u16_le(&bytes[12..14]);
    let bits_per_sample = read_u16_le(&bytes[14..16]);
    let codec = match format_code {
        1 => WavCodec::Pcm,
        3 => WavCodec::IeeeFloat,
        0xFFFE => {
            if bytes.len() < 40 {
                return Err("WAVE_FORMAT_EXTENSIBLE fmt chunk is too small".to_string());
            }
            let guid = &bytes[24..40];
            if guid == PCM_SUBFORMAT_GUID {
                WavCodec::Pcm
            } else if guid == IEEE_FLOAT_SUBFORMAT_GUID {
                WavCodec::IeeeFloat
            } else {
                return Err(format!(
                    "unsupported WAVE_FORMAT_EXTENSIBLE subformat {}",
                    format_guid(guid)
                ));
            }
        }
        other => {
            return Err(format!(
                "unsupported wav format {other}; expected PCM, IEEE float, or WAVE_FORMAT_EXTENSIBLE PCM/float"
            ));
        }
    };

    Ok(WavFormat {
        codec,
        channels,
        sample_rate,
        block_align,
        bits_per_sample,
    })
}

fn decode_interleaved_samples(data: &[u8], format: WavFormat) -> Result<Vec<f32>, String> {
    let frame_stride = format.block_align as usize;
    let channel_count = format.channels as usize;
    let bytes_per_sample = (format.bits_per_sample as usize).div_ceil(8);
    if bytes_per_sample == 0 || channel_count * bytes_per_sample > frame_stride {
        return Err("wav format uses an invalid block alignment".to_string());
    }

    let mut samples = Vec::with_capacity(data.len() / frame_stride * channel_count);
    for frame in data.chunks_exact(frame_stride) {
        for channel in 0..channel_count {
            let sample_offset = channel * bytes_per_sample;
            let raw = &frame[sample_offset..sample_offset + bytes_per_sample];
            samples.push(decode_sample(raw, format.codec, format.bits_per_sample)?);
        }
    }
    Ok(samples)
}

fn decode_sample(bytes: &[u8], codec: WavCodec, bits_per_sample: u16) -> Result<f32, String> {
    match (codec, bits_per_sample) {
        (WavCodec::Pcm, 8) => Ok((bytes[0] as f32 - 128.0) / 128.0),
        (WavCodec::Pcm, 16) => Ok((read_i16_le(bytes) as f32 / 32_768.0).clamp(-1.0, 1.0)),
        (WavCodec::Pcm, 24) => Ok((read_i24_le(bytes) as f32 / 8_388_608.0).clamp(-1.0, 1.0)),
        (WavCodec::Pcm, 32) => Ok((read_i32_le(bytes) as f32 / 2_147_483_648.0).clamp(-1.0, 1.0)),
        (WavCodec::IeeeFloat, 32) => {
            Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).clamp(-1.0, 1.0))
        }
        (WavCodec::IeeeFloat, 64) => Ok(f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
        .clamp(-1.0, 1.0) as f32),
        _ => Err(format!(
            "unsupported wav sample format: codec={codec:?}, bits_per_sample={bits_per_sample}"
        )),
    }
}

fn decoded_mono_samples(decoded: &DecodedWav) -> Vec<f32> {
    let channels = decoded.channels as usize;
    decoded
        .samples
        .chunks_exact(channels)
        .map(|frame| (frame.iter().copied().sum::<f32>() / channels as f32).clamp(-1.0, 1.0))
        .collect()
}

fn build_peak_pyramid(samples: &[f32]) -> Vec<WaveformMipLevel> {
    build_peak_pyramid_from(samples, 1)
}

/// [`build_peak_pyramid`] whose finest level holds `first` frames per
/// bucket (a power of two).
fn build_peak_pyramid_from(samples: &[f32], first: usize) -> Vec<WaveformMipLevel> {
    let mut levels = Vec::new();
    let mut samples_per_bucket = first.max(1);
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

const PCM_SUBFORMAT_GUID: &[u8] = &[
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];
const IEEE_FLOAT_SUBFORMAT_GUID: &[u8] = &[
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

fn chunk_name(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            }
        })
        .collect()
}

fn format_guid(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
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

    fn append_chunk(bytes: &mut Vec<u8>, id: [u8; 4], data: &[u8]) {
        bytes.extend_from_slice(&id);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if data.len() % 2 == 1 {
            bytes.push(0);
        }
    }

    fn wav_with_chunks(chunks: Vec<([u8; 4], Vec<u8>)>, riff_size: Option<u32>) -> Vec<u8> {
        let riff_payload_len = chunks
            .iter()
            .map(|(_, data)| 8 + data.len() + (data.len() % 2))
            .sum::<usize>()
            + 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&riff_size.unwrap_or(riff_payload_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        for (id, data) in chunks {
            append_chunk(&mut bytes, id, &data);
        }
        bytes
    }

    fn pcm_fmt(format_code: u16, channels: u16, sample_rate: u32, bits_per_sample: u16) -> Vec<u8> {
        let block_align = channels * bits_per_sample.div_ceil(8);
        let byte_rate = sample_rate * block_align as u32;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&format_code.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&sample_rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&bits_per_sample.to_le_bytes());
        fmt
    }

    fn extensible_fmt(
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        guid: &[u8],
    ) -> Vec<u8> {
        let mut fmt = pcm_fmt(0xFFFE, channels, sample_rate, bits_per_sample);
        fmt.extend_from_slice(&22_u16.to_le_bytes());
        fmt.extend_from_slice(&bits_per_sample.to_le_bytes());
        fmt.extend_from_slice(&0_u32.to_le_bytes());
        fmt.extend_from_slice(guid);
        fmt
    }

    fn pcm16_wav(samples: &[i16]) -> Vec<u8> {
        let mut data = Vec::new();
        for sample in samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        wav_with_chunks(
            vec![
                (b"fmt ".to_owned(), pcm_fmt(1, 1, 44_100, 16)),
                (*b"data", data),
            ],
            None,
        )
    }

    #[test]
    fn pcm16_wav_loads_basic_metadata_and_peaks() {
        let samples = [0_i16, i16::MAX / 2, -i16::MAX / 2, i16::MAX / 4];
        let bytes = pcm16_wav(&samples);

        let path = std::env::temp_dir().join(format!(
            "eseqlisp-waveform-test-{}.wav",
            std::process::id()
        ));
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
    fn decode_pcm16_wav_returns_interleaved_samples() {
        let decoded = decode_wav_bytes(&pcm16_wav(&[0, i16::MAX, i16::MIN])).expect("decode wav");
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.frames, 3);
        assert_eq!(decoded.samples[0], 0.0);
        assert!(decoded.samples[1] > 0.99);
        assert_eq!(decoded.samples[2], -1.0);
    }

    #[test]
    fn decode_extensible_pcm24_wav() {
        let data = vec![0x00, 0x00, 0x00, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x80];
        let wav = wav_with_chunks(
            vec![
                (*b"fmt ", extensible_fmt(1, 48_000, 24, PCM_SUBFORMAT_GUID)),
                (*b"data", data),
            ],
            None,
        );
        let decoded = decode_wav_bytes(&wav).expect("decode extensible pcm24");
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.frames, 3);
        assert_eq!(decoded.samples[0], 0.0);
        assert!(decoded.samples[1] > 0.99);
        assert_eq!(decoded.samples[2], -1.0);
    }

    #[test]
    fn decode_extensible_float32_wav() {
        let mut data = Vec::new();
        for sample in [0.0_f32, 0.5, -2.0] {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let wav = wav_with_chunks(
            vec![
                (
                    *b"fmt ",
                    extensible_fmt(1, 96_000, 32, IEEE_FLOAT_SUBFORMAT_GUID),
                ),
                (*b"data", data),
            ],
            None,
        );
        let decoded = decode_wav_bytes(&wav).expect("decode extensible float32");
        assert_eq!(decoded.sample_rate, 96_000);
        assert_eq!(decoded.samples, vec![0.0, 0.5, -1.0]);
    }

    #[test]
    fn unknown_chunks_and_odd_padding_are_skipped() {
        let wav = wav_with_chunks(
            vec![
                (*b"JUNK", vec![1, 2, 3]),
                (*b"fmt ", pcm_fmt(1, 1, 44_100, 16)),
                (*b"LIST", vec![4, 5, 6, 7, 8]),
                (*b"data", vec![0, 0, 0, 64]),
                (*b"bext", vec![9, 10, 11]),
            ],
            None,
        );
        let decoded = decode_wav_bytes(&wav).expect("decode with metadata chunks");
        assert_eq!(decoded.frames, 2);
        assert_eq!(decoded.samples[0], 0.0);
        assert!(decoded.samples[1] > 0.49);
    }

    #[test]
    fn riff_size_mismatch_is_a_warning_not_a_hard_error() {
        let wav = wav_with_chunks(
            vec![
                (*b"fmt ", pcm_fmt(1, 1, 44_100, 16)),
                (*b"data", vec![0, 0]),
            ],
            Some(12),
        );
        let decoded = decode_wav_bytes(&wav).expect("decode size mismatch");
        assert_eq!(decoded.frames, 1);
        assert!(
            decoded
                .warnings
                .iter()
                .any(|warning| warning.contains("RIFF declared length"))
        );
    }

    #[test]
    fn trailing_incomplete_data_frame_is_ignored_with_warning() {
        let wav = wav_with_chunks(
            vec![
                (*b"fmt ", pcm_fmt(1, 1, 44_100, 16)),
                (*b"data", vec![0, 0, 0]),
            ],
            None,
        );
        let decoded = decode_wav_bytes(&wav).expect("decode trailing byte");
        assert_eq!(decoded.frames, 1);
        assert!(
            decoded
                .warnings
                .iter()
                .any(|warning| warning.contains("trailing byte"))
        );
    }

    #[test]
    fn unsupported_compressed_format_returns_clear_error() {
        let wav = wav_with_chunks(
            vec![
                (*b"fmt ", pcm_fmt(2, 1, 44_100, 16)),
                (*b"data", vec![0, 0]),
            ],
            None,
        );
        let error = decode_wav_bytes(&wav).expect_err("compressed wav should fail");
        assert!(error.contains("unsupported wav format 2"));
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
