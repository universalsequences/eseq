//! Host-aware compilation for external effect authoring/audition tools.
//! Includes ESeq declarations, preamble, macro imports and asset resolution.
use std::path::PathBuf;

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("Usage: dgen_effect_compile SOURCE [--sample-rate HZ]")?;
    let mut rate = 44_100u32;
    if let Some(flag) = args.next() {
        if flag != "--sample-rate" { return Err(format!("Unknown option: {flag}")); }
        rate = args.next().ok_or("Missing sample rate")?.parse().map_err(|_| "Invalid sample rate")?;
    }
    if args.next().is_some() || rate == 0 { return Err("Invalid arguments or zero sample rate".into()); }
    // Resolve before app path initialization; never depend on the caller's cwd
    // after initialization for source-local asset lookup.
    let path = PathBuf::from(path).canonicalize().map_err(|e| e.to_string())?;
    sequencer::app_paths::init_dev().map_err(|e| e.to_string())?;
    let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let json = sequencer::lisp_host::compile_lisp_with_asset_base(&source, rate, path.parent())?;
    let manifest = sequencer::lisp_host::parse_manifest(&json)?;
    // Standalone consumers cannot know the host's scratch directory. Return
    // an absolute library path; the remaining compiler/host manifest is intact.
    let mut json: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    json["dylib"] = serde_json::json!(manifest.dylib_path);
    println!("{}", serde_json::to_string(&json).map_err(|e| e.to_string())?);
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
