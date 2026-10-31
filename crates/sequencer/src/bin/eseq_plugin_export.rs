//! Export an eseq instrument as CLAP / VST3 plugin bundles.
//!
//! `eseq_plugin_export "Drums/Latent Kick" [--format clap,vst3] [--out DIR]`
//! See docs/plugin-export-spec.md.

use std::path::PathBuf;

use sequencer::plugin_export::{ExportOptions, PluginFormat, export_instrument_plugin};

fn usage() {
    eprintln!(
        "Usage: eseq_plugin_export <instrument-name-or-folder> [options]\n\
         \n\
         Options:\n\
           --format LIST      clap,vst3 (default: both)\n\
           --out DIR          Write bundles to DIR (default: ~/Library/Audio/Plug-Ins/<format>)\n\
           --polyphony N      Voices per instance (default: 8 drums, 16 otherwise)\n\
           --preset NAME      Plugin defaults from this preset\n\
           --param name=value Override a default; repeatable\n\
           --name NAME        Plugin display name"
    );
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
    let Some(target) = args.next().filter(|a| a != "-h" && a != "--help") else {
        usage();
        std::process::exit(2);
    };
    let mut options = ExportOptions::new(target);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("{arg} requires a value");
                std::process::exit(2);
            })
        };
        match arg.as_str() {
            "--format" => {
                options.formats = value()
                    .split(',')
                    .map(|f| {
                        PluginFormat::parse(f).unwrap_or_else(|| {
                            eprintln!("unknown format '{f}'");
                            std::process::exit(2);
                        })
                    })
                    .collect();
            }
            "--out" => options.out_dir = Some(PathBuf::from(value())),
            "--polyphony" => options.polyphony = value().parse().ok(),
            "--preset" => options.preset = Some(value()),
            "--name" => options.name = Some(value()),
            "--param" => {
                let raw = value();
                let Some((name, v)) = raw.split_once('=').and_then(|(n, v)| Some((n, v.parse::<f32>().ok()?))) else {
                    eprintln!("--param wants name=value, got '{raw}'");
                    std::process::exit(2);
                };
                options.param_values.insert(name.to_string(), v);
            }
            other => {
                eprintln!("unknown option '{other}'");
                usage();
                std::process::exit(2);
            }
        }
    }
    match export_instrument_plugin(&options) {
        Ok(report) => {
            println!("exported {} ({}), {} params, panel: {}", report.name, report.plugin_id, report.params, report.has_panel);
            for bundle in report.bundles {
                println!("  {}", bundle.display());
            }
        }
        Err(err) => {
            eprintln!("export failed: {err}");
            std::process::exit(1);
        }
    }
}
