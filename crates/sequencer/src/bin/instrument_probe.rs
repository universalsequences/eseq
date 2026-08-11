use std::path::{Path, PathBuf};

use sequencer::lisp_host::{self, InstrumentParamEvent, InstrumentRenderOptions};

fn usage() {
    eprintln!(
        "Usage: instrument_probe <instrument-name-or-path> [options]\n\
         \n\
         Options:\n\
           --frames N          Render frame count (default: 44100)\n\
           --block-size N      Process block size (default: 128)\n\
           --sample-rate N     Sample rate (default: 44100)\n\
           --midi-note N       MIDI note to render (default: 69)\n\
           --preset NAME       Load an instrument preset by name or id\n\
           --velocity V        Velocity 0..1 (default: 1)\n\
           --gate-frames N     Gate duration in frames (default: frames)\n\
           --param name=value  Override an instrument parameter; repeatable\n\
           --param-at frame:name=value\n\
                               Apply an instrument parameter at a render frame; repeatable\n\
           --input N=value     Fill input channel N with value; repeatable\n\
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
        let source_path = if path.is_dir() {
            path.join("dsp.lisp")
        } else {
            path.to_path_buf()
        };
        let source = std::fs::read_to_string(&source_path)
            .map_err(|e| format!("failed to read '{}': {e}", source_path.display()))?;
        let asset_base = if path.is_dir() {
            Some(path.to_path_buf())
        } else {
            path.parent().map(|parent| parent.to_path_buf())
        };
        return Ok((source, asset_base, target.to_string()));
    }

    let source = lisp_host::load_instrument_source(target).map_err(|e| e.to_string())?;
    let asset_base = lisp_host::instrument_source_path(target)
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    Ok((source, asset_base, target.to_string()))
}

fn main() {
    if let Err(e) = sequencer::paths::enter_sequencer_dir() {
        eprintln!("failed to enter sequencer crate directory: {e}");
        std::process::exit(1);
    }
    if let Err(e) = sequencer::app_paths::init_dev() {
        eprintln!("failed to initialize app paths: {e}");
        std::process::exit(1);
    }

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
    let mut preset_name: Option<String> = None;
    let mut velocity = 1.0f32;
    let mut gate_frames: Option<usize> = None;
    let mut param_overrides = Vec::new();
    let mut param_events = Vec::new();
    let mut input_overrides = Vec::new();
    let mut min_peak: Option<f32> = None;
    let mut min_rms: Option<f32> = None;
    let mut json = false;

    while let Some(arg) = args.next() {
        let result: Result<(), String> = match arg.as_str() {
            "--frames" => parse_value("--frames", args.next()).map(|v| frames = v),
            "--block-size" => parse_value("--block-size", args.next()).map(|v| block_size = v),
            "--sample-rate" => parse_value("--sample-rate", args.next()).map(|v| sample_rate = v),
            "--midi-note" => parse_value("--midi-note", args.next()).map(|v| midi_note = v),
            "--preset" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--preset requires a preset name or id".to_string());
                value.map(|value| preset_name = Some(value))
            }
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
            "--param-at" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--param-at requires frame:name=value".to_string());
                value.and_then(|raw| {
                    let Some((frame, assignment)) = raw.split_once(':') else {
                        return Err("--param-at requires frame:name=value".to_string());
                    };
                    let frame = frame
                        .parse::<usize>()
                        .map_err(|_| format!("invalid --param-at frame '{frame}'"))?;
                    let Some((name, value)) = assignment.split_once('=') else {
                        return Err("--param-at requires frame:name=value".to_string());
                    };
                    let value = value
                        .parse::<f32>()
                        .map_err(|_| format!("invalid --param-at value for '{name}'"))?;
                    param_events.push(InstrumentParamEvent {
                        frame,
                        name: name.to_string(),
                        value,
                    });
                    Ok(())
                })
            }
            "--input" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--input requires channel=value".to_string());
                value.and_then(|raw| {
                    let Some((channel, value)) = raw.split_once('=') else {
                        return Err("--input requires channel=value".to_string());
                    };
                    let channel = channel
                        .parse::<usize>()
                        .map_err(|_| format!("invalid --input channel '{channel}'"))?;
                    let value = value
                        .parse::<f32>()
                        .map_err(|_| format!("invalid --input value for channel {channel}"))?;
                    input_overrides.push((channel, value));
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

    let result = match lisp_host::compile_and_load_instrument_with_asset_base(
        &source,
        sample_rate,
        asset_base.as_deref(),
    ) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::exit(1);
        }
    };

    let mut effective_midi_note = midi_note;
    let mut merged_param_overrides = Vec::new();
    if let Some(name) = preset_name.as_deref() {
        let presets = match lisp_host::load_instrument_presets(&label) {
            Ok(presets) => presets,
            Err(error) => {
                eprintln!("Error: failed to load presets for '{label}': {error}");
                std::process::exit(1);
            }
        };
        let Some(preset) = presets
            .iter()
            .find(|preset| preset.name == name || preset.id == name)
        else {
            eprintln!("Error: preset '{name}' not found for '{label}'");
            std::process::exit(1);
        };
        effective_midi_note += preset.base_note_offset;
        for param in &result.manifest.params {
            if let Some(value) = preset.params.get(&param.name) {
                merged_param_overrides
                    .push((param.name.clone(), value.clamp(param.min, param.max)));
            }
        }
    }
    merged_param_overrides.extend(param_overrides);

    let options = InstrumentRenderOptions {
        sample_rate,
        block_size,
        frames,
        midi_note: effective_midi_note,
        velocity,
        gate_frames: gate_frames.unwrap_or(frames),
        voice_index: 0,
        param_overrides: merged_param_overrides,
        param_events,
        input_overrides,
    };

    let report =
        match lisp_host::render_loaded_instrument_for_test(&result.manifest, &result.lib, &options)
        {
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
    if report.non_finite_samples > 0 || report.non_finite_state_slots > 0 {
        failed = true;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "instrument": label,
                "preset": preset_name,
                "midi_note": midi_note,
                "effective_midi_note": effective_midi_note,
                "frames": report.frames,
                "peak": report.peak,
                "rms": report.rms,
                "mean_abs": report.mean_abs,
                "nonzero_frames": report.nonzero_frames,
                "first_nonzero_frame": report.first_nonzero_frame,
                "non_finite_samples": report.non_finite_samples,
                "first_non_finite_frame": report.first_non_finite_frame,
                "non_finite_state_slots": report.non_finite_state_slots,
                "first_non_finite_state_slot": report.first_non_finite_state_slot,
                "first_samples": report.first_samples,
            })
        );
    } else {
        println!("instrument: {label}");
        if let Some(name) = preset_name.as_deref() {
            println!("preset: {name}");
            println!("effective_midi_note: {:.3}", effective_midi_note);
        }
        println!("frames: {}", report.frames);
        println!("peak: {:.8}", report.peak);
        println!("rms: {:.8}", report.rms);
        println!("mean_abs: {:.8}", report.mean_abs);
        println!("nonzero_frames: {}", report.nonzero_frames);
        println!("first_nonzero_frame: {:?}", report.first_nonzero_frame);
        println!("non_finite_samples: {}", report.non_finite_samples);
        println!(
            "first_non_finite_frame: {:?}",
            report.first_non_finite_frame
        );
        println!("non_finite_state_slots: {}", report.non_finite_state_slots);
        println!(
            "first_non_finite_state_slot: {:?}",
            report.first_non_finite_state_slot
        );
        println!("first_samples: {:?}", report.first_samples);
    }

    if failed {
        std::process::exit(1);
    }
}
