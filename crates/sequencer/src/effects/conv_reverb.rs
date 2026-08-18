//! Convolution reverb IR preprocessing.
//!
//! The DSP itself is a dgenlisp patch (`conv_stereo`) that convolves the live
//! input against pre-FFT'd, partitioned impulse responses held in four mutable
//! tensors (`irL_re`, `irL_im`, `irR_re`, `irR_im`). This module turns an
//! arbitrary WAV file into the partitioned-spectrum float data that gets
//! memcpy'd into those tensors at runtime.
//!
//! We deliberately do NOT reimplement the FFT here. dgenlisp's `partition-ir`
//! operator computes the partitioned FFT in exactly the convention the
//! `partitioned-spectral-mac` operator expects, and emits the result as plain
//! floats in the compiled manifest's `tensorInitData`. So the pipeline is:
//!   decode WAV -> split channels -> resample to host SR -> normalize ->
//!   pad/truncate to K*hop samples -> temp f32 WAV -> `partition-ir` via the
//!   DGenLisp tool -> extract the two K*N float blocks (re, im) per channel.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// FFT size (must match `conv_stereo.lisp` `@N`).
pub const N: usize = 1024;
/// Hop / partition size (must match `conv_stereo.lisp` hop).
pub const HOP: usize = 512;
/// Number of partitions (must match the tensor shapes `[K, N]`).
pub const K: usize = 128;
/// IR length in samples that fills exactly K partitions of HOP samples each.
pub const IR_LEN: usize = K * HOP; // 65536  (~1.49 s @ 44.1k)
/// Length of one partitioned spectrum block (re or im), in floats.
pub const PART_LEN: usize = K * N; // 131072

/// Display name of the builtin effect. Recognized in the builtin add path,
/// where it routes through the dgenlisp compile/apply path with bundled source.
pub const NAME: &str = "Convolution Reverb";

/// The bundled dgenlisp DSP source for the convolution reverb. Compiled fresh
/// per instance (no caching — the dylib carries instance-specific scratch).
pub fn dsp_source() -> &'static str {
    include_str!("conv_reverb_dsp.lisp")
}

/// Persisted reference for the bundled default IR (Lexicon 300 Rich Plate).
pub const DEFAULT_IR_REF: &str = "lexicon-300-rich-plate";

/// Absolute path to the bundled default impulse response, if present. Shipped
/// in the crate under `assets/ir/` so a fresh Convolution Reverb has a sound
/// out of the box without touching the user's sample library.
pub fn default_ir_path() -> Option<std::path::PathBuf> {
    let p = crate::paths::sequencer_dir()
        .ok()?
        .join("assets/ir/lexicon-300-rich-plate.wav");
    p.exists().then_some(p)
}

/// Partitioned-spectrum IR for one channel: K*N reals and K*N imags.
#[derive(Clone, Debug)]
pub struct ChannelIr {
    pub re: Vec<f32>,
    pub im: Vec<f32>,
}

/// True-stereo partitioned IR ready to memcpy into the effect's tensors.
#[derive(Clone, Debug)]
pub struct StereoIr {
    pub left: ChannelIr,
    pub right: ChannelIr,
}

/// Decode `path`, resample to `host_sr`, normalize, partition, and return the
/// per-channel partitioned spectra. Mono files are applied to both channels;
/// files with >2 channels use channels 0 and 1.
pub fn prepare_ir(path: &Path, host_sr: u32) -> Result<StereoIr, String> {
    if let Some(ir) = try_read_cached_ir(path, host_sr)? {
        return Ok(ir);
    }
    let ir = prepare_ir_uncached(path, host_sr)?;
    if let Err(error) = write_cached_ir(path, host_sr, &ir) {
        eprintln!("[convolution reverb] failed to write IR cache: {error}");
    }
    Ok(ir)
}

