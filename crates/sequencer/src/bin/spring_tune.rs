//! Offline render harness for tuning the dispersive spring reverb against a
//! reference impulse response. Driven by `scripts/spring_tune.py`.
//!
//! ```text
//! spring_tune --params params.json [--sr 44100] [--seconds 6.5] [--amp 0.25] [--wav out.wav]
//! ```
//!
//! Renders one mono tank impulse response. Default output is raw
//! little-endian f32 samples on stdout (fast path for the optimizer); `--wav`
//! writes a wav file instead.

use sequencer::effects::spring::{render_impulse, SpringParams};
use std::io::Write;

fn main() {
    let mut sr = 44_100.0f32;
    let mut seconds = 6.5f32;
    let mut amp = 0.25f32;
    let mut params_path: Option<String> = None;
    let mut wav_path: Option<String> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let need = |i: usize| {
            args.get(i + 1)
                .unwrap_or_else(|| panic!("missing value for {}", args[i]))
                .clone()
        };
        match args[i].as_str() {
            "--sr" => {
                sr = need(i).parse().expect("bad --sr");
                i += 2;
            }
            "--seconds" => {
                seconds = need(i).parse().expect("bad --seconds");
                i += 2;
            }
            "--amp" => {
                amp = need(i).parse().expect("bad --amp");
                i += 2;
            }
            "--params" => {
                params_path = Some(need(i));
                i += 2;
            }
            "--wav" => {
                wav_path = Some(need(i));
                i += 2;
            }
            other => panic!("unknown arg {other}"),
        }
    }

    let params: SpringParams = match params_path {
        Some(p) => {
            let text = std::fs::read_to_string(&p).expect("read params json");
            serde_json::from_str(&text).expect("parse params json")
        }
        None => SpringParams::re201(),
    };

    let ir = render_impulse(&params, sr, seconds, amp);

    match wav_path {
        Some(path) => {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: sr as u32,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut w = hound::WavWriter::create(&path, spec).expect("create wav");
            for s in &ir {
                w.write_sample(*s).expect("write sample");
            }
            w.finalize().expect("finalize wav");
        }
        None => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let bytes: Vec<u8> = ir.iter().flat_map(|s| s.to_le_bytes()).collect();
            lock.write_all(&bytes).expect("write stdout");
        }
    }
}
