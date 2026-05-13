pub mod sample;

use std::path::{Path, PathBuf};

use crate::runtime::Runtime;
use crate::vm::Value;

pub fn register_audio_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "sample-load-wav",
        "(sample-load-wav path)",
        "Load a WAV file and return a waveform buffer map with metadata and peak levels.",
        |args, ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("sample-load-wav expects a string path".to_string());
            };
            let resolved = resolve_sample_path(path, ctx.current_buffer_path().as_deref());
            let sample = sample::register_sample(sample::SampleBuffer::load_wav(&resolved)?);
            Ok(sample.to_value())
        },
    );
}

fn resolve_sample_path(path: &str, current_buffer_path: Option<&Path>) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return candidate;
    }
    if let Some(base) = current_buffer_path.and_then(Path::parent) {
        return base.join(candidate);
    }
    candidate
}
