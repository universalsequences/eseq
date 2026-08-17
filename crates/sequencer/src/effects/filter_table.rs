//! Filter Table magnitude-bank preprocessing and live-instance state.
//!
//! User audio is analyzed under an explicit [`AnalysisMode`] — structured
//! wavetable, single cycle, audio texture, or impulse response — each with its
//! own framing, windowing, DC, and normalization policy (see the per-mode
//! `analyze_*` functions). Every mode produces the same validated 64x1025
//! magnitude bank. Only magnitudes enter the live effect: zero phase is
//! synthesized by the DSP so interpolation between frames cannot suffer
//! complex-phase cancellation.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

pub const NAME: &str = "Filter Table";
/// DSP param whose descriptor is marked `ParamScaling::Exponential` (and whose
/// panel knob uses the log taper): 40–18000 Hz wants equal arc per octave.
pub const PARAM_CUTOFF: &str = "cutoff";
pub const N: usize = 2048;
pub const NBINS: usize = N / 2 + 1;
pub const FRAMES: usize = 64;
pub const TABLE_LEN: usize = FRAMES * NBINS;
pub const DEFAULT_TABLE_REF: &str = "builtin-filter-table";

pub fn visualization_key(node_id: i32) -> String {
    format!("filter-table:{node_id}:magnitudes")
}

pub fn dsp_source() -> &'static str {
    include_str!("filter_table_dsp.lisp")
}

#[derive(Clone, Debug)]
pub struct MagnitudeTable {
    pub data: Arc<Vec<f32>>,
}

impl MagnitudeTable {
    pub(crate) fn new(data: Vec<f32>) -> Result<Self, String> {
        if data.len() != TABLE_LEN {
            return Err(format!(
                "filter table has {} values, expected {TABLE_LEN}",
                data.len()
            ));
        }
        if data.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return Err("filter table contains invalid magnitudes".to_string());
        }
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

/// Table harmonic that the `cutoff` parameter pins to its own frequency: the
/// DSP maps input bin frequency `f` to table position `REFERENCE_HARMONIC *
/// f / cutoff` (see `filter_table_dsp.lisp`, which hard-codes the same value).
/// This is a global constant for now; per-asset reference semantics are
/// planned as part of the versioned asset format (eseq-dtx.6).
pub const REFERENCE_HARMONIC: usize = 24;

/// How a dropped source is turned into the 64x1025 magnitude bank. The four
/// modes are mathematically different inputs and carry different framing,
/// windowing, DC, and normalization policies:
///
/// | mode            | framing                     | window       | DC      | amplitude norm |
/// |-----------------|-----------------------------|--------------|---------|----------------|
/// | Wavetable       | whole aligned 2048 cycles   | rectangular  | kept    | per-frame RMS  |
/// | SingleCycle     | one periodic resample       | rectangular  | kept    | RMS            |
/// | AudioTexture    | 64 evenly spaced in time    | Hann         | removed | per-frame RMS  |
/// | ImpulseResponse | first 2048 samples (faded)  | rectangular  | kept    | none           |
///
/// Every mode ends with per-frame spectral peak normalization so the DSP sees
/// magnitudes in 0..=1. Automatic detection ([`recommend_mode`]) only ever
/// proposes a mode — the chosen mode is surfaced in the UI, persisted in the
/// table reference ([`encode_table_ref`]), and can be switched after import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisMode {
    Wavetable,
    SingleCycle,
    AudioTexture,
    ImpulseResponse,
}

impl AnalysisMode {
    pub const ALL: [AnalysisMode; 4] = [
        AnalysisMode::Wavetable,
        AnalysisMode::SingleCycle,
        AnalysisMode::AudioTexture,
        AnalysisMode::ImpulseResponse,
    ];

    /// Stable identifier used in persisted table references and host commands.
    pub fn tag(self) -> &'static str {
        match self {
            AnalysisMode::Wavetable => "wavetable",
            AnalysisMode::SingleCycle => "single-cycle",
            AnalysisMode::AudioTexture => "audio",
            AnalysisMode::ImpulseResponse => "impulse-response",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.tag() == tag)
    }

    /// Short display label for panel UI.
    pub fn label(self) -> &'static str {
        match self {
            AnalysisMode::Wavetable => "Wavetable",
            AnalysisMode::SingleCycle => "Single Cycle",
            AnalysisMode::AudioTexture => "Audio",
            AnalysisMode::ImpulseResponse => "Impulse",
        }
    }

    /// Cycle order for the panel's mode-switch affordance.
    pub fn next(self) -> Self {
        match self {
            AnalysisMode::Wavetable => AnalysisMode::SingleCycle,
            AnalysisMode::SingleCycle => AnalysisMode::AudioTexture,
            AnalysisMode::AudioTexture => AnalysisMode::ImpulseResponse,
            AnalysisMode::ImpulseResponse => AnalysisMode::Wavetable,
        }
    }
}

/// Recommend an analysis mode from the mono source length. Exact multiples of
/// the 2048-sample cycle with at least two cycles read as structured
/// wavetables; anything up to two cycles long reads as one periodic cycle;
/// longer non-multiple material reads as ordinary audio. Impulse responses are
/// never auto-detected — an IR is indistinguishable from short audio, so that
/// mode is only ever chosen explicitly.
pub fn recommend_mode(mono_len: usize) -> AnalysisMode {
    if mono_len >= 2 * N && mono_len % N == 0 {
        AnalysisMode::Wavetable
    } else if mono_len <= 2 * N {
        AnalysisMode::SingleCycle
    } else {
        AnalysisMode::AudioTexture
    }
}

/// Separator that embeds the analysis mode in a persisted table reference
/// (`"<sample-ref>#ft-mode=<tag>"`). Riding in the existing reference string
/// keeps project serialization, undo mementos, and history labels unchanged
/// while making a reload reproduce the exact analysis. Bare references from
/// older projects decode with no mode and fall back to [`recommend_mode`].
const MODE_REF_SEPARATOR: &str = "#ft-mode=";

pub fn encode_table_ref(sample_ref: &str, mode: AnalysisMode) -> String {
    format!("{sample_ref}{MODE_REF_SEPARATOR}{}", mode.tag())
}

/// Split a persisted table reference into the sample reference and the
/// analysis mode it was imported with. Unknown or missing mode suffixes leave
/// the whole string as the sample reference.
pub fn decode_table_ref(reference: &str) -> (&str, Option<AnalysisMode>) {
    if let Some(split) = reference.rfind(MODE_REF_SEPARATOR) {
        let (sample_ref, suffix) = reference.split_at(split);
        if let Some(mode) = AnalysisMode::from_tag(&suffix[MODE_REF_SEPARATOR.len()..]) {
            return (sample_ref, Some(mode));
        }
    }
    (reference, None)
}

