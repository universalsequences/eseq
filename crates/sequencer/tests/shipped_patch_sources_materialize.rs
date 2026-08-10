//! Guard: every shipped patch source must still resolve its `(use-defmacro ...)`
//! imports against the checked-in defmacro library. Pruning a package that a
//! shipped effect or instrument imports fails here instead of at patch load.

use std::fs;
use std::path::{Path, PathBuf};

fn collect_dsp_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dsp_sources(&path, out);
        } else if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
            out.push(path);
        }
    }
}

#[test]
fn shipped_patch_sources_materialize_defmacro_imports() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    collect_dsp_sources(&root.join("effects"), &mut sources);
    collect_dsp_sources(&root.join("instruments"), &mut sources);
    sources.sort();
    assert!(
        !sources.is_empty(),
        "expected to find shipped dsp.lisp sources under {}",
        root.display()
    );

    let mut failures = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        if let Err(error) = eseqlisp::defmacro_library::materialize_with_default_library(&source) {
            failures.push(format!("{}: {error:?}", path.display()));
        }
    }

    assert!(
        failures.is_empty(),
        "shipped patch sources failed to materialize defmacro imports:\n{}",
        failures.join("\n")
    );
}
