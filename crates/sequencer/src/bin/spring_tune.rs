//! Offline spring renderer. Raw f32 output on stdout, or float WAV via --wav.
//!
//! spring_tune --voice grampian --dump-params
//! spring_tune --voice grampian --params fit.json --seconds 5 --wav impulse.wav
//! spring_tune --voice grampian --space-echo --input dry.wav --amp 1
//!             --host-settings host.json --seconds 4 --wav wet.wav
//!
//! With --input, --seconds is appended silence; otherwise it is IR duration.
//! --space-echo exercises the production node (stereo output), not just a tank.
//! --benchmark N repeats complete renders and reports median wall time.

use sequencer::effects::spring::{
    grampian, render_space_echo, spring_tank_process, SpaceEchoRenderSettings, SpringCoeffs,
    SpringParams, SPRING_TANK_STATE_LEN,
};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sr = 48000u32;
    let mut seconds = 6.5f32;
    let mut amp = 0.25f32;
    let mut params_path = None;
    let mut wav_path = None;
    let mut input_path = None;
    let mut host_path = None;
    let mut voice = String::from("re201");
    let mut tension = 0.5f32;
    let mut explicit_tension = false;
    let mut dump_params = false;
    let mut host = false;
    let mut repeats = 1usize;
    let mut benchmark = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing value for {arg}"))
        };
        match arg.as_str() {
            "--sr" => sr = value()?.parse()?,
            "--seconds" => seconds = value()?.parse()?,
            "--amp" => amp = value()?.parse()?,
            "--params" => params_path = Some(value()?),
            "--wav" => wav_path = Some(value()?),
            "--input" => input_path = Some(value()?),
            "--host-settings" => host_path = Some(value()?),
            "--voice" => voice = value()?,
            "--tension" => {
                tension = value()?.parse()?;
                explicit_tension = true;
            }
            "--dump-params" => dump_params = true,
            "--space-echo" => host = true,
            "--benchmark" => {
                repeats = value()?.parse()?;
                benchmark = true;
            }
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    if !(8000..=192000).contains(&sr)
        || !seconds.is_finite()
        || !(0.0..=60.0).contains(&seconds)
        || seconds * (sr as f32) < 1.0
        || !amp.is_finite()
        || !tension.is_finite()
        || !(0.0..=1.0).contains(&tension)
        || !(1..=100).contains(&repeats)
    {
        return Err("invalid rate, duration, amplitude, tension or benchmark count".into());
    }
    if !matches!(voice.as_str(), "re201" | "grampian" | "king-tubby-v1") {
        return Err(format!("unknown voice {voice}").into());
    }
    if host && (params_path.is_some() || voice == "king-tubby-v1" || dump_params) {
        return Err("production render uses shipped RE-201/Grampian parameters; no --params or --dump-params".into());
    }
    if host && explicit_tension {
        return Err("for production renders, set tension in --host-settings, not --tension".into());
    }
    if host_path.is_some() && !host {
        return Err("--host-settings requires --space-echo".into());
    }
    let text = params_path.map(std::fs::read_to_string).transpose()?;
    let grampian_params: grampian::Params = if voice == "grampian" {
        text.as_ref()
            .map(|s| serde_json::from_str(s))
            .transpose()?
            .unwrap_or_default()
    } else {
        grampian::Params::default()
    };
    grampian_params.validate()?;
    let legacy_params = if voice != "grampian" {
        text.as_ref()
            .map(|s| serde_json::from_str::<SpringParams>(s))
            .transpose()?
            .unwrap_or_else(|| {
                if voice == "re201" {
                    SpringParams::re201()
                } else {
                    SpringParams::king_tubby_v1()
                }
            })
    } else {
        SpringParams::re201()
    }
    .with_tension(tension);
    if dump_params {
        let json = if voice == "grampian" {
            serde_json::to_string_pretty(&grampian_params)?
        } else {
            serde_json::to_string_pretty(&legacy_params)?
        };
        println!("{json}");
        return Ok(());
    }
    let mut input: Vec<[f32; 2]> = Vec::new();
    if let Some(path) = input_path {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        if spec.sample_rate != sr || !(1..=2).contains(&spec.channels) {
            return Err(
                "input WAV must match --sr and have one or two channels (resample with ffmpeg)"
                    .into(),
            );
        }
        let samples = if spec.sample_format == hound::SampleFormat::Float {
            reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?
        } else {
            let scale = 2.0f32.powi(spec.bits_per_sample as i32 - 1);
            reader
                .samples::<i32>()
                .map(|s| s.map(|x| x as f32 / scale))
                .collect::<Result<Vec<_>, _>>()?
        };
        for frame in samples.chunks_exact(spec.channels as usize) {
            input.push([frame[0] * amp, frame[frame.len() - 1] * amp]);
        }
        input.resize(input.len() + (sr as f32 * seconds) as usize, [0.0; 2]);
    } else {
        input.resize((sr as f32 * seconds) as usize, [0.0; 2]);
        input[0] = [amp; 2];
    }
    if input.iter().flatten().any(|x| !x.is_finite()) {
        return Err("input contains non-finite samples".into());
    }
    let mut settings: SpaceEchoRenderSettings = host_path
        .map(std::fs::read_to_string)
        .transpose()?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    settings.sample_rate = sr;
    settings.grampian = voice == "grampian";
    // The explicit CLI tension takes precedence only for isolated tanks;
    // production controls (including tension) all live in --host-settings.
    let mono: Vec<f32> = input.iter().map(|v| (v[0] + v[1]) * 0.5).collect();
    let mut output = Vec::new();
    let mut times = Vec::new();
    for _ in 0..repeats {
        let start = std::time::Instant::now();
        output = if host {
            render_space_echo(&input, &settings)?
                .into_iter()
                .flatten()
                .collect()
        } else if voice == "grampian" {
            grampian::render(&grampian_params, sr as f32, &mono, tension)?
        } else {
            let c = SpringCoeffs::new(&legacy_params, sr as f32);
            let mut state = vec![0.0; SPRING_TANK_STATE_LEN];
            mono.iter()
                .map(|&x| spring_tank_process(x, &c, &mut state))
                .collect()
        };
        times.push(start.elapsed().as_secs_f64());
    }
    if benchmark {
        times.sort_by(f64::total_cmp);
        let median = times[times.len() / 2];
        eprintln!(
            "median {:.3} ms; {:.3}% realtime; {} runs, {} frames at {} Hz",
            median * 1000.0,
            100.0 * median / (input.len() as f64 / sr as f64),
            repeats,
            input.len(),
            sr
        );
    }
    if let Some(path) = wav_path {
        let spec = hound::WavSpec {
            channels: if host { 2 } else { 1 },
            sample_rate: sr,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec)?;
        for s in output {
            w.write_sample(s)?;
        }
        w.finalize()?;
    } else {
        let mut stdout = std::io::stdout().lock();
        let bytes: Vec<u8> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
        stdout.write_all(&bytes)?;
    }
    Ok(())
}