/// Analyze an audio file under the recommended mode. Returns the table and the
/// mode actually used so callers can persist it.
pub fn prepare_table(path: &Path) -> Result<(MagnitudeTable, AnalysisMode), String> {
    let mono = decode_mono(path)?;
    let mode = recommend_mode(mono.len());
    analyze(&mono, mode).map(|table| (table, mode))
}

/// Analyze an audio file under an explicitly requested mode.
pub fn prepare_table_with_mode(path: &Path, mode: AnalysisMode) -> Result<MagnitudeTable, String> {
    let mono = decode_mono(path)?;
    analyze(&mono, mode)
}

/// Deterministic stereo policy shared by every mode: channels average into one
/// mono signal before analysis.
fn decode_mono(path: &Path) -> Result<Vec<f32>, String> {
    let decoded = crate::sample_import::decode_audio_file(path)?;
    let channels = usize::from(decoded.channels.max(1));
    let mut mono = Vec::with_capacity(decoded.samples.len() / channels);
    for frame in decoded.samples.chunks_exact(channels) {
        mono.push(frame.iter().copied().sum::<f32>() / channels as f32);
    }
    if mono.is_empty() {
        return Err("table source contains no complete audio frames".to_string());
    }
    Ok(mono)
}

fn analyze(mono: &[f32], mode: AnalysisMode) -> Result<MagnitudeTable, String> {
    match mode {
        AnalysisMode::Wavetable => analyze_wavetable(mono),
        AnalysisMode::SingleCycle => analyze_single_cycle(mono),
        AnalysisMode::AudioTexture => analyze_audio_texture(mono),
        AnalysisMode::ImpulseResponse => analyze_impulse_response(mono),
    }
}

/// Structured wavetable: the source must be whole aligned 2048-sample cycles.
/// Table frame f maps to fractional cycle position f*(cycles-1)/63 and is the
/// sample-wise linear interpolation of the two adjacent whole cycles; no
/// analysis window may straddle a cycle boundary, and no window function is
/// applied because each frame is exactly periodic. DC is kept: reference
/// tables carry deliberate DC energy.
fn analyze_wavetable(mono: &[f32]) -> Result<MagnitudeTable, String> {
    if mono.len() < N || mono.len() % N != 0 {
        return Err(format!(
            "wavetable analysis requires whole 2048-sample cycles, but the source has {} samples; \
             use single-cycle or audio analysis instead",
            mono.len()
        ));
    }
    let cycles = mono.len() / N;
    let mut work = vec![0.0f32; N];
    let mut audible = false;
    let table = spectrum_table(|table_frame, frame_out| {
        let position = if cycles == 1 {
            0.0
        } else {
            table_frame as f64 * (cycles - 1) as f64 / (FRAMES - 1) as f64
        };
        let lo = (position.floor() as usize).min(cycles - 1);
        let hi = (lo + 1).min(cycles - 1);
        let fraction = (position - lo as f64) as f32;
        let lo_cycle = &mono[lo * N..(lo + 1) * N];
        let hi_cycle = &mono[hi * N..(hi + 1) * N];
        for (index, value) in work.iter_mut().enumerate() {
            *value = lo_cycle[index] + (hi_cycle[index] - lo_cycle[index]) * fraction;
        }
        if rms_normalize(&mut work, KeepDc::Keep) {
            audible = true;
            frame_out.copy_from_slice(&work);
            true
        } else {
            false
        }
    })?;
    if !audible {
        return Err("wavetable source is silent".to_string());
    }
    Ok(table)
}

/// Single cycle: the whole source is one period, linearly resampled to the
/// 2048-sample analysis cycle and repeated across all 64 frames. Sources
/// longer than one cycle are decimated by the same linear resampler (aliasing
/// from over-length cycles is accepted and documented). Rectangular window, DC
/// kept.
fn analyze_single_cycle(mono: &[f32]) -> Result<MagnitudeTable, String> {
    let mut cycle = vec![0.0f32; N];
    let scale = mono.len() as f64 / N as f64;
    for (index, value) in cycle.iter_mut().enumerate() {
        let position = index as f64 * scale;
        let lo = position.floor() as usize % mono.len();
        let hi = (lo + 1) % mono.len();
        let fraction = (position - position.floor()) as f32;
        *value = mono[lo] + (mono[hi] - mono[lo]) * fraction;
    }
    if !rms_normalize(&mut cycle, KeepDc::Keep) {
        return Err("single-cycle source is silent".to_string());
    }
    spectrum_table(|_, frame_out| {
        frame_out.copy_from_slice(&cycle);
        true
    })
}

/// Ordinary audio: 64 analysis windows evenly spaced in time across the
/// source, each Hann-windowed to control spectral leakage (raw rectangular
/// windows are only correct for known periodic cycles). The window mean is
/// removed before windowing so window-induced DC bias cannot masquerade as
/// filter DC gain. No spectral smoothing beyond the window main lobe is
/// applied here; perceptual smoothing is tracked separately (eseq-dtx.3), and
/// novelty/onset-informed framing is a documented future extension.
fn analyze_audio_texture(mono: &[f32]) -> Result<MagnitudeTable, String> {
    let window = hann_window();
    let mut padded;
    let source = if mono.len() < N {
        padded = mono.to_vec();
        padded.resize(N, 0.0);
        padded.as_slice()
    } else {
        mono
    };
    let max_start = source.len() - N;
    let mut work = vec![0.0f32; N];
    let mut audible = false;
    let table = spectrum_table(|table_frame, frame_out| {
        let start = ((table_frame as u128 * max_start as u128) / (FRAMES - 1) as u128) as usize;
        work.copy_from_slice(&source[start..start + N]);
        if !rms_normalize(&mut work, KeepDc::Remove) {
            return false;
        }
        audible = true;
        for (value, weight) in work.iter_mut().zip(&window) {
            *value *= weight;
        }
        frame_out.copy_from_slice(&work);
        true
    })?;
    if !audible {
        return Err("audio source is silent".to_string());
    }
    let mut data = table.data.as_ref().clone();
    for row in data.chunks_exact_mut(NBINS) {
        octave_fraction_smooth(row);
    }
    MagnitudeTable::new(data)
}

/// Half-width of the audio-mode perceptual smoothing band as a frequency
/// ratio: each bin is averaged over [k / r, k * r], one-sixth of an octave in
/// total. Broadband material analyzed frame-locally otherwise bakes noisy
/// spectral masks that sound like stem-separation artifacts; a modest
/// constant-Q average keeps the spectral envelope while dropping bin-local
/// noise. Conservative by design — tuned by ear later if needed.
const AUDIO_SMOOTH_RATIO: f32 = 1.059_463_1; // 2^(1/12)

