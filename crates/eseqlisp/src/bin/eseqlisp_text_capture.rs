use std::path::PathBuf;

use eseqlisp::text_capture::{PlatformTextCaptureSource, capture};

fn usage() -> &'static str {
    "usage: eseqlisp_text_capture --name <capture-name> [--output-dir <directory>]"
}

fn main() {
    if let Err(error) = run() {
        eprintln!("eseqlisp_text_capture: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    let mut output_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/text-capture/goldens");
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--name" => name = Some(args.next().ok_or_else(|| usage().to_string())?),
            "--output-dir" => {
                output_dir = PathBuf::from(args.next().ok_or_else(|| usage().to_string())?)
            }
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}\n{}", usage())),
        }
    }
    let name = name.ok_or_else(|| usage().to_string())?;
    let paths = capture(&PlatformTextCaptureSource, &output_dir, &name)?;
    println!("{}", paths.metrics.display());
    println!("{}", paths.screenshot.display());
    Ok(())
}
