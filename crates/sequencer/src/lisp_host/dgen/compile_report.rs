/*!
The compiler's per-binding shapes report for failed compiles
(docs/patcher-tensor-cables-spec.md §3-§4).

DGenLisp writes `<outdir>/<dylib-name>.shapes.json` = `{bindingShapes,
failedForm?}` whether or not evaluation succeeds. On success the manifest
carries the same `bindingShapes` (and survives the dylib cache, which persists
the manifest), so the manifest is authoritative. A failed compile writes no
manifest, so `compile_effective_dgen_source_to_dir` records the report here,
keyed by the sha256 of the effective source it compiled; a caller holding only
the `Err(String)` fetches it with [`dgen_compile_failure_report`] without every
compile caller's result type changing.
*/

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use eseqlisp::widget_render::patcher::CompileShapesReport;

use super::dylib_cache::DGenCompileKind;

/// Recent failures kept. Previews compile one revision at a time per session;
/// a handful covers several open editors racing.
const MAX_REPORTS: usize = 16;

static FAILED_COMPILE_REPORTS: Mutex<VecDeque<(String, CompileShapesReport)>> =
    Mutex::new(VecDeque::new());

pub(crate) fn shapes_report_path(dir: &Path, dylib_name: &str) -> std::path::PathBuf {
    dir.join(format!("{dylib_name}.shapes.json"))
}

/// Read a compile's shapes report. `None` when the compiler predates the file
/// or the run stopped before writing it (a crash, a missing tool).
pub(crate) fn read_shapes_report(dir: &Path, dylib_name: &str) -> Option<CompileShapesReport> {
    let text = std::fs::read_to_string(shapes_report_path(dir, dylib_name)).ok()?;
    match serde_json::from_str(&text) {
        Ok(report) => Some(report),
        Err(error) => {
            eprintln!("[dgenlisp] ignoring unreadable shapes report: {error}");
            None
        }
    }
}

fn source_key(effective_source: &str) -> String {
    super::dylib_cache::sha256_hex(effective_source.as_bytes())
}

/// Record the report of a failed compile of `effective_source` (the source
/// handed to `compile_effective_dgen_source_to_dir`), replacing any older
/// report for the same source.
pub(crate) fn record_failed_compile_report(effective_source: &str, report: CompileShapesReport) {
    let key = source_key(effective_source);
    let Ok(mut reports) = FAILED_COMPILE_REPORTS.lock() else {
        return;
    };
    reports.retain(|(existing, _)| *existing != key);
    reports.push_back((key, report));
    while reports.len() > MAX_REPORTS {
        reports.pop_front();
    }
}

/// Drop the failure report for `effective_source` (it compiled since).
pub(crate) fn forget_failed_compile_report(effective_source: &str) {
    let key = source_key(effective_source);
    if let Ok(mut reports) = FAILED_COMPILE_REPORTS.lock() {
        reports.retain(|(existing, _)| *existing != key);
    }
}

fn failed_compile_report_for_effective_source(effective_source: &str) -> Option<CompileShapesReport> {
    let key = source_key(effective_source);
    let reports = FAILED_COMPILE_REPORTS.lock().ok()?;
    reports
        .iter()
        .rev()
        .find(|(existing, _)| *existing == key)
        .map(|(_, report)| report.clone())
}