fn prepare_ir_uncached(path: &Path, host_sr: u32) -> Result<StereoIr, String> {
    let (interleaved, src_sr, channels) = decode_wav(path)?;
    if interleaved.is_empty() {
        return Err("IR file contains no samples".to_string());
    }

    let (mut left, mut right) = split_channels(&interleaved, channels);

    if (src_sr - host_sr as f32).abs() > 0.5 {
        left = resample(&left, src_sr, host_sr as f32);
        right = resample(&right, src_sr, host_sr as f32);
    }

    // Shared peak normalization across both channels preserves the stereo image.
    let peak = left
        .iter()
        .chain(right.iter())
        .fold(0.0f32, |m, &s| m.max(s.abs()));
    if peak > 1e-9 {
        let g = 1.0 / peak;
        for s in left.iter_mut().chain(right.iter_mut()) {
            *s *= g;
        }
    }

    let left_ir = partition_channel(&fit_to_ir_len(&left), host_sr)?;
    let right_ir = partition_channel(&fit_to_ir_len(&right), host_sr)?;
    Ok(StereoIr {
        left: left_ir,
        right: right_ir,
    })
}

const IR_CACHE_MAGIC: &[u8; 8] = b"ESEQIR01";

fn ir_cache_root() -> PathBuf {
    crate::paths::workspace_root()
        .join(".eseq")
        .join("dgenlisp-cache")
        .join("ir-prep")
}

fn ir_cache_key(path: &Path, host_sr: u32) -> Result<String, String> {
    let ir_bytes = std::fs::read(path).map_err(|e| format!("read IR for cache key: {e}"))?;
    let tool = crate::lisp_host::dgenlisp_tool_path();
    let tool_bytes = std::fs::read(&tool).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(b"eseq-conv-ir-cache-v1");
    hasher.update(host_sr.to_le_bytes());
    hasher.update((N as u64).to_le_bytes());
    hasher.update((HOP as u64).to_le_bytes());
    hasher.update((K as u64).to_le_bytes());
    hasher.update((ir_bytes.len() as u64).to_le_bytes());
    hasher.update(sha256_bytes(&ir_bytes));
    hasher.update(tool.to_string_lossy().as_bytes());
    hasher.update((tool_bytes.len() as u64).to_le_bytes());
    hasher.update(sha256_bytes(&tool_bytes));
    let digest = hasher.finalize();
    Ok(hex_digest(&digest))
}

fn try_read_cached_ir(path: &Path, host_sr: u32) -> Result<Option<StereoIr>, String> {
    let key = ir_cache_key(path, host_sr)?;
    let cache_path = ir_cache_root().join(key).join("ir.bin");
    let Ok(mut file) = std::fs::File::open(&cache_path) else {
        return Ok(None);
    };
    match read_cached_ir_file(&mut file, host_sr) {
        Ok(ir) => Ok(Some(ir)),
        Err(error) => {
            eprintln!(
                "[convolution reverb] ignoring invalid IR cache {}: {error}",
                cache_path.display()
            );
            Ok(None)
        }
    }
}

fn write_cached_ir(path: &Path, host_sr: u32, ir: &StereoIr) -> Result<(), String> {
    let key = ir_cache_key(path, host_sr)?;
    let dir = ir_cache_root().join(key);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create IR cache dir: {e}"))?;
    let tmp = dir.join(format!("ir-{}-{}.tmp", std::process::id(), next_seq()));
    {
        let mut file =
            std::fs::File::create(&tmp).map_err(|e| format!("create IR cache temp file: {e}"))?;
        write_cached_ir_file(&mut file, host_sr, ir)?;
        file.flush()
            .map_err(|e| format!("flush IR cache temp file: {e}"))?;
    }
    std::fs::rename(&tmp, dir.join("ir.bin")).map_err(|e| format!("commit IR cache file: {e}"))
}

fn write_cached_ir_file<W: Write>(
    mut writer: W,
    host_sr: u32,
    ir: &StereoIr,
) -> Result<(), String> {
    writer
        .write_all(IR_CACHE_MAGIC)
        .map_err(|e| format!("write IR cache magic: {e}"))?;
    for value in [N as u32, HOP as u32, K as u32, host_sr, PART_LEN as u32] {
        writer
            .write_all(&value.to_le_bytes())
            .map_err(|e| format!("write IR cache header: {e}"))?;
    }
    write_f32_block(&mut writer, &ir.left.re)?;
    write_f32_block(&mut writer, &ir.left.im)?;
    write_f32_block(&mut writer, &ir.right.re)?;
    write_f32_block(&mut writer, &ir.right.im)?;
    Ok(())
}

