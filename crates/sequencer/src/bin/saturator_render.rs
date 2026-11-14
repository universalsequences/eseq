//! Saturator offline render harness: the measurement and ear-tuning loop for
//! the Saturator builtin (and so for its solved-circuit curves, `effects/triode.rs` and siblings).
//!
//! saturator_render --in in.wav --out out.wav [--set "name=value"]... [--bench]
//!
//! `name` is a Saturator param name as the descriptor spells it ("drive",
//! "curve", "amount", "bias", "tone", "output", "dry/wet", "auto gain");
//! unset params keep the effect's defaults. `--bench` prints the render time
//! as a fraction of real time. `tools/saturator/measure.py` drives this.

use sequencer::effects::{roar, EffectDescriptor};

fn main() {
    let mut input = None;
    let mut output = None;
    let mut sets: Vec<(String, f32)> = Vec::new();
    let mut bench = false;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--in" => input = args.next(),
            "--out" => output = args.next(),
            "--bench" => bench = true,
            "--set" => {
                let pair = args.next().expect("--set name=value");
                let (name, value) = pair.split_once('=').expect("--set name=value");
                sets.push((name.to_string(), value.parse().expect("numeric value")));
            }
            other => panic!("unknown flag {other}"),
        }
    }
    let input = input.expect("--in file.wav");
    let output = output.expect("--out file.wav");

    let descriptor = EffectDescriptor::builtin_saturator();
    let mut params: Vec<(u32, f32)> = descriptor
        .params
        .iter()
        .filter(|param| param.name != "enabled")
        .map(|param| (param.node_param_idx, param.default))
        .collect();
    for (name, value) in &sets {
        let param = descriptor
            .params
            .iter()
            .find(|param| param.name == *name)
            .unwrap_or_else(|| panic!("Saturator has no param {name:?}"));
        params.push((param.node_param_idx, value.clamp(param.min, param.max)));
    }

    let mut reader = hound::WavReader::open(&input).expect("open input wav");
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 * scale).collect()
        }
    };
    let left: Vec<f32> = samples.iter().step_by(channels).copied().collect();
    let right: Vec<f32> = if channels > 1 {
        samples.iter().skip(1).step_by(channels).copied().collect()
    } else {
        left.clone()
    };

    let started = std::time::Instant::now();
    let (out_left, out_right) = roar::render_offline(spec.sample_rate, &params, &left, &right);
    if bench {
        let audio_seconds = left.len() as f64 / spec.sample_rate as f64;
        eprintln!(
            "cpu {:.2}% of real time (stereo, {:.1}s)",
            100.0 * started.elapsed().as_secs_f64() / audio_seconds,
            audio_seconds
        );
    }

    let mut writer = hound::WavWriter::create(
        &output,
        hound::WavSpec {
            channels: 2,
            sample_rate: spec.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .expect("create output wav");
    for (l, r) in out_left.iter().zip(&out_right) {
        writer.write_sample(*l).unwrap();
        writer.write_sample(*r).unwrap();
    }
    writer.finalize().unwrap();
}