/// The shapes report of the latest failed compile of `source` — the same
/// arguments just passed to `compile_and_load_with_origin` /
/// `compile_and_load_instrument_with_origin` (or any compile that goes through
/// the dylib cache), which returned `Err`. `None` when the failure produced no
/// report (old compiler, crash, the compiler never ran) or `source` has not
/// failed to compile in this process.
pub fn dgen_compile_failure_report(
    kind: DGenCompileKind,
    source: &str,
    sample_rate: u32,
) -> Option<CompileShapesReport> {
    let effective = super::effect_compile::effective_dgen_source(kind, source, sample_rate).ok()?;
    failed_compile_report_for_effective_source(&effective)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_report_is_read_from_the_output_dir_and_found_by_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            shapes_report_path(dir.path(), "dgen_x"),
            r#"{"bindingShapes":{"a":{"kind":"tensor","shape":[8]}},
                "failedForm":{"index":1,"message":"Type error","name":"b","op":"def"}}"#,
        )
        .unwrap();
        let report = read_shapes_report(dir.path(), "dgen_x").expect("report parses");
        assert_eq!(report.binding_shapes["a"].shape, vec![8]);
        assert_eq!(report.failed_form.as_ref().unwrap().name.as_deref(), Some("b"));
        assert!(read_shapes_report(dir.path(), "missing").is_none());

        let source = "(def a (iota 8))\n(def b (+ a (iota 3)))\n(out (sum b) 1)\n";
        let effective =
            super::super::effect_compile::effective_dgen_source(DGenCompileKind::Effect, source, 48_000)
                .unwrap();
        record_failed_compile_report(&effective, report.clone());
        assert_eq!(
            dgen_compile_failure_report(DGenCompileKind::Effect, source, 48_000),
            Some(report)
        );
        assert_eq!(
            dgen_compile_failure_report(DGenCompileKind::Effect, "(out 0 1)", 48_000),
            None,
            "another source has no report"
        );
        forget_failed_compile_report(&effective);
        assert_eq!(dgen_compile_failure_report(DGenCompileKind::Effect, source, 48_000), None);
    }

    const MANIFEST_WITHOUT_SHAPES: &str = r#"{
        "processAbi": "dgen-host-abi-v1",
        "dylib": "test.dylib",
        "totalMemorySlots": 16
    }"#;

    #[test]
    fn manifest_binding_shapes_parse_and_default_to_empty_for_older_compilers() {
        let old = crate::lisp_host::parse_manifest(MANIFEST_WITHOUT_SHAPES).unwrap();
        assert!(old.binding_shapes.is_empty());

        let manifest = crate::lisp_host::parse_manifest(
            r#"{
                "processAbi": "dgen-host-abi-v1",
                "dylib": "test.dylib",
                "totalMemorySlots": 16,
                "bindingShapes": {
                    "c": {"kind": "signal-tensor", "shape": [8]},
                    "s": {"kind": "signal", "shape": []},
                    "pair": {"kind": "tuple", "shape": [],
                             "elements": [{"kind": "signal", "shape": []},
                                          {"kind": "tensor", "shape": [4, 2]}]},
                    "junk": {"shape": "not a shape"}
                }
            }"#,
        )
        .unwrap();
        use eseqlisp::widget_render::patcher::BindingShapeKind;
        assert_eq!(manifest.binding_shapes["c"].kind, BindingShapeKind::SignalTensor);
        assert_eq!(manifest.binding_shapes["c"].shape, vec![8]);
        assert!(!manifest.binding_shapes["s"].is_multichannel());
        assert_eq!(manifest.binding_shapes["pair"].outlet(1).unwrap().shape, vec![4, 2]);
        assert!(
            !manifest.binding_shapes.contains_key("junk"),
            "an unreadable entry is unknown, not a manifest failure"
        );
    }

    /// Real compiles with the shape-reporting compiler. Opt-in: runs only when
    /// `ESEQ_DGENLISP_TOOL` names a compiler (dgen-audio PR #27 or later).
    #[test]
    fn real_compiler_reports_shapes_on_success_cache_hit_and_failure() {
        let Some(tool) = std::env::var_os("ESEQ_DGENLISP_TOOL") else {
            eprintln!("skipping: ESEQ_DGENLISP_TOOL not set");
            return;
        };
        if !Path::new(&tool).exists() {
            eprintln!("skipping: ESEQ_DGENLISP_TOOL does not exist");
            return;
        }
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        // Unique source per run so the dylib cache misses on the first load.
        let good = format!(
            "; {nonce}\n(def lanes (iota 8))\n(def ph (phasor (* lanes 110)))\n\
             (def s (sum (cos (* ph 6.283))))\n(out s 1)\n(out s 2)\n"
        );
        let origin = crate::lisp_host::DGenSourceOrigin::Draft;
        for attempt in ["compiled", "cached"] {
            let result =
                crate::lisp_host::compile_and_load_with_origin(&good, 48_000, None, origin)
                    .unwrap_or_else(|error| panic!("{attempt}: {error}"));
            let shapes = &result.manifest.binding_shapes;
            assert_eq!(shapes.get("ph").map(|s| s.shape.clone()), Some(vec![8]), "{attempt}");
            assert!(shapes.get("ph").unwrap().is_multichannel(), "{attempt}");
            assert!(!shapes.get("s").unwrap().is_multichannel(), "{attempt}");
        }

        let bad = format!(
            "; {nonce}\n(def a (iota 8))\n(def a2 (reshape a [8 1]))\n(out (sum a2) 1)\n(out 0 2)\n"
        );
        let error = crate::lisp_host::compile_and_load_with_origin(&bad, 48_000, None, origin)
            .err()
            .expect("reshape without @shape fails");
        assert!(error.contains("a2"), "{error}");
        let report = dgen_compile_failure_report(DGenCompileKind::Effect, &bad, 48_000)
            .expect("failure report recorded");
        assert_eq!(report.binding_shapes["a"].shape, vec![8]);
        let failed = report.failed_form.expect("failed form");
        assert_eq!((failed.op.as_str(), failed.name.as_deref()), ("def", Some("a2")));
    }
}
