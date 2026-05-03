use std::path::{Path, PathBuf};

use sequencer::lisp_effect::{self, InstrumentRenderOptions};

fn usage() {
    eprintln!(
        "Usage: instrument_probe <instrument-name-or-path> [options]\n\
         \n\
         Options:\n\
           --frames N          Render frame count (default: 44100)\n\
           --block-size N      Process block size (default: 128)\n\
           --sample-rate N     Sample rate (default: 44100)\n\
           --midi-note N       MIDI note to render (default: 69)\n\
           --velocity V        Velocity 0..1 (default: 1)\n\
           --gate-frames N     Gate duration in frames (default: frames)\n\
           --param name=value  Override an instrument parameter; repeatable\n\
           --min-peak V        Fail if peak is below V\n\
           --min-rms V         Fail if RMS is below V\n\
           --json              Print JSON instead of text"
    );
}

fn parse_value<T: std::str::FromStr>(flag: &str, value: Option<String>) -> Result<T, String> {
    value
        .ok_or_else(|| format!("{flag} requires a value"))?
        .parse::<T>()
        .map_err(|_| format!("invalid value for {flag}"))
}

fn resolve_source(target: &str) -> Result<(String, Option<PathBuf>, String), String> {
    let path = Path::new(target);
    if path.exists() {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
        let asset_base = path.parent().map(|parent| parent.to_path_buf());
        return Ok((source, asset_base, target.to_string()));
    }

    let source = lisp_effect::load_instrument_source(target).map_err(|e| e.to_string())?;
    let asset_base = lisp_effect::instrument_source_path(target)
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    Ok((source, asset_base, target.to_string()))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else {
        usage();
        std::process::exit(2);
    };
    if target == "-h" || target == "--help" {
        usage();
        return;
    }

    let mut frames = 44_100usize;
    let mut block_size = 128usize;
    let mut sample_rate = 44_100u32;
    let mut midi_note = 69.0f32;
    let mut velocity = 1.0f32;
    let mut gate_frames: Option<usize> = None;
    let mut param_overrides = Vec::new();
    let mut min_peak: Option<f32> = None;
    let mut min_rms: Option<f32> = None;
    let mut json = false;

    while let Some(arg) = args.next() {
        let result: Result<(), String> = match arg.as_str() {
            "--frames" => parse_value("--frames", args.next()).map(|v| frames = v),
            "--block-size" => parse_value("--block-size", args.next()).map(|v| block_size = v),
            "--sample-rate" => parse_value("--sample-rate", args.next()).map(|v| sample_rate = v),
            "--midi-note" => parse_value("--midi-note", args.next()).map(|v| midi_note = v),
            "--velocity" => parse_value("--velocity", args.next()).map(|v| velocity = v),
            "--gate-frames" => {
                parse_value("--gate-frames", args.next()).map(|v| gate_frames = Some(v))
            }
            "--min-peak" => parse_value("--min-peak", args.next()).map(|v| min_peak = Some(v)),
            "--min-rms" => parse_value("--min-rms", args.next()).map(|v| min_rms = Some(v)),
            "--param" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--param requires name=value".to_string());
                value.and_then(|raw| {
                    let Some((name, value)) = raw.split_once('=') else {
                        return Err("--param requires name=value".to_string());
                    };
                    let value = value
                        .parse::<f32>()
                        .map_err(|_| format!("invalid --param value for '{name}'"))?;
                    param_overrides.push((name.to_string(), value));
                    Ok(())
                })
            }
            "--json" => {
                json = true;
                Ok(())
            }
            _ => Err(format!("unknown option '{arg}'")),
        };
        if let Err(error) = result {
            eprintln!("Error: {error}");
            usage();
            std::process::exit(2);
        }
    }

    let (source, asset_base, label) = match resolve_source(&target) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::exit(1);
        }
    };

    let options = InstrumentRenderOptions {
        sample_rate,
        block_size,
        frames,
        midi_note,
        velocity,
        gate_frames: gate_frames.unwrap_or(frames),
        voice_index: 0,
        param_overrides,
    };

    let report = match lisp_effect::render_instrument_source_for_test(
        &source,
        asset_base.as_deref(),
        &options,
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::exit(1);
        }
    };

    let mut failed = false;
    if let Some(min_peak) = min_peak {
        if report.peak < min_peak {
            failed = true;
        }
    }
    if let Some(min_rms) = min_rms {
        if report.rms < min_rms {
            failed = true;
        }
    }

    if json {
        println!(
            "{{\"instrument\":{:?},\"frames\":{},\"peak\":{},\"rms\":{},\"mean_abs\":{},\"nonzero_frames\":{},\"first_nonzero_frame\":{:?},\"first_samples\":{:?}}}",
            label,
            report.frames,
            report.peak,
            report.rms,
            report.mean_abs,
            report.nonzero_frames,
            report.first_nonzero_frame,
            report.first_samples
        );
    } else {
        println!("instrument: {label}");
        println!("frames: {}", report.frames);
        println!("peak: {:.8}", report.peak);
        println!("rms: {:.8}", report.rms);
        println!("mean_abs: {:.8}", report.mean_abs);
        println!("nonzero_frames: {}", report.nonzero_frames);
        println!("first_nonzero_frame: {:?}", report.first_nonzero_frame);
        println!("first_samples: {:?}", report.first_samples);
    }

    if failed {
        std::process::exit(1);
    }
}
