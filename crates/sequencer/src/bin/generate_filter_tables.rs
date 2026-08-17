//! Deterministic Filter Table factory-preset generator (eseq-dtx.7).
//!
//! Bakes the original factory library defined in
//! `effects::filter_table_presets` into versioned `.fltab` assets, and
//! optionally emits review probes: a PPM magnitude heatmap per preset, plus
//! audio probes (white-noise-ish default probe with a full frame sweep, and
//! a sawtooth-like harmonic stack) rendered through the real bundled DSP.
//!
//! generate_filter_tables [--out <dir>] [--probes <dir>] [--no-audio]
//!
//! `--out` defaults to the bundled factory directory
//! (`crates/sequencer/assets/filter-tables`). Same code, same output: the
//! `bundled_factory_assets_match_their_recipes` test fails if the baked
//! files drift from the recipes in code.

use std::path::PathBuf;

use sequencer::effects::filter_table::{dsp_source, FRAMES, N, NBINS};
use sequencer::effects::filter_table_asset::bundled_asset_dir;
use sequencer::effects::filter_table_presets::{bake, factory_presets, write_preset};
use sequencer::lisp_host::{
    dgenlisp_tool_path, render_effect_source_for_test, EffectRenderOptions,
    InstrumentParamEvent,
};

fn main() {
    let mut out_dir = bundled_asset_dir();
    let mut probes_dir: Option<PathBuf> = None;
    let mut audio = true;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--out" => out_dir = PathBuf::from(args.next().expect("--out <dir>")),
            "--probes" => {
                probes_dir = Some(PathBuf::from(args.next().expect("--probes <dir>")))
            }
            "--no-audio" => audio = false,
            other => {
                eprintln!("unknown flag {other}");
                eprintln!("usage: generate_filter_tables [--out <dir>] [--probes <dir>] [--no-audio]");
                std::process::exit(2);
            }
        }
    }

    std::fs::create_dir_all(&out_dir).expect("create output dir");
    if let Some(dir) = &probes_dir {
        std::fs::create_dir_all(dir).expect("create probes dir");
    }
    let render_audio = audio && probes_dir.is_some() && dgenlisp_tool_path().exists();
    if audio && probes_dir.is_some() && !render_audio {
        eprintln!("DGenLisp tool not found; skipping audio probes");
    }

    for preset in factory_presets() {
        let table = bake(&preset.recipe).expect(preset.stem);
        write_preset(&out_dir, &preset).expect(preset.stem);
        println!(
            "wrote {}/{}.fltab — {}",
            out_dir.display(),
            preset.stem,
            preset.intent
        );

        let Some(probes) = &probes_dir else { continue };
        write_heatmap(&probes.join(format!("{}.ppm", preset.stem)), &table.data);
        if render_audio {
            write_audio_probes(probes, preset.stem, &table.data);
        }
    }
}

/// dB-mapped grayscale heatmap: one row per frame (frame 0 on top), one
/// column per bin. PPM (P6) so review needs no image dependencies.
fn write_heatmap(path: &std::path::Path, data: &[f32]) {
    let mut bytes = format!("P6\n{NBINS} {FRAMES}\n255\n").into_bytes();
    for frame in 0..FRAMES {
        for bin in 0..NBINS {
            let magnitude = data[frame * NBINS + bin].max(1.0e-4);
            let db = 20.0 * magnitude.log10(); // 0 dB .. -80 dB
            let level = ((db + 80.0) / 80.0).clamp(0.0, 1.0);
            let value = (level.powf(0.8) * 255.0) as u8;
            bytes.extend_from_slice(&[value, value, value]);
        }
    }
    std::fs::write(path, bytes).expect("write heatmap");
}

fn write_audio_probes(dir: &std::path::Path, stem: &str, table: &[f32]) {
    const SAMPLE_RATE: u32 = 44_100;
    const PROBE_FRAMES: usize = 6 * SAMPLE_RATE as usize;
    // Sweep the whole table over the probe: 32 frame steps (the DSP
    // smooths/crossfades between hop responses).
    let sweep_events = (0..32)
        .map(|step| InstrumentParamEvent {
            frame: step * PROBE_FRAMES / 32,
            name: "frame".to_string(),
            value: step as f32 / 31.0,
        })
        .collect::<Vec<_>>();
    // Sawtooth-like stack: harmonics of 110 Hz at 1/n amplitude.
    let saw_tones = (1..=24)
        .map(|harmonic| (0usize, 110.0 * harmonic as f32, 0.35 / harmonic as f32))
        .collect::<Vec<_>>();

    let identity_cutoff = 24.0 * SAMPLE_RATE as f32 / N as f32;
    for (label, tones) in [("probe", Vec::new()), ("saw", saw_tones)] {
        let report = render_effect_source_for_test(
            dsp_source(),
            &EffectRenderOptions {
                sample_rate: SAMPLE_RATE,
                block_size: 512,
                frames: PROBE_FRAMES,
                param_overrides: vec![
                    ("frame".to_string(), 0.0),
                    ("cutoff".to_string(), identity_cutoff),
                    ("resonance".to_string(), 0.0),
                    ("mix".to_string(), 1.0),
                ],
                param_events: sweep_events.clone(),
                input_tones: tones,
                tensor_overrides: vec![("table_magnitudes".to_string(), table.to_vec())],
                input_overrides: Vec::new(),
            },
        )
        .expect("render audio probe");

        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let path = dir.join(format!("{stem}-{label}.wav"));
        let mut writer = hound::WavWriter::create(&path, spec).expect("create probe wav");
        for sample in &report.samples {
            writer.write_sample(*sample).expect("write sample");
        }
        writer.finalize().expect("finalize probe wav");
        println!("  probe {}", path.display());
    }
}