/// Constant-Q box smoothing in linear magnitude over [k/r, k*r], followed by
/// re-normalization to the row's original peak so audio-mode rows keep the
/// 0..=1 domain contract. Bin 0 (DC) keeps a zero-width band and is untouched.
fn octave_fraction_smooth(row: &mut [f32]) {
    let bins = row.len();
    let peak_before = row.iter().fold(0.0f32, |current, value| current.max(*value));
    if peak_before <= 0.0 {
        return;
    }
    let mut prefix = Vec::with_capacity(bins + 1);
    prefix.push(0.0f64);
    for value in row.iter() {
        prefix.push(prefix.last().unwrap() + *value as f64);
    }
    let mut smoothed = vec![0.0f32; bins];
    for (bin, out) in smoothed.iter_mut().enumerate() {
        let lo = ((bin as f32 / AUDIO_SMOOTH_RATIO).floor() as usize).min(bin);
        let hi = ((bin as f32 * AUDIO_SMOOTH_RATIO).ceil() as usize).clamp(bin, bins - 1);
        let count = (hi - lo + 1) as f64;
        *out = ((prefix[hi + 1] - prefix[lo]) / count) as f32;
    }
    let peak_after = smoothed
        .iter()
        .fold(0.0f32, |current, value| current.max(*value));
    if peak_after <= 0.0 {
        return;
    }
    let gain = peak_before / peak_after;
    for (value, source) in row.iter_mut().zip(&smoothed) {
        *value = source * gain;
    }
}

/// Impulse response: the source *is* the filter, so its spectrum is taken
/// directly — no DC removal and no amplitude normalization beyond the shared
/// spectral peak normalization. The first 2048 samples are analyzed; a longer
/// IR is truncated with a 256-sample half-Hann fade to suppress truncation
/// ringing. The single response repeats across all 64 frames.
fn analyze_impulse_response(mono: &[f32]) -> Result<MagnitudeTable, String> {
    const FADE: usize = 256;
    let take = mono.len().min(N);
    let mut response = vec![0.0f32; N];
    response[..take].copy_from_slice(&mono[..take]);
    if mono.len() > N {
        for index in 0..FADE {
            let phase = (index + 1) as f32 / FADE as f32;
            let weight = 0.5 + 0.5 * (std::f32::consts::PI * phase).cos();
            response[N - FADE + index] *= weight;
        }
    }
    if response.iter().all(|value| value.abs() <= 1.0e-12) {
        return Err("impulse response is silent".to_string());
    }
    spectrum_table(|_, frame_out| {
        frame_out.copy_from_slice(&response);
        true
    })
}

/// Shared spectral stage: `fill` writes each frame's 2048 time-domain samples
/// (returning false for an intentionally silent frame, which becomes a zero
/// row); each filled frame is transformed and peak-normalized to 0..=1.
fn spectrum_table(
    mut fill: impl FnMut(usize, &mut [f32]) -> bool,
) -> Result<MagnitudeTable, String> {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut data = Vec::with_capacity(TABLE_LEN);
    let mut time = vec![0.0f32; N];
    let mut work = vec![Complex32::default(); N];
    for table_frame in 0..FRAMES {
        time.fill(0.0);
        if !fill(table_frame, &mut time) {
            data.extend(std::iter::repeat(0.0).take(NBINS));
            continue;
        }
        for (complex, sample) in work.iter_mut().zip(&time) {
            *complex = Complex32::new(*sample, 0.0);
        }
        fft.process(&mut work);
        let peak = work[..NBINS]
            .iter()
            .fold(0.0_f32, |current, value| current.max(value.norm()));
        data.extend(
            work[..NBINS]
                .iter()
                .map(|value| value.norm() / peak.max(1.0e-12)),
        );
    }
    MagnitudeTable::new(data)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeepDc {
    Keep,
    Remove,
}

/// Scale a frame to unit RMS (computed about zero so deliberate DC energy
/// counts), optionally removing the mean first. Returns false and zeroes the
/// frame when it is effectively silent.
fn rms_normalize(frame: &mut [f32], dc: KeepDc) -> bool {
    let mean = match dc {
        KeepDc::Remove => {
            frame.iter().map(|value| *value as f64).sum::<f64>() / frame.len() as f64
        }
        KeepDc::Keep => 0.0,
    };
    let energy = frame
        .iter()
        .map(|value| {
            let centered = *value as f64 - mean;
            centered * centered
        })
        .sum::<f64>()
        / frame.len() as f64;
    let rms = energy.sqrt();
    if rms <= 1.0e-12 {
        frame.fill(0.0);
        return false;
    }
    let gain = (1.0 / rms) as f32;
    for value in frame {
        *value = (*value - mean as f32) * gain;
    }
    true
}

fn hann_window() -> Vec<f32> {
    (0..N)
        .map(|index| {
            0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (N - 1) as f32).cos()
        })
        .collect()
}

