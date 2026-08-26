//! Write the deterministic MSL shader captures for one macOS host.
//!
//! ```text
//! cargo run -p eseqlisp --bin eseqlisp_metal_shader_capture -- \
//!     --name msl-macos-arm64 \
//!     --output-dir crates/eseqlisp/tests/fixtures/shader-capture/goldens
//! ```
//!
//! The Metal half of `eseq-linux.25`; the WGSL half is
//! `eseqlisp_shader_capture`. Exits non-zero when no Metal device is
//! available, so a run on a machine without one fails loudly instead of
//! writing an empty capture.

use std::process::ExitCode;

// Cargo has no way to require a target OS for a bin target, so the binary is
// built everywhere and refuses to run off macOS rather than failing to link.
#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("eseqlisp_metal_shader_capture needs macOS: there is no Metal backend here");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    use std::path::PathBuf;

    use eseqlisp::metal_shader_capture::{MetalCaptureRenderer, write_capture};

    let mut name = String::from("msl-local");
    let mut output_dir = PathBuf::from(".");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => match args.next() {
                Some(value) => name = value,
                None => {
                    eprintln!("--name needs a value");
                    return ExitCode::FAILURE;
                }
            },
            "--output-dir" => match args.next() {
                Some(value) => output_dir = PathBuf::from(value),
                None => {
                    eprintln!("--output-dir needs a value");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("unknown argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(renderer) = MetalCaptureRenderer::new() else {
        eprintln!("no Metal device available on this machine");
        return ExitCode::FAILURE;
    };
    eprintln!("capturing with {} (Metal)", renderer.device_name());
    match write_capture(&renderer, &output_dir, &name) {
        Ok(()) => {
            eprintln!("wrote {}", output_dir.join(&name).display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("capture failed: {error}");
            ExitCode::FAILURE
        }
    }
}
