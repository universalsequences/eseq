//! Promote freshly generated physical-model sources before factory installation.
//! Usage: cargo run -p eseqlisp --example pm_factory_sidecars -- /absolute/staging/directory
use std::path::PathBuf;

use eseqlisp::widget_render::patcher::{PatcherIntent, promote_source_to_patch};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(std::env::args_os().nth(1)
        .ok_or("expected a staging directory containing PM model folders")?);
    let library = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/defmacros");
    eseqlisp::defmacro_library::set_default_library_root(library);
    let mut sources: Vec<_> = std::fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path().join("dsp.lisp")))
        .collect::<Result<_, _>>()?;
    sources.retain(|path| path.is_file());
    sources.sort();
    if sources.is_empty() { return Err("staging directory contains no instrument sources".into()); }
    // Existing authored sidecars can take precedence over the generated source.
    // Refuse the entire operation before writing anything if any are present.
    for source in &sources {
        if source.with_extension("layout.json").exists() {
            return Err(format!("use a fresh staging directory; {} already has a sidecar", source.display()).into());
        }
    }
    for path in sources {
        let source = std::fs::read_to_string(&path)?;
        promote_source_to_patch(&path, &source, PatcherIntent::Instrument)?;
        println!("Promoted {}", path.display());
    }
    Ok(())
}