/// Original procedural default bank. Eight anchor responses are smoothly
/// interpolated over 64 frames: low/high-pass, two formants, band/notch, comb,
/// and flat. This is generated in code so the bundled effect is reproducible
/// and does not depend on third-party wavetable content.
pub fn default_table() -> MagnitudeTable {
    let anchors = (0..8)
        .map(|anchor| {
            (0..NBINS)
                .map(|bin| procedural_magnitude(anchor, bin as f32 / (NBINS - 1) as f32))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut data = Vec::with_capacity(TABLE_LEN);
    for frame in 0..FRAMES {
        let position = frame as f32 * (anchors.len() - 1) as f32 / (FRAMES - 1) as f32;
        let lo = position.floor() as usize;
        let hi = (lo + 1).min(anchors.len() - 1);
        let fraction = position - lo as f32;
        for bin in 0..NBINS {
            data.push(anchors[lo][bin] + (anchors[hi][bin] - anchors[lo][bin]) * fraction);
        }
    }
    MagnitudeTable::new(data).expect("procedural table dimensions are fixed")
}

fn procedural_magnitude(anchor: usize, x: f32) -> f32 {
    let smooth_step = |edge0: f32, edge1: f32, value: f32| {
        let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let gaussian = |center: f32, width: f32| {
        let distance = (x - center) / width;
        (-0.5 * distance * distance).exp()
    };
    match anchor {
        0 => 1.0 - smooth_step(0.18, 0.42, x),
        1 => smooth_step(0.08, 0.32, x),
        2 => (0.04 + gaussian(0.18, 0.055) + 0.7 * gaussian(0.46, 0.09)).min(1.0),
        3 => (0.03 + gaussian(0.32, 0.075) + 0.8 * gaussian(0.7, 0.07)).min(1.0),
        4 => (0.02 + gaussian(0.5, 0.12)).min(1.0),
        5 => (1.0 - 0.96 * gaussian(0.48, 0.09)).max(0.02),
        6 => (0.1 + 0.9 * (0.5 + 0.5 * (x * 18.0 * std::f32::consts::PI).cos())).min(1.0),
        _ => 1.0,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TableSlot {
    pub magnitudes: usize,
}

impl TableSlot {
    pub fn from_manifest(manifest: &crate::lisp_host::DGenManifest) -> Option<Self> {
        manifest
            .tensors
            .iter()
            .find(|tensor| tensor.name == "table_magnitudes" && tensor.shape == [FRAMES, NBINS])
            .map(|tensor| Self {
                magnitudes: tensor.cell_offset,
            })
    }
}

type NodeMap<V> = Mutex<BTreeMap<i32, V>>;
static TABLE_SLOTS: NodeMap<TableSlot> = Mutex::new(BTreeMap::new());
static TABLE_REFS: NodeMap<String> = Mutex::new(BTreeMap::new());
static TABLE_NAMES: NodeMap<String> = Mutex::new(BTreeMap::new());
static TABLE_DATA: NodeMap<Arc<MagnitudeTable>> = Mutex::new(BTreeMap::new());

pub fn record_slot(node_id: i32, slot: TableSlot) {
    if let Ok(mut slots) = TABLE_SLOTS.lock() {
        slots.insert(node_id, slot);
    }
}

pub fn table_name_for(node_id: i32) -> Option<String> {
    TABLE_NAMES.lock().ok()?.get(&node_id).cloned()
}

pub fn table_ref_for(node_id: i32) -> Option<String> {
    TABLE_REFS.lock().ok()?.get(&node_id).cloned()
}

pub fn prepared_table_for(node_id: i32) -> Option<Arc<MagnitudeTable>> {
    TABLE_DATA.lock().ok()?.get(&node_id).cloned()
}

pub fn record_prepared_table(
    node_id: i32,
    reference: &str,
    display_name: &str,
    table: Arc<MagnitudeTable>,
) {
    if let Ok(mut references) = TABLE_REFS.lock() {
        references.insert(node_id, reference.to_string());
    }
    if let Ok(mut names) = TABLE_NAMES.lock() {
        names.insert(node_id, display_name.to_string());
    }
    eseqlisp::widget_render::wavetable_viewer::publish_bank(
        visualization_key(node_id),
        NBINS,
        table.data.clone(),
    );
    if let Ok(mut data) = TABLE_DATA.lock() {
        data.insert(node_id, table);
    }
}

pub fn clear_instance(node_id: i32) {
    eseqlisp::widget_render::wavetable_viewer::remove_published_bank(&visualization_key(node_id));
    if let Ok(mut slots) = TABLE_SLOTS.lock() {
        slots.remove(&node_id);
    }
    if let Ok(mut references) = TABLE_REFS.lock() {
        references.remove(&node_id);
    }
    if let Ok(mut names) = TABLE_NAMES.lock() {
        names.remove(&node_id);
    }
    if let Ok(mut data) = TABLE_DATA.lock() {
        data.remove(&node_id);
    }
}

/// Queue a complete table replacement at an audio block boundary.
///
/// # Safety
/// `graph` must be live and `node_id` must identify the corresponding DGen node.
pub unsafe fn apply_table_to_node(
    graph: *mut crate::audiograph::LiveGraph,
    node_id: i32,
    table: &MagnitudeTable,
) -> Result<(), String> {
    let slot = TABLE_SLOTS
        .lock()
        .ok()
        .and_then(|slots| slots.get(&node_id).copied())
        .ok_or_else(|| "slot is not a Filter Table".to_string())?;
    if crate::lisp_host::queue_tensor_write(graph, node_id, slot.magnitudes, &table.data) {
        Ok(())
    } else {
        Err("failed to queue filter table write (graph edit queue full)".to_string())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Concurrent renders of the bundled DSP share one cached dylib image
    /// (same source → same cache key → same dlopen handle), and the compiled
    /// code is not reentrant with itself. Serialize the render-based tests —
    /// including render tests in other modules (e.g. filter_table_presets)
    /// that compile the same bundled source.
    pub(crate) fn render_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The per-node registries (TABLE_SLOTS et al.) are keyed by graph node
    /// id, and separate TestLiveGraphs hand out overlapping ids — two
    /// registry-touching tests running in parallel can clear each other's
    /// entries. Serialize them.
    pub(crate) fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn default_bank_is_finite_normalized_and_varies_by_frame() {
        let table = default_table();
        assert_eq!(table.data.len(), TABLE_LEN);
        assert!(table
            .data
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0 && *value <= 1.0));
        assert_ne!(&table.data[..NBINS], &table.data[(FRAMES - 1) * NBINS..]);
    }

    #[test]
    fn short_periodic_wave_is_repeated_across_table_frames() {
        let source = (0..64)
            .map(|index| (index as f32 * std::f32::consts::TAU / 64.0).sin())
            .collect::<Vec<_>>();
        let table = analyze(&source, AnalysisMode::SingleCycle).expect("single cycle");
        assert_eq!(&table.data[..NBINS], &table.data[(FRAMES - 1) * NBINS..]);
    }

    #[test]
    fn mode_recommendation_is_deterministic() {
        assert_eq!(recommend_mode(300), AnalysisMode::SingleCycle);
        assert_eq!(recommend_mode(N), AnalysisMode::SingleCycle);
        assert_eq!(recommend_mode(2 * N), AnalysisMode::Wavetable);
        assert_eq!(recommend_mode(256 * N), AnalysisMode::Wavetable);
        // Non-integer cycle counts are ordinary audio, never wavetables.
        assert_eq!(recommend_mode(2 * N + 1), AnalysisMode::AudioTexture);
        assert_eq!(recommend_mode(500_000), AnalysisMode::AudioTexture);
    }

    #[test]
    fn table_ref_mode_encoding_roundtrips() {
        for mode in AnalysisMode::ALL {
            let encoded = encode_table_ref("my-sample", mode);
            assert_eq!(decode_table_ref(&encoded), ("my-sample", Some(mode)));
        }
        // Legacy bare references and unknown tags stay whole with no mode.
        assert_eq!(decode_table_ref("legacy-name"), ("legacy-name", None));
        assert_eq!(
            decode_table_ref("odd#ft-mode=nonsense"),
            ("odd#ft-mode=nonsense", None)
        );
        assert_eq!(
            decode_table_ref(DEFAULT_TABLE_REF),
            (DEFAULT_TABLE_REF, None)
        );
    }

    #[test]
    fn wavetable_mode_rejects_non_cycle_multiples() {
        let source = vec![0.5f32; 3000];
        let error = analyze(&source, AnalysisMode::Wavetable).unwrap_err();
        assert!(
            error.contains("whole 2048-sample cycles") && error.contains("3000"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn wavetable_mode_keeps_deliberate_dc_and_audio_mode_removes_it() {
        // One cycle: a sine on harmonic 8 riding on a strong DC offset.
        let cycle = (0..N)
            .map(|index| 0.8 + 0.3 * (std::f32::consts::TAU * 8.0 * index as f32 / N as f32).sin())
            .collect::<Vec<_>>();
        let wavetable = analyze(&cycle, AnalysisMode::Wavetable).expect("wavetable");
        assert!(
            wavetable.data[0] > 0.5,
            "wavetable mode must preserve deliberate DC energy: bin0={}",
            wavetable.data[0]
        );

        let long = cycle
            .iter()
            .copied()
            .cycle()
            .take(5 * N + 7)
            .collect::<Vec<_>>();
        let audio = analyze(&long, AnalysisMode::AudioTexture).expect("audio");
        assert!(
            audio.data[0] < 0.05,
            "audio mode must remove window DC bias: bin0={}",
            audio.data[0]
        );
    }

    #[test]
    fn single_cycle_mode_resamples_any_length_and_replicates_frames() {
        // One period spread over 3000 samples resamples to the fundamental.
        let source = (0..3000)
            .map(|index| (std::f32::consts::TAU * index as f32 / 3000.0).sin())
            .collect::<Vec<_>>();
        let table = analyze(&source, AnalysisMode::SingleCycle).expect("single cycle");
        let first = &table.data[..NBINS];
        let dominant = first
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(dominant, 1);
        for frame in 1..FRAMES {
            assert_eq!(first, &table.data[frame * NBINS..(frame + 1) * NBINS]);
        }
    }

    #[test]
    fn stereo_import_matches_downmixed_mono() {
        // Amplitude stays below 1/1.5 so the wider stereo channel cannot clip.
        let mono: Vec<f32> = (0..N)
            .map(|index| 0.5 * (std::f32::consts::TAU * 12.0 * index as f32 / N as f32).sin())
            .collect();
        let mono_path = write_mono_wav("stereo-ref-mono", mono.iter().copied());
        let stereo_path = {
            let path = std::env::temp_dir().join(format!(
                "eseq-filter-table-{}-stereo.wav",
                std::process::id(),
            ));
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
            for sample in &mono {
                // Asymmetric channels whose average is the mono reference.
                writer.write_sample(sample * 1.5).expect("write left");
                writer.write_sample(sample * 0.5).expect("write right");
            }
            writer.finalize().expect("finalize wav");
            path
        };
        let (mono_table, mono_mode) = prepare_table(&mono_path).expect("mono");
        let (stereo_table, stereo_mode) = prepare_table(&stereo_path).expect("stereo");
        let _ = std::fs::remove_file(mono_path);
        let _ = std::fs::remove_file(stereo_path);
        assert_eq!(mono_mode, stereo_mode);
        let max_diff = mono_table
            .data
            .iter()
            .zip(stereo_table.data.iter())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff < 1.0e-4,
            "stereo downmix diverged from mono reference: max diff {max_diff}"
        );
    }

    #[test]
    fn audio_texture_mode_tracks_spectral_motion_over_time() {
        // First half harmonic 16, second half harmonic 48: early frames must
        // resolve the first tone, late frames the second, with Hann-controlled
        // leakage away from each peak.
        let half = 8 * N;
        let source: Vec<f32> = (0..half)
            .map(|index| (std::f32::consts::TAU * 16.0 * index as f32 / N as f32).sin())
            .chain(
                (0..half).map(|index| {
                    (std::f32::consts::TAU * 48.0 * index as f32 / N as f32).sin()
                }),
            )
            .collect();
        let table = analyze(&source, AnalysisMode::AudioTexture).expect("audio texture");
        let dominant = |frame: usize| {
            table.data[frame * NBINS..(frame + 1) * NBINS]
                .iter()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(right.1))
                .map(|(index, _)| index)
                .unwrap()
        };
        assert_eq!(dominant(0), 16);
        assert_eq!(dominant(FRAMES - 1), 48);
        let first = &table.data[..NBINS];
        assert!(
            first[200] < 0.01,
            "Hann-windowed frame should have low far-field leakage: {}",
            first[200]
        );
    }

    #[test]
    fn impulse_response_mode_takes_the_spectrum_directly() {
        // A unit impulse is an allpass: every bin at full magnitude.
        let mut impulse = vec![0.0f32; 400];
        impulse[0] = 1.0;
        let table = analyze(&impulse, AnalysisMode::ImpulseResponse).expect("impulse");
        for frame in [0, FRAMES / 2, FRAMES - 1] {
            for (bin, value) in table.data[frame * NBINS..(frame + 1) * NBINS]
                .iter()
                .enumerate()
            {
                assert!(
                    (value - 1.0).abs() < 1.0e-5,
                    "frame {frame} bin {bin}: impulse spectrum should be flat, got {value}"
                );
            }
        }

        // A two-tap comb IR nulls the odd harmonics of its delay.
        let mut comb = vec![0.0f32; 64];
        comb[0] = 1.0;
        comb[32] = -1.0;
        let comb_table = analyze(&comb, AnalysisMode::ImpulseResponse).expect("comb");
        let row = &comb_table.data[..NBINS];
        assert!(row[0] < 1.0e-4, "comb DC null missing: {}", row[0]);
        assert!(row[32] > 0.9, "comb peak missing: {}", row[32]);
    }

    #[test]
    fn silent_sources_error_deterministically_per_mode() {
        let silent = vec![0.0f32; 4 * N];
        assert_eq!(
            analyze(&silent, AnalysisMode::Wavetable).unwrap_err(),
            "wavetable source is silent"
        );
        assert_eq!(
            analyze(&silent, AnalysisMode::SingleCycle).unwrap_err(),
            "single-cycle source is silent"
        );
        assert_eq!(
            analyze(&silent, AnalysisMode::AudioTexture).unwrap_err(),
            "audio source is silent"
        );
        assert_eq!(
            analyze(&silent, AnalysisMode::ImpulseResponse).unwrap_err(),
            "impulse response is silent"
        );
    }

    #[test]
    fn malformed_sources_error_instead_of_analyzing() {
        let path = std::env::temp_dir().join(format!(
            "eseq-filter-table-{}-malformed.wav",
            std::process::id(),
        ));
        std::fs::write(&path, b"this is not a wav file at all").expect("write junk");
        let error = prepare_table(&path).unwrap_err();
        let _ = std::fs::remove_file(path);
        assert!(!error.is_empty());
    }

    fn write_mono_wav(tag: &str, samples: impl Iterator<Item = f32>) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eseq-filter-table-{}-{tag}.wav",
            std::process::id(),
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
        for sample in samples {
            writer.write_sample(sample).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
        path
    }

    /// Synthetic structured wavetable: `cycles` whole 2048-sample cycles where
    /// cycle k is the pure harmonic `first_harmonic + k`.
    fn harmonic_ramp_wavetable(cycles: usize, first_harmonic: usize) -> Vec<f32> {
        (0..cycles)
            .flat_map(|cycle| {
                let harmonic = (first_harmonic + cycle) as f32;
                (0..N).map(move |index| {
                    (std::f32::consts::TAU * harmonic * index as f32 / N as f32).sin()
                })
            })
            .collect()
    }

    /// Expected structured mapping: table frame f sits at fractional source
    /// cycle f*(cycles-1)/(FRAMES-1) and blends the two adjacent whole cycles.
    fn expected_cycle_pair(table_frame: usize, cycles: usize) -> (usize, usize) {
        let position = table_frame as f64 * (cycles - 1) as f64 / (FRAMES - 1) as f64;
        let lo = (position.floor() as usize).min(cycles - 1);
        (lo, (lo + 1).min(cycles - 1))
    }

    fn assert_frames_are_cycle_pure(table: &MagnitudeTable, cycles: usize, first_harmonic: usize) {
        for table_frame in 0..FRAMES {
            let row = &table.data[table_frame * NBINS..(table_frame + 1) * NBINS];
            let (lo, hi) = expected_cycle_pair(table_frame, cycles);
            let expected = [first_harmonic + lo, first_harmonic + hi];
            let dominant = row
                .iter()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(right.1))
                .map(|(index, _)| index)
                .unwrap();
            assert!(
                expected.contains(&dominant),
                "frame {table_frame}: dominant bin {dominant} not in expected {expected:?}",
            );
            for (bin, magnitude) in row.iter().enumerate() {
                if !expected.contains(&bin) {
                    assert!(
                        *magnitude < 5.0e-3,
                        "frame {table_frame}: bin {bin} leaked magnitude {magnitude} \
                         (expected energy only in {expected:?}); analysis window is \
                         not cycle-aligned",
                    );
                }
            }
        }
    }

    #[test]
    fn structured_256x2048_wavetable_imports_as_aligned_cycles() {
        let cycles = 256;
        let path = write_mono_wav(
            "structured256",
            harmonic_ramp_wavetable(cycles, 8).into_iter(),
        );
        let (table, mode) = prepare_table(&path).expect("prepare structured wavetable");
        let _ = std::fs::remove_file(path);
        assert_eq!(mode, AnalysisMode::Wavetable);
        assert_frames_are_cycle_pure(&table, cycles, 8);
    }

    #[test]
    fn arbitrary_cycle_count_wavetable_maps_deterministically() {
        // Three whole cycles: harmonics 10/11/12. Frame 0 must be exactly the
        // first cycle, the last frame exactly the final cycle, and every
        // interior frame a blend of two adjacent aligned cycles only.
        let cycles = 3;
        let path = write_mono_wav(
            "structured3",
            harmonic_ramp_wavetable(cycles, 10).into_iter(),
        );
        let (table, mode) = prepare_table(&path).expect("prepare three-cycle wavetable");
        let _ = std::fs::remove_file(path);
        assert_eq!(mode, AnalysisMode::Wavetable);
        assert_frames_are_cycle_pure(&table, cycles, 10);

        let first = &table.data[..NBINS];
        let last = &table.data[(FRAMES - 1) * NBINS..];
        assert!((first[10] - 1.0).abs() < 1.0e-5 && first[11] < 5.0e-3);
        assert!((last[12] - 1.0).abs() < 1.0e-5 && last[11] < 5.0e-3);
    }

    #[test]
    fn audio_source_is_converted_to_peak_normalized_magnitudes() {
        let path = write_mono_wav(
            "single-cycle",
            (0..N).map(|index| {
                (std::f32::consts::TAU * 24.0 * index as f32 / N as f32).sin()
            }),
        );
        let (table, mode) = prepare_table(&path).expect("prepare magnitude table");
        let _ = std::fs::remove_file(path);
        assert_eq!(mode, AnalysisMode::SingleCycle);
        let first = &table.data[..NBINS];
        let peak_bin = first
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak_bin, 24);
        assert!((first[24] - 1.0).abs() < 1.0e-6);
        assert!(first[23] < 1.0e-4 && first[25] < 1.0e-4);
        assert_eq!(first, &table.data[(FRAMES - 1) * NBINS..]);
    }

    #[test]
    fn bundled_dsp_applies_host_modulation_to_mix() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let render = |mod_value: f32| {
            crate::lisp_host::render_effect_source_for_test(
                dsp_source(),
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("mix".to_string(), 0.0),
                        ("__dgen_mod_active__mix".to_string(), 1.0),
                        ("mod mix slot 1 amt".to_string(), 0.8),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: Vec::new(),
                    input_overrides: vec![(2, mod_value)],
                },
            )
            .expect("render bundled Filter Table DSP")
        };

        // The test tensor starts at zero, so increasing mix attenuates the
        // latency-aligned dry signal toward the silent wet response.
        let unmodulated = render(0.0);
        let modulated = render(1.0);
        assert!(unmodulated.rms > 0.001, "dry render should contain signal");
        assert!(
            modulated.rms < unmodulated.rms * 0.6,
            "mix modulation should audibly reduce the dry signal: unmodulated rms={}, modulated rms={}",
            unmodulated.rms,
            modulated.rms,
        );
    }

    #[test]
    fn bundled_dsp_resonance_is_gain_bounded() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let table = default_table();
        let render = |resonance: f32| {
            crate::lisp_host::render_effect_source_for_test(
                dsp_source(),
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("frame".to_string(), 6.0 / 7.0),
                        ("cutoff".to_string(), 3000.0),
                        ("resonance".to_string(), resonance),
                        ("mix".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: vec![(
                        "table_magnitudes".to_string(),
                        table.data.as_ref().clone(),
                    )],
                    input_overrides: Vec::new(),
                },
            )
            .expect("render bundled Filter Table resonance")
        };

        let low = render(0.0);
        let high = render(1.0);
        assert!(low.rms > 0.001, "low-resonance wet output should remain audible");
        assert!(
            high.peak <= low.peak * 4.0,
            "resonance must not create runaway gain: low peak={}, high peak={}",
            low.peak,
            high.peak,
        );
        assert!(
            high.peak < 2.0,
            "bounded spectral makeup should keep the probe output sane: {}",
            high.peak,
        );
    }

    #[test]
    fn bundled_dsp_frame_endpoint_reads_exactly_the_final_row() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        // cutoff = REFERENCE_HARMONIC * samplerate / N makes the harmonic axis
        // an identity mapping, so the rendered response is the selected table
        // row itself.
        let identity_cutoff = REFERENCE_HARMONIC as f32 * 44_100.0 / N as f32;
        let render = |table: Vec<f32>| {
            crate::lisp_host::render_effect_source_for_test(
                dsp_source(),
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 8192,
                    param_overrides: vec![
                        ("frame".to_string(), 1.0),
                        ("cutoff".to_string(), identity_cutoff),
                        ("resonance".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: vec![("table_magnitudes".to_string(), table)],
                    input_overrides: Vec::new(),
                },
            )
            .expect("render bundled Filter Table endpoint")
        };

        // Only the final row passes audio; every other row is silent. frame=1.0
        // must resolve to exactly row 63 without gathering a row that does not
        // exist, so this render is loud...
        let mut final_row_open = vec![0.0f32; TABLE_LEN];
        final_row_open[(FRAMES - 1) * NBINS..].fill(1.0);
        let open = render(final_row_open);

        // ...and with the final row silenced (all other rows open) it is not.
        let mut final_row_closed = vec![1.0f32; TABLE_LEN];
        final_row_closed[(FRAMES - 1) * NBINS..].fill(0.0);
        let closed = render(final_row_closed);

        assert!(
            open.peak.is_finite() && closed.peak.is_finite(),
            "endpoint interpolation produced non-finite output",
        );
        assert!(
            open.rms > 0.001,
            "frame=1.0 should pass audio through the final open row: rms={}",
            open.rms,
        );
        assert!(
            open.rms > 50.0 * closed.rms,
            "frame=1.0 must select only row 63: open rms={}, closed rms={}",
            open.rms,
            closed.rms,
        );
    }

    #[test]
    fn bundled_dsp_compiles_with_magnitude_tensor() {
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }
        let json = crate::lisp_host::compile_lisp(dsp_source(), 44_100)
            .expect("compile bundled Filter Table DSP");
        let manifest = crate::lisp_host::parse_manifest(&json).expect("parse manifest");
        TableSlot::from_manifest(&manifest).expect("manifest table_magnitudes tensor");
        assert_eq!(manifest.n_inputs, 6);
        assert_eq!(manifest.n_outputs, 2);
        let modulated = manifest
            .mod_destinations
            .iter()
            .map(|destination| destination.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(modulated, ["frame", "cutoff", "resonance", "mix"]);
        assert!(!modulated.contains(&"output"));
        for name in ["frame", "resonance", "mix"] {
            let param = manifest
                .params
                .iter()
                .find(|param| param.name == name)
                .unwrap_or_else(|| panic!("missing {name} parameter"));
            assert_eq!(param.unit.as_deref(), Some("%"));
        }
    }

    /// Hop-rate RMS envelope of the left channel inside [start, end), and the
    /// summed squared second difference of that envelope. A hop-quantized
    /// response staircase concentrates envelope motion into jumps (high
    /// jerk); a smoothed response trajectory spreads the same motion across
    /// every hop (low jerk).
    fn envelope_jerk(samples: &[f32], start: usize, end: usize) -> (f64, f64) {
        let left: Vec<f32> = samples
            .chunks_exact(2)
            .skip(start)
            .take(end - start)
            .map(|frame| frame[0])
            .collect();
        let envelope: Vec<f64> = left
            .chunks_exact(512)
            .map(|hop| {
                (hop.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / hop.len() as f64)
                    .sqrt()
            })
            .collect();
        let jerk = envelope
            .windows(3)
            .map(|w| {
                let d = w[2] - 2.0 * w[1] + w[0];
                d * d
            })
            .sum();
        let swing = envelope.iter().cloned().fold(f64::MIN, f64::max)
            - envelope.iter().cloned().fold(f64::MAX, f64::min);
        (jerk, swing)
    }

    #[test]
    fn bundled_dsp_smoothing_removes_automation_zipper() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        // A gradual low-pass slope (1.0 at DC falling to 0.03 by table
        // harmonic 24): the output envelope then tracks cutoff continuously,
        // so a hop-quantized cutoff staircase shows up directly as an
        // envelope staircase while a smoothed trajectory glides.
        let mut row = vec![0.03f32; NBINS];
        for (bin, value) in row.iter_mut().enumerate().take(24) {
            *value = 1.0 - 0.97 * (bin as f32 / 24.0);
        }
        let mut table = Vec::with_capacity(TABLE_LEN);
        for _ in 0..FRAMES {
            table.extend_from_slice(&row);
        }
        // Coarse staircase automation: 8 steps from 4000 Hz down to 800 Hz,
        // 6 hops apart, the way coarse host automation reaches the DSP.
        let ramp_start = 12_288usize;
        let steps = 8usize;
        let step_frames = 3072usize;
        let events = (0..steps)
            .map(|step| crate::lisp_host::InstrumentParamEvent {
                frame: ramp_start + step * step_frames,
                name: "cutoff".to_string(),
                value: 4000.0 - (3200.0 * (step + 1) as f32 / steps as f32),
            })
            .collect::<Vec<_>>();
        let render = |source: String| {
            crate::lisp_host::render_effect_source_for_test(
                &source,
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 45_056,
                    param_overrides: vec![
                        ("frame".to_string(), 0.0),
                        ("cutoff".to_string(), 4000.0),
                        ("resonance".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                    ],
                    param_events: events.clone(),
                    input_tones: vec![(0, 620.0, 0.4)],
                    tensor_overrides: vec![("table_magnitudes".to_string(), table.clone())],
                    input_overrides: Vec::new(),
                },
            )
            .expect("render bundled Filter Table automation")
        };

        let smoothed = render(dsp_source().to_string());
        let legacy_source = dsp_source().replace("(def SMOOTH-MS 30)", "(def SMOOTH-MS 0)");
        assert_ne!(legacy_source, dsp_source(), "SMOOTH-MS constant not found");
        let stepped = render(legacy_source);

        assert!(smoothed.rms > 0.001, "smoothed render should carry signal");
        assert!(stepped.rms > 0.001, "stepped render should carry signal");
        let ramp_end = ramp_start + steps * step_frames + 4096;
        let (smooth_jerk, smooth_swing) = envelope_jerk(&smoothed.samples, ramp_start, ramp_end);
        let (stepped_jerk, stepped_swing) = envelope_jerk(&stepped.samples, ramp_start, ramp_end);
        assert!(
            stepped_swing > 0.005 && smooth_swing > 0.005,
            "the automation must actually move the output envelope: stepped={stepped_swing}, smoothed={smooth_swing}",
        );
        // The 4x-overlap synthesis already spreads each response switch over
        // one window, so the smoothing's measurable win at hop resolution is
        // roughly a further halving of envelope jerk (observed ratio ~0.55,
        // deterministic render). Without smoothing the renders are identical
        // and the ratio is 1.0, so 0.65 separates the two cleanly.
        assert!(
            smooth_jerk < stepped_jerk * 0.65,
            "response smoothing should spread envelope motion across hops: smoothed={smooth_jerk:e}, stepped={stepped_jerk:e}",
        );
    }

    #[test]
    fn bundled_dsp_low_cutoff_resample_keeps_narrow_features() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        // A single one-bin passband at table bin 513. With cutoff at stride 8
        // the naive resample reads bins 0, 8, .., 512, 520, .. and lands on
        // 512/520 around the feature — the passband vanishes entirely unless
        // the row is band-limited to the stride first.
        let mut row = vec![0.0f32; NBINS];
        row[513] = 1.0;
        let mut table = Vec::with_capacity(TABLE_LEN);
        for _ in 0..FRAMES {
            table.extend_from_slice(&row);
        }
        let bin_hz = 44_100.0 / N as f32;
        let stride = 8.0f32;
        let cutoff = REFERENCE_HARMONIC as f32 * bin_hz / stride;
        let passband_hz = (513.0 / stride) * bin_hz;
        let render = |source: String, tone_hz: f32| {
            crate::lisp_host::render_effect_source_for_test(
                &source,
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 16_384,
                    param_overrides: vec![
                        ("frame".to_string(), 0.0),
                        ("cutoff".to_string(), cutoff),
                        ("resonance".to_string(), 0.0),
                        ("mix".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: vec![(0, tone_hz, 0.4)],
                    tensor_overrides: vec![("table_magnitudes".to_string(), table.clone())],
                    input_overrides: Vec::new(),
                },
            )
            .expect("render bundled Filter Table anti-alias probe")
        };

        let banded = render(dsp_source().to_string(), passband_hz);
        let naive_source = dsp_source().replace("(def AA-MAX-LEVEL 4)", "(def AA-MAX-LEVEL 0)");
        assert_ne!(naive_source, dsp_source(), "AA-MAX-LEVEL constant not found");
        let naive = render(naive_source, passband_hz);
        assert!(
            banded.left_rms > 5.0 * naive.left_rms.max(1.0e-6),
            "band-limited resample must keep the narrow passband the naive stride drops: banded={}, naive={}",
            banded.left_rms,
            naive.left_rms,
        );
        assert!(
            banded.peak < 2.5,
            "anti-aliased response stays gain-bounded: peak={}",
            banded.peak,
        );

        // Selectivity check: smoothing widens the passband by the stride, not
        // into a wholesale blur — a tone an octave away stays strongly
        // attenuated relative to the passband.
        let out_of_band = render(dsp_source().to_string(), passband_hz * 2.0);
        assert!(
            banded.left_rms > 5.0 * out_of_band.left_rms.max(1.0e-6),
            "anti-aliased response keeps spectral selectivity: passband={}, out_of_band={}",
            banded.left_rms,
            out_of_band.left_rms,
        );
    }

    #[test]
    fn bundled_dsp_smoothing_preserves_steady_state() {
        let _render = render_lock();
        if !crate::lisp_host::dgenlisp_tool_path().exists() {
            eprintln!("skipping: DGenLisp tool not found");
            return;
        }

        let table = default_table();
        let render = |source: String| {
            crate::lisp_host::render_effect_source_for_test(
                &source,
                &crate::lisp_host::EffectRenderOptions {
                    sample_rate: 44_100,
                    block_size: 512,
                    frames: 16_384,
                    param_overrides: vec![
                        ("frame".to_string(), 6.0 / 7.0),
                        ("cutoff".to_string(), 3000.0),
                        ("resonance".to_string(), 0.3),
                        ("mix".to_string(), 1.0),
                    ],
                    param_events: Vec::new(),
                    input_tones: Vec::new(),
                    tensor_overrides: vec![(
                        "table_magnitudes".to_string(),
                        table.data.as_ref().clone(),
                    )],
                    input_overrides: Vec::new(),
                },
            )
            .expect("render bundled Filter Table steady state")
        };

        let smoothed = render(dsp_source().to_string());
        let legacy = render(dsp_source().replace("(def SMOOTH-MS 30)", "(def SMOOTH-MS 0)"));
        assert!(legacy.rms > 0.001, "steady render should carry signal");
        let rms_ratio = smoothed.rms / legacy.rms;
        assert!(
            (0.98..=1.02).contains(&rms_ratio),
            "constant parameters must converge to the unsmoothed response: ratio={rms_ratio}",
        );
    }

    #[test]
    fn audio_mode_smoothing_drops_bin_noise_and_keeps_envelope() {
        // Jagged synthetic row: broad triangular envelope carrying alternating
        // one-bin spikes — the frame-local analysis noise the audio mode's
        // constant-Q smoothing is meant to suppress.
        let mut row = vec![0.0f32; NBINS];
        for (bin, value) in row.iter_mut().enumerate().skip(64) {
            let envelope = 1.0 - ((bin as f32 - 512.0).abs() / 512.0);
            *value = if bin % 2 == 0 { envelope } else { envelope * 0.1 };
        }
        let original = row.clone();
        octave_fraction_smooth(&mut row);

        let variation = |data: &[f32]| -> f32 {
            data.windows(2).skip(256).map(|w| (w[1] - w[0]).abs()).sum()
        };
        assert!(
            variation(&row) < variation(&original) / 3.0,
            "constant-Q smoothing should drop bin-local noise: before={}, after={}",
            variation(&original),
            variation(&row),
        );

        // Re-normalization rescales absolute energy, so compare each octave
        // band's *share* of the total: the spectral envelope must keep its
        // shape even though bin-local detail is averaged away.
        let band_share = |data: &[f32], lo: usize, hi: usize| -> f32 {
            let total: f32 = data[64..].iter().map(|value| value * value).sum();
            let band: f32 = data[lo..hi].iter().map(|value| value * value).sum();
            band / total
        };
        for (lo, hi) in [(128usize, 256usize), (256, 512), (512, 1024)] {
            let before = band_share(&original, lo, hi);
            let after = band_share(&row, lo, hi);
            assert!(
                (after / before) > 0.7 && (after / before) < 1.4,
                "octave-band energy share should be preserved in {lo}..{hi}: before={before}, after={after}",
            );
        }

        let peak = |data: &[f32]| data.iter().fold(0.0f32, |c, v| c.max(*v));
        assert!(
            (peak(&row) - peak(&original)).abs() < 1.0e-3,
            "rows stay peak-normalized after smoothing",
        );
        assert_eq!(row[0], original[0], "DC bin is untouched");
    }
}
