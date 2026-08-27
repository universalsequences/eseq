//! Write the deterministic WGSL shader captures for one host.
//!
//! ```text
//! cargo run -p eseqlisp --features wgpu,capture-harness --bin eseqlisp_shader_capture -- \
//!     --name wgsl-linux-x86_64 \
//!     --output-dir crates/eseqlisp/tests/fixtures/shader-capture/goldens
//! ```
//!
//! Exits non-zero when no wgpu adapter is available, so a headless run fails
//! loudly instead of writing an empty capture.

use std::path::PathBuf;
use std::process::ExitCode;

use eseqlisp::shader_capture::{CaptureRenderer, write_capture};

fn main() -> ExitCode {
    let mut name = String::from("wgsl-local");
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

    let Some(renderer) = CaptureRenderer::new() else {
        eprintln!("no wgpu adapter available on this machine");
        return ExitCode::FAILURE;
    };
    eprintln!(
        "capturing with {} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );
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
