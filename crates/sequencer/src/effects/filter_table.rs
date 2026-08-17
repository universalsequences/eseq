//! Filter Table magnitude-bank preprocessing and live-instance state.
//!
//! User audio is interpreted as up to 64 consecutive single-cycle frames.
//! Each selected frame is DC-centered, RMS-normalized, transformed offline,
//! reduced to its non-negative half-spectrum, and peak-normalized. Only
//! magnitudes enter the live effect: zero phase is synthesized by the DSP so
//! interpolation between frames cannot suffer complex-phase cancellation.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

pub const NAME: &str = "Filter Table";
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
    fn new(data: Vec<f32>) -> Result<Self, String> {
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

/// Build a fixed-size magnitude bank from any supported audio file. For audio
/// shorter than one FFT frame, the entire clip is periodically resampled to one
/// cycle. Longer files provide evenly-spaced N-sample frames across their full
/// duration. Mono and stereo files share the same deterministic downmix path.
pub fn prepare_table(path: &Path) -> Result<MagnitudeTable, String> {
    let decoded = crate::sample_import::decode_audio_file(path)?;
    let channels = usize::from(decoded.channels.max(1));
    let mut mono = Vec::with_capacity(decoded.samples.len() / channels);
    for frame in decoded.samples.chunks_exact(channels) {
        mono.push(frame.iter().copied().sum::<f32>() / channels as f32);
    }
    if mono.is_empty() {
        return Err("table source contains no complete audio frames".to_string());
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut data = Vec::with_capacity(TABLE_LEN);
    let mut work = vec![Complex32::default(); N];
    let mut has_audible_frame = false;

    for table_frame in 0..FRAMES {
        fill_time_frame(&mono, table_frame, &mut work);
        if !normalize_time_frame(&mut work) {
            data.extend(std::iter::repeat(0.0).take(NBINS));
            continue;
        }
        has_audible_frame = true;
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
    if !has_audible_frame {
        return Err("table source is silent after DC removal".to_string());
    }
    MagnitudeTable::new(data)
}

fn fill_time_frame(source: &[f32], table_frame: usize, output: &mut [Complex32]) {
    debug_assert_eq!(output.len(), N);
    if source.len() < N {
        // Treat a short source as one periodic cycle and linearly resample it.
        let scale = source.len() as f64 / N as f64;
        for (index, value) in output.iter_mut().enumerate() {
            let position = index as f64 * scale;
            let lo = position.floor() as usize % source.len();
            let hi = (lo + 1) % source.len();
            let fraction = (position - position.floor()) as f32;
            value.re = source[lo] + (source[hi] - source[lo]) * fraction;
            value.im = 0.0;
        }
        return;
    }

    let max_start = source.len() - N;
    let start = if FRAMES == 1 {
        0
    } else {
        ((table_frame as u128 * max_start as u128) / (FRAMES - 1) as u128) as usize
    };
    for (value, sample) in output.iter_mut().zip(&source[start..start + N]) {
        *value = Complex32::new(*sample, 0.0);
    }
}

fn normalize_time_frame(frame: &mut [Complex32]) -> bool {
    let mean = frame.iter().map(|value| value.re as f64).sum::<f64>() / frame.len() as f64;
    let energy = frame
        .iter()
        .map(|value| {
            let centered = value.re as f64 - mean;
            centered * centered
        })
        .sum::<f64>()
        / frame.len() as f64;
    let rms = energy.sqrt();
    if rms <= 1.0e-12 {
        frame.fill(Complex32::default());
        return false;
    }
    let gain = (1.0 / rms) as f32;
    for value in frame {
        value.re = (value.re - mean as f32) * gain;
        value.im = 0.0;
    }
    true
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
mod tests {
    use super::*;

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
        let mut first = vec![Complex32::default(); N];
        let mut last = vec![Complex32::default(); N];
        fill_time_frame(&source, 0, &mut first);
        fill_time_frame(&source, FRAMES - 1, &mut last);
        assert_eq!(first, last);
    }

    #[test]
    fn audio_source_is_converted_to_peak_normalized_magnitudes() {
        let path = std::env::temp_dir().join(format!(
            "eseq-filter-table-{}-{}.wav",
            std::process::id(),
            N,
        ));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
        for index in 0..N {
            let phase = std::f32::consts::TAU * 24.0 * index as f32 / N as f32;
            writer.write_sample(phase.sin()).expect("write sample");
        }
        writer.finalize().expect("finalize wav");

        let table = prepare_table(&path).expect("prepare magnitude table");
        let _ = std::fs::remove_file(path);
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
}