fn read_cached_ir_file<R: Read>(mut reader: R, host_sr: u32) -> Result<StereoIr, String> {
    let mut magic = [0_u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(|e| format!("read IR cache magic: {e}"))?;
    if &magic != IR_CACHE_MAGIC {
        return Err("bad IR cache magic".to_string());
    }
    let n = read_u32(&mut reader)?;
    let hop = read_u32(&mut reader)?;
    let k = read_u32(&mut reader)?;
    let sr = read_u32(&mut reader)?;
    let part_len = read_u32(&mut reader)?;
    if n != N as u32 || hop != HOP as u32 || k != K as u32 || sr != host_sr {
        return Err("IR cache dimensions do not match current runtime".to_string());
    }
    if part_len != PART_LEN as u32 {
        return Err("IR cache partition length does not match current runtime".to_string());
    }
    Ok(StereoIr {
        left: ChannelIr {
            re: read_f32_block(&mut reader, PART_LEN)?,
            im: read_f32_block(&mut reader, PART_LEN)?,
        },
        right: ChannelIr {
            re: read_f32_block(&mut reader, PART_LEN)?,
            im: read_f32_block(&mut reader, PART_LEN)?,
        },
    })
}

fn write_f32_block<W: Write>(writer: &mut W, data: &[f32]) -> Result<(), String> {
    if data.len() != PART_LEN {
        return Err(format!(
            "IR cache block has {} floats, expected {PART_LEN}",
            data.len()
        ));
    }
    for value in data {
        writer
            .write_all(&value.to_le_bytes())
            .map_err(|e| format!("write IR cache block: {e}"))?;
    }
    Ok(())
}

fn read_f32_block<R: Read>(reader: &mut R, len: usize) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(len);
    let mut bytes = [0_u8; 4];
    for _ in 0..len {
        reader
            .read_exact(&mut bytes)
            .map_err(|e| format!("read IR cache block: {e}"))?;
        out.push(f32::from_le_bytes(bytes));
    }
    Ok(out)
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, String> {
    let mut bytes = [0_u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| format!("read IR cache header: {e}"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

// ── WAV decode ──

fn decode_wav(path: &Path) -> Result<(Vec<f32>, f32, usize), String> {
    let reader = hound::WavReader::open(path).map_err(|e| format!("open IR {path:?}: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let sr = spec.sample_rate as f32;
    let mut reader = reader;
    let samples: Result<Vec<f32>, String> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map_err(|e| format!("read IR sample: {e}")))
            .collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| {
                    s.map(|v| v as f32 / max)
                        .map_err(|e| format!("read IR sample: {e}"))
                })
                .collect()
        }
    };
    Ok((samples?, sr, channels))
}

fn split_channels(interleaved: &[f32], channels: usize) -> (Vec<f32>, Vec<f32>) {
    if channels <= 1 {
        return (interleaved.to_vec(), interleaved.to_vec());
    }
    let frames = interleaved.len() / channels;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for f in 0..frames {
        left.push(interleaved[f * channels]);
        right.push(interleaved[f * channels + 1]);
    }
    (left, right)
}

/// Pad with zeros or truncate (with a short fade-out to avoid a click) so the
/// channel is exactly `IR_LEN` samples.
fn fit_to_ir_len(samples: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; IR_LEN];
    let n = samples.len().min(IR_LEN);
    out[..n].copy_from_slice(&samples[..n]);
    if samples.len() > IR_LEN {
        // Truncated: fade the last 256 samples to zero to suppress the edge click.
        let fade = 256.min(IR_LEN);
        for i in 0..fade {
            let g = (fade - 1 - i) as f32 / fade as f32;
            out[IR_LEN - fade + i] *= g;
        }
    }
    out
}

// ── Windowed-sinc resampler (offline, one-shot per IR load) ──

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Blackman window evaluated at normalized position `p` in [-1, 1].
fn blackman(p: f64) -> f64 {
    if p.abs() >= 1.0 {
        0.0
    } else {
        let t = std::f64::consts::PI * p;
        0.42 + 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
    }
}

/// Band-limited windowed-sinc resampler. Quality is plenty for offline IR prep.
fn resample(input: &[f32], src_sr: f32, dst_sr: f32) -> Vec<f32> {
    if input.is_empty() || (src_sr - dst_sr).abs() < 1e-3 {
        return input.to_vec();
    }
    let ratio = (dst_sr / src_sr) as f64;
    let out_len = ((input.len() as f64) * ratio).round().max(1.0) as usize;
    // When downsampling, lower the cutoff to the new Nyquist to avoid aliasing.
    let cutoff = if ratio < 1.0 { ratio } else { 1.0 };
    let taps = 16.0_f64; // zero-crossings each side at full cutoff
    let support = (taps / cutoff).ceil() as isize;

    let mut out = vec![0.0f32; out_len];
    for (o, slot) in out.iter_mut().enumerate() {
        let center = o as f64 / ratio; // position in source samples
        let i0 = center.floor() as isize;
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        for k in (i0 - support)..=(i0 + support) {
            if k < 0 || k as usize >= input.len() {
                continue;
            }
            let dist = center - k as f64;
            let w = blackman(dist / support as f64) * sinc(dist * cutoff) * cutoff;
            acc += input[k as usize] as f64 * w;
            wsum += w;
        }
        *slot = if wsum.abs() > 1e-12 {
            (acc / wsum) as f32
        } else {
            acc as f32
        };
    }
    out
}

// ── partition-ir via the DGenLisp tool ──

fn ir_prep_dir() -> PathBuf {
    crate::app_paths::app_paths().ir_prep_dir()
}

fn tool_path() -> PathBuf {
    crate::app_paths::app_paths().dgenlisp_tool()
}

/// Write `samples` (exactly IR_LEN) to a temp f32 WAV, run it through
/// `partition-ir`, and extract the two K*N float blocks (re, im).
fn partition_channel(samples: &[f32], host_sr: u32) -> Result<ChannelIr, String> {
    debug_assert_eq!(samples.len(), IR_LEN);
    let dir = ir_prep_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create IR prep dir: {e}"))?;

    // Unique names so concurrent / repeated loads never collide.
    let seq = next_seq();
    let wav_path = dir.join(format!("ir_{seq}.wav"));
    let lisp_path = dir.join(format!("ir_{seq}.lisp"));

    write_f32_wav(&wav_path, samples, host_sr)?;

    let abs_wav = wav_path.canonicalize().unwrap_or(wav_path.clone());
    let src = format!(
        "(def (ir-re ir-im) (partition-ir (ir @file \"{}\") @N {N} @hop {HOP}))\n\
         (out (+ (peek ir-re 0) (peek ir-im 0)) 1 @name probe)\n",
        abs_wav.to_string_lossy()
    );
    std::fs::write(&lisp_path, &src).map_err(|e| format!("write IR prep lisp: {e}"))?;

    // Same hermetic toolchain hand-off as the effect/instrument compile path
    // (impl spec, slice E2): staged root passed unconditionally, preflighted,
    // no system-compiler fallback.
    let toolchain_root = crate::app_paths::app_paths().dgen_toolchain_root_checked()?;
    let out_name = format!("ir_{seq}");
    let output = std::process::Command::new(tool_path())
        .args(["compile", lisp_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", &out_name])
        .args(["--sample-rate", &host_sr.to_string()])
        .arg("--toolchain-root")
        .arg(&toolchain_root)
        // Skip DGenLisp's inline shell audit (nm/otool, a Command Line Tools
        // dependency). No Rust audit replaces it here: only the manifest's
        // tensorInitData is consumed — the dylib byproduct is deleted below,
        // never loaded.
        .arg("--skip-inline-audit")
        .output()
        .map_err(|e| format!("run DGenLisp for IR prep: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "IR partition-ir failed: {}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ));
    }

    let (re, im) = extract_partition_blocks(&String::from_utf8_lossy(&output.stdout))?;

    // Best-effort cleanup of the scratch files.
    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&lisp_path);
    let _ = std::fs::remove_file(dir.join(format!("{out_name}.dylib")));
    let _ = std::fs::remove_file(dir.join(format!("{out_name}.json")));
    let _ = std::fs::remove_file(dir.join(format!("{out_name}.c")));

    Ok(ChannelIr { re, im })
}

/// Pull the two `K*N`-length blocks out of the manifest's `tensorInitData`.
/// `partition-ir` allocates the real tensor before the imaginary one, so they
/// appear in that order; the raw IR samples block (length IR_LEN) is ignored.
fn extract_partition_blocks(manifest_json: &str) -> Result<(Vec<f32>, Vec<f32>), String> {
    let v: serde_json::Value =
        serde_json::from_str(manifest_json).map_err(|e| format!("parse IR manifest: {e}"))?;
    let init = v["tensorInitData"]
        .as_array()
        .ok_or("IR manifest missing tensorInitData")?;

    let mut blocks: Vec<Vec<f32>> = Vec::new();
    for entry in init {
        let data = match entry["data"].as_array() {
            Some(d) => d,
            None => continue,
        };
        if data.len() != PART_LEN {
            continue;
        }
        blocks.push(
            data.iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect(),
        );
    }

    if blocks.len() < 2 {
        return Err(format!(
            "expected 2 partition blocks of {PART_LEN} floats, found {}",
            blocks.len()
        ));
    }
    let im = blocks.pop().unwrap();
    let re = blocks.pop().unwrap();
    Ok((re, im))
}

/// Write a canonical mono 32-bit IEEE-float WAV (audioFormat 3). We build the
/// header by hand because hound emits a WAVE_FORMAT_EXTENSIBLE header (format
/// 65534) that dgenlisp's WAV parser rejects.
fn write_f32_wav(path: &Path, samples: &[f32], sr: u32) -> Result<(), String> {
    let data_bytes = (samples.len() * 4) as u32;
    let byte_rate = sr * 4; // mono, 4 bytes/sample
    let mut buf: Vec<u8> = Vec::with_capacity(44 + data_bytes as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    buf.extend_from_slice(&3u16.to_le_bytes()); // audioFormat = IEEE float
    buf.extend_from_slice(&1u16.to_le_bytes()); // channels
    buf.extend_from_slice(&sr.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&4u16.to_le_bytes()); // block align
    buf.extend_from_slice(&32u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, &buf).map_err(|e| format!("write temp IR wav: {e}"))
}

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

// ── Applying a prepared IR to a live effect node ──

/// Manifest tensor offsets (float indices) for the four IR spectra of a live
/// Convolution Reverb instance. Resolved from the compiled manifest's
/// `tensors[]` by name and recorded per effect slot.
#[derive(Clone, Copy, Debug)]
pub struct StereoIrSlots {
    pub l_re: usize,
    pub l_im: usize,
    pub r_re: usize,
    pub r_im: usize,
}

// Per-instance state, keyed by the effect's audiograph `node_id` (stable and
// available wherever we touch the instance). Populated when a Convolution
// Reverb instance is created and when its IR changes.
type NodeMap<V> = std::sync::Mutex<std::collections::BTreeMap<i32, V>>;

/// Tensor offsets (where to bulk-write the IR) per live instance.
static IR_SLOTS: NodeMap<StereoIrSlots> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// Persisted IR reference (sample hash / stem) per live instance.
static IR_REFS: NodeMap<String> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// Human-readable IR name for the UI label per live instance.
static IR_NAMES: NodeMap<String> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// Prepared IR data retained for exact in-session undo/redo without another
/// filesystem lookup or decode pass.
static IR_DATA: NodeMap<std::sync::Arc<StereoIr>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Record the tensor offsets for a live instance.
pub fn record_ir_slots(node_id: i32, slots: StereoIrSlots) {
    if let Ok(mut map) = IR_SLOTS.lock() {
        map.insert(node_id, slots);
    }
}

/// Look up the tensor offsets for a live instance.
pub fn ir_slots_for(node_id: i32) -> Option<StereoIrSlots> {
    IR_SLOTS
        .lock()
        .ok()
        .and_then(|map| map.get(&node_id).copied())
}

/// Remember which IR (reference + display name) an instance currently has.
fn record_ir(node_id: i32, reference: &str, display_name: &str) {
    if let Ok(mut map) = IR_REFS.lock() {
        map.insert(node_id, reference.to_string());
    }
    if let Ok(mut map) = IR_NAMES.lock() {
        map.insert(node_id, display_name.to_string());
    }
}

pub fn record_prepared_ir(
    node_id: i32,
    reference: &str,
    display_name: &str,
    ir: std::sync::Arc<StereoIr>,
) {
    record_ir(node_id, reference, display_name);
    if let Ok(mut map) = IR_DATA.lock() {
        map.insert(node_id, ir);
    }
}

pub fn prepared_ir_for(node_id: i32) -> Option<std::sync::Arc<StereoIr>> {
    IR_DATA
        .lock()
        .ok()
        .and_then(|map| map.get(&node_id).cloned())
}

/// The persisted IR reference currently loaded for an instance, if any.
pub fn ir_ref_for(node_id: i32) -> Option<String> {
    IR_REFS
        .lock()
        .ok()
        .and_then(|map| map.get(&node_id).cloned())
}

/// The human-readable IR name for an instance, if any.
pub fn ir_name_for(node_id: i32) -> Option<String> {
    IR_NAMES
        .lock()
        .ok()
        .and_then(|map| map.get(&node_id).cloned())
}

/// Forget an instance's IR state (on effect removal / replacement).
pub fn clear_instance(node_id: i32) {
    if let Ok(mut map) = IR_SLOTS.lock() {
        map.remove(&node_id);
    }
    if let Ok(mut map) = IR_REFS.lock() {
        map.remove(&node_id);
    }
    if let Ok(mut map) = IR_NAMES.lock() {
        map.remove(&node_id);
    }
    if let Ok(mut map) = IR_DATA.lock() {
        map.remove(&node_id);
    }
}

impl StereoIrSlots {
    /// Pull the four IR tensor offsets out of a compiled manifest by name.
    pub fn from_manifest(m: &crate::lisp_host::DGenManifest) -> Option<Self> {
        let find = |name: &str| {
            m.tensors
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.cell_offset)
        };
        Some(Self {
            l_re: find("irL_re")?,
            l_im: find("irL_im")?,
            r_re: find("irR_re")?,
            r_im: find("irR_im")?,
        })
    }
}

/// Queue the four bulk writes that load a prepared stereo IR into a live
/// Convolution Reverb node. The engine applies them at a block boundary; the
/// data is copied internally so `ir` may be dropped immediately after.
///
/// # Safety
/// `lg` must be a valid live graph and `node_id` a live dgenlisp effect node.
pub unsafe fn apply_ir_to_node(
    lg: *mut crate::audiograph::LiveGraph,
    node_id: i32,
    slots: &StereoIrSlots,
    ir: &StereoIr,
) -> Result<(), String> {
    use crate::lisp_host::queue_tensor_write;
    let ok = queue_tensor_write(lg, node_id, slots.l_re, &ir.left.re)
        && queue_tensor_write(lg, node_id, slots.l_im, &ir.left.im)
        && queue_tensor_write(lg, node_id, slots.r_re, &ir.right.re)
        && queue_tensor_write(lg, node_id, slots.r_im, &ir.right.im);
    if ok {
        Ok(())
    } else {
        Err("failed to queue IR write (graph edit queue full)".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_length_ratio_and_dc() {
        // A constant signal should stay ~constant through the resampler.
        let input = vec![0.5f32; 1000];
        let out = resample(&input, 48000.0, 44100.0);
        let expected = (1000.0_f64 * 44100.0 / 48000.0).round() as usize;
        assert_eq!(out.len(), expected);
        // Interior samples should be very close to the DC level.
        let mid = out[out.len() / 2];
        assert!((mid - 0.5).abs() < 1e-3, "DC not preserved: {mid}");
    }

    #[test]
    fn fit_pads_and_truncates_to_ir_len() {
        assert_eq!(fit_to_ir_len(&[1.0, 2.0]).len(), IR_LEN);
        assert_eq!(fit_to_ir_len(&vec![1.0; IR_LEN * 2]).len(), IR_LEN);
    }

    #[test]
    fn ir_cache_binary_roundtrip_preserves_blocks() {
        let block = |seed: f32| {
            (0..PART_LEN)
                .map(|idx| seed + (idx as f32 * 0.000_001))
                .collect::<Vec<_>>()
        };
        let ir = StereoIr {
            left: ChannelIr {
                re: block(0.1),
                im: block(0.2),
            },
            right: ChannelIr {
                re: block(0.3),
                im: block(0.4),
            },
        };

        let mut bytes = Vec::new();
        write_cached_ir_file(&mut bytes, 44_100, &ir).expect("write cache");
        let restored = read_cached_ir_file(&bytes[..], 44_100).expect("read cache");

        assert_eq!(restored.left.re[123], ir.left.re[123]);
        assert_eq!(restored.left.im[456], ir.left.im[456]);
        assert_eq!(restored.right.re[789], ir.right.re[789]);
        assert_eq!(restored.right.im[1024], ir.right.im[1024]);
    }

    // Compiles the bundled DSP through the real effect path (preamble included)
    // and checks the four IR tensors are present at distinct offsets.
    #[test]
    fn bundled_dsp_compiles_with_ir_tensors() {
        if !tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found at {:?}", tool_path());
            return;
        }
        let json = crate::lisp_host::compile_lisp(dsp_source(), 44100)
            .expect("compile bundled conv reverb DSP");
        let manifest = crate::lisp_host::parse_manifest(&json).expect("parse manifest");
        let slots = StereoIrSlots::from_manifest(&manifest)
            .expect("manifest should expose irL_re/irL_im/irR_re/irR_im");
        // All four offsets distinct.
        let offs = [slots.l_re, slots.l_im, slots.r_re, slots.r_im];
        for i in 0..offs.len() {
            for j in (i + 1)..offs.len() {
                assert_ne!(offs[i], offs[j], "tensor offsets must be distinct");
            }
        }
        assert_eq!(manifest.n_inputs, 2);
        assert_eq!(manifest.n_outputs, 2);
    }

    // The bundled default IR (48k/24-bit/stereo) must prep cleanly: exercises
    // channel split + resample + partition. Skips if tool/asset absent.
    #[test]
    fn default_ir_prepares() {
        if !tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }
        let Some(path) = default_ir_path() else {
            eprintln!("skipping: bundled default IR not found");
            return;
        };
        let ir = prepare_ir(&path, 44100).expect("prepare default IR");
        assert_eq!(ir.left.re.len(), PART_LEN);
        assert_eq!(ir.right.im.len(), PART_LEN);
        // Stereo source: L and R partitions should differ.
        assert!(
            ir.left.re != ir.right.re,
            "expected distinct L/R IRs from a stereo file"
        );
        let nz = ir.left.re.iter().filter(|&&x| x != 0.0).count();
        assert!(nz > PART_LEN / 2, "default IR left.re mostly zero");
    }

    // End-to-end: requires the DGenLisp tool and a sample WAV. Skips if absent.
    #[test]
    fn prepare_ir_end_to_end() {
        if !tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found at {:?}", tool_path());
            return;
        }
        let wav = std::fs::read_dir("samples.backup.1779974926")
            .ok()
            .and_then(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .find(|p| p.extension().map(|x| x == "wav").unwrap_or(false))
            });
        let Some(wav) = wav else {
            eprintln!("skipping: no sample wav found");
            return;
        };
        let ir = prepare_ir(&wav, 44100).expect("prepare_ir");
        assert_eq!(ir.left.re.len(), PART_LEN);
        assert_eq!(ir.left.im.len(), PART_LEN);
        assert_eq!(ir.right.re.len(), PART_LEN);
        assert_eq!(ir.right.im.len(), PART_LEN);
        let nonzero = ir.left.re.iter().filter(|&&x| x != 0.0).count();
        assert!(nonzero > PART_LEN / 2, "left.re mostly zero: {nonzero}");
    }
}
