/*!
Compile pipeline entry points for DGenLisp effects, plus on-disk source
storage.

`compile_and_load*` run the full pipeline: expand defmacro imports
(`materialize_defmacro_imports`), inject the effect preamble, invoke the
external dgenlisp tool (`compile_lisp*` / `compile_effective_dgen_source_to_dir`),
parse the resulting manifest, and load the dylib — going through `dylib_cache`
unless the uncached variant is used. `CompileResult` bundles the loaded lib +
manifest. Also owns the effects/instruments source directories (`save_effect`,
`list_saved_effects`, `load_effect_source`, ...) and the render-report types
used by offline effect/instrument test rendering in `instrument_compile`.
*/

use super::super::*;

pub struct CompileResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
    pub lease: Option<DylibLease>,
}

#[derive(Clone, Debug)]
pub struct InstrumentRenderOptions {
    pub sample_rate: u32,
    pub block_size: usize,
    pub frames: usize,
    pub midi_note: f32,
    pub velocity: f32,
    pub gate_frames: usize,
    pub voice_index: usize,
    pub param_overrides: Vec<(String, f32)>,
    pub param_events: Vec<InstrumentParamEvent>,
    pub input_overrides: Vec<(usize, f32)>,
}

#[derive(Clone, Debug)]
pub struct InstrumentParamEvent {
    pub frame: usize,
    pub name: String,
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct InstrumentRenderReport {
    pub frames: usize,
    pub peak: f32,
    pub rms: f32,
    pub mean_abs: f32,
    pub nonzero_frames: usize,
    pub first_nonzero_frame: Option<usize>,
    pub non_finite_samples: usize,
    pub first_non_finite_frame: Option<usize>,
    pub non_finite_state_slots: usize,
    pub first_non_finite_state_slot: Option<usize>,
    pub first_samples: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct EffectRenderOptions {
    pub sample_rate: u32,
    pub block_size: usize,
    pub frames: usize,
    pub param_overrides: Vec<(String, f32)>,
    pub input_overrides: Vec<(usize, f32)>,
}

#[derive(Clone, Debug)]
pub struct EffectRenderReport {
    pub frames: usize,
    pub peak: f32,
    pub rms: f32,
    pub left_rms: f32,
    pub right_rms: f32,
    pub mean_abs: f32,
    pub diff_rms: f32,
    pub nonzero_frames: usize,
    pub first_nonzero_frame: Option<usize>,
    pub first_samples: Vec<f32>,
}

pub fn compile_and_load(source: &str, sample_rate: u32) -> Result<CompileResult, String> {
    compile_and_load_with_asset_base(source, sample_rate, None)
}

pub fn compile_and_load_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    compile_and_load_with_origin(source, sample_rate, asset_base, DGenSourceOrigin::Custom)
}

pub fn compile_and_load_with_origin(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    origin: DGenSourceOrigin,
) -> Result<CompileResult, String> {
    dylib_cache::global_cache_manager().acquire(
        DGenCompileKind::Effect,
        origin,
        source,
        sample_rate,
        asset_base,
    )
}

pub fn compile_and_load_uncached_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    let json = compile_lisp_with_asset_base(source, sample_rate, asset_base)?;
    let manifest = parse_manifest(&json)?;
    // Uncached path: the subprocess skipped its inline audit, so audit here
    // before the dylib is loaded (impl spec, slice E5).
    crate::lisp_host::dgen::dgen_audit::audit_dylib(&manifest.dylib_path)?;
    let lib = load_dylib_prewarmed(&manifest)?;
    Ok(CompileResult {
        manifest,
        lib,
        lease: None,
    })
}

// ── Effect library storage ──

pub(in crate::lisp_host) const EFFECTS_DIR: &str = "effects";
pub(in crate::lisp_host) const INSTRUMENTS_DIR: &str = "instruments";

pub fn save_effect(name: &str, source: &str) -> io::Result<()> {
    let path = effect_source_path(name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, source)
}

pub fn save_effect_ui(name: &str, source: &str) -> io::Result<()> {
    let path = effect_ui_path(name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, source)
}

pub fn list_saved_effects() -> Vec<String> {
    fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
                collect(&path, root, out);
            } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                let file_stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                if matches!(file_stem, "dsp" | "ui") {
                    continue;
                }
                out.push(file_stem.to_string());
            }
        }
    }

    let dir = crate::app_paths::app_paths().effects_dir();
    let mut names = Vec::new();
    collect(&dir, &dir, &mut names);
    names.sort();
    names.dedup();
    names
}

pub fn load_effect_source(name: &str) -> io::Result<String> {
    let path = effect_source_path(name);
    std::fs::read_to_string(&path)
}

pub fn load_effect_ui_source(name: &str) -> io::Result<String> {
    std::fs::read_to_string(effect_ui_path(name))
}

pub fn effect_source_path(name: &str) -> PathBuf {
    let root = crate::app_paths::app_paths().effects_dir();
    if name.ends_with('/') {
        return root.join(name.trim_end_matches('/')).join("dsp.lisp");
    }
    let folder_dsp = root.join(name).join("dsp.lisp");
    if folder_dsp.exists() {
        folder_dsp
    } else {
        root.join(format!("{name}.lisp"))
    }
}

pub fn effect_ui_path(name: &str) -> PathBuf {
    crate::app_paths::app_paths()
        .effects_dir()
        .join(name.trim_end_matches('/'))
        .join("ui.lisp")
}

// ── Editor flow ──

pub fn edit_text(initial: &str) -> io::Result<String> {
    let dir = std::env::temp_dir();
    let path = dir.join("sequencer_lisp_edit.lisp");
    std::fs::write(&path, initial)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("editor exited with status: {status}"),
        ));
    }

    std::fs::read_to_string(&path)
}

// ── Compile ──

pub(crate) fn output_dir() -> PathBuf {
    crate::app_paths::app_paths().dgen_scratch_dir()
}

pub(crate) fn dgenlisp_tool_path() -> PathBuf {
    crate::app_paths::app_paths().dgenlisp_tool()
}

pub fn compile_lisp(source: &str, sample_rate: u32) -> Result<String, String> {
    compile_lisp_with_asset_base(source, sample_rate, None)
}

pub fn compile_lisp_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<String, String> {
    let dir = output_dir();
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Include the pid: concurrent test processes share output_dir() and each
    // starts COMPILE_COUNTER at 0, so a bare counter collides across them.
    let dylib_name = format!("effect_{}_{}", std::process::id(), seq);
    let effective_source = effective_dgen_source(DGenCompileKind::Effect, source, sample_rate)?;
    compile_effective_dgen_source_to_dir(
        DGenCompileKind::Effect,
        &effective_source,
        sample_rate,
        asset_base,
        &dir,
        &dylib_name,
    )
}

pub(crate) fn materialize_defmacro_imports(source: &str) -> Result<String, String> {
    eseqlisp::defmacro_library::materialize_with_default_library(source)
        .map_err(|error| error.to_string())
}

pub(crate) fn effective_dgen_source(
    kind: DGenCompileKind,
    source: &str,
    sample_rate: u32,
) -> Result<String, String> {
    let source = materialize_defmacro_imports(source)?;
    let preamble = match kind {
        DGenCompileKind::Effect => effect_preamble(sample_rate),
        DGenCompileKind::Instrument => instrument_preamble(sample_rate),
    };
    Ok(format!("{preamble}\n\n{source}"))
}

/// DGenLisp's `mod` validator only sees top-level `(param ...)` declarations,
/// so a param declared inside a `defmacro` body breaks any `(mod name)` that
/// references it — even from the same body. Params are host-global either
/// way, so standalone param forms are hoisted out of macro bodies (deduped by
/// name) before invoking the compiler. This runs on the temp compile copy
/// only; the persisted patch source keeps params inside their macros, so the
/// patch editor's macro views are unchanged. The hoist is verbatim byte-range
/// surgery on source spans — untouched text (comments, number formatting like
/// `@modulator 1`) is preserved exactly.
pub(crate) fn hoist_defmacro_params(source: &str) -> String {
    use eseqlisp::parser::{Expr, ExprKind, Parser, SourceSpan, SpannedASTParser};

    fn head_is(items: &[Expr], name: &str) -> bool {
        matches!(items.first(), Some(Expr { kind: ExprKind::Symbol(head), .. }) if head == name)
    }

    fn standalone_param_name(expr: &Expr) -> Option<&str> {
        let ExprKind::List(items) = &expr.kind else {
            return None;
        };
        if !head_is(items, "param") {
            return None;
        }
        match items.get(1) {
            Some(Expr {
                kind: ExprKind::Symbol(name),
                ..
            }) => Some(name),
            _ => None,
        }
    }

    let Ok(tokens) = Parser::new(source.to_string()).parse_spanned() else {
        return source.to_string();
    };
    let Ok(exprs) = SpannedASTParser::new(tokens).parse() else {
        return source.to_string();
    };

    let mut declared: std::collections::HashSet<String> = exprs
        .iter()
        .filter_map(|expr| standalone_param_name(expr).map(str::to_string))
        .collect();
    // Insertions keyed by the owning defmacro's start byte: hoisted params
    // land immediately above their macro, after everything that precedes it
    // (the preamble's @modulator inputs must stay ahead of @mod params).
    let mut insertions: Vec<(usize, String)> = Vec::new();
    let mut removals: Vec<SourceSpan> = Vec::new();
    for expr in &exprs {
        let ExprKind::List(items) = &expr.kind else {
            continue;
        };
        if !head_is(items, "defmacro") || items.len() < 3 {
            continue;
        }
        let macro_start = expr.origin.primary_span.start_byte;
        let mut hoisted = String::new();
        for form in &items[3..] {
            let Some(name) = standalone_param_name(form) else {
                continue;
            };
            let span = form.origin.primary_span.clone();
            if span.end_byte > source.len() || span.start_byte >= span.end_byte {
                continue;
            }
            if declared.insert(name.to_string()) {
                hoisted.push_str(&source[span.start_byte..span.end_byte]);
                hoisted.push('\n');
            }
            removals.push(span);
        }
        if !hoisted.is_empty() {
            insertions.push((macro_start, hoisted));
        }
    }
    if removals.is_empty() {
        return source.to_string();
    }

    enum Splice {
        Insert(String),
        Remove(usize),
    }
    let mut events: Vec<(usize, Splice)> = insertions
        .into_iter()
        .map(|(pos, text)| (pos, Splice::Insert(text)))
        .chain(
            removals
                .into_iter()
                .map(|span| (span.start_byte, Splice::Remove(span.end_byte))),
        )
        .collect();
    events.sort_by_key(|(pos, _)| *pos);
    let mut out = String::new();
    let mut cursor = 0;
    for (pos, splice) in events {
        out.push_str(&source[cursor..pos]);
        match splice {
            Splice::Insert(text) => {
                out.push_str(&text);
                cursor = pos;
            }
            Splice::Remove(end) => {
                cursor = end;
            }
        }
    }
    out.push_str(&source[cursor..]);
    out
}

/// Final source transform immediately before invoking DGenLisp. External
/// tool flows such as Patch Learn must use this too; handing them the editor
/// source directly omits both the instrument preamble and this hoisting pass.
pub(crate) fn finalize_effective_dgen_source(effective_source: &str) -> String {
    hoist_defmacro_params(effective_source)
}

pub(crate) fn compile_effective_dgen_source_to_dir(
    kind: DGenCompileKind,
    effective_source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    dir: &Path,
    dylib_name: &str,
) -> Result<String, String> {
    let effective_source = &finalize_effective_dgen_source(effective_source);
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create output dir: {e}"))?;
    let source_name = match kind {
        DGenCompileKind::Effect => "effect",
        DGenCompileKind::Instrument => "instrument",
    };
    let src_path = dir.join(format!("{dylib_name}.lisp"));
    std::fs::write(&src_path, effective_source)
        .map_err(|e| format!("Failed to write source: {e}"))?;

    // Hermetic toolchain hand-off (impl spec, decision 1 / slice E2): the
    // staged root is passed unconditionally and preflight-checked here; a
    // missing/incomplete stage is a hard compile error, never a fallback to
    // the system compiler.
    let toolchain_root = crate::app_paths::app_paths().dgen_toolchain_root_checked()?;
    let tool_path = dgenlisp_tool_path();
    let mut command = std::process::Command::new(&tool_path);
    command
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()])
        .arg("--toolchain-root")
        .arg(&toolchain_root)
        // The host audits the artifact itself (dgen_audit.rs); DGenLisp's
        // inline shell audit would reintroduce the nm/otool (Command Line
        // Tools) dependency this path must not have.
        .arg("--skip-inline-audit");
    if kind == DGenCompileKind::Instrument {
        command.args(["--voices", "12"]);
    }
    if let Some(asset_base) = asset_base {
        command.args(["--asset-base", asset_base.to_str().unwrap_or(".")]);
    }
    let output = command
        .output()
        .map_err(|e| format!("Failed to run DGenLisp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let error = format!("{}{}", stderr, stdout);
        log_dgenlisp_compile_failure(source_name, &src_path, &error, effective_source);
        return Err(error);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    log_dgenlisp_compile_manifest(source_name, &src_path, &stdout);
    Ok(stdout)
}

// ── Parse manifest ──

pub(in crate::lisp_host) fn parse_dgen_param_span(param: &serde_json::Value) -> usize {
    const DEFAULT_DGEN_PARAM_SPAN: usize = 1;
    const MAX_DGEN_PARAM_SPAN: usize = 64;

    [
        "cellSpan",
        "vectorWidth",
        "cellWidth",
        "laneWidth",
        "laneCount",
        "span",
        "width",
    ]
    .iter()
    .find_map(|key| param.get(*key).and_then(|value| value.as_u64()))
    .map(|span| span as usize)
    .filter(|span| *span > 0)
    .unwrap_or(DEFAULT_DGEN_PARAM_SPAN)
    .min(MAX_DGEN_PARAM_SPAN)
}

#[cfg(test)]
mod hoist_tests {
    use super::hoist_defmacro_params;

    #[test]
    fn hoists_macro_body_params_to_top_level() {
        let source = "(defmacro reverb123 (input)\n  (param xyz @min 0.3 @max 8 @mod true @mod-mode additive)\n  (def m (mod xyz))\n  (* input m))\n(def sig (in 1))\n(out (reverb123 sig) 1)";
        let hoisted = hoist_defmacro_params(source);
        let first_form = hoisted.lines().next().unwrap();
        assert!(
            first_form.starts_with("(param xyz"),
            "param should hoist above the macro:\n{hoisted}"
        );
        assert!(
            first_form.contains("@mod-mode additive"),
            "param attrs must survive the hoist:\n{hoisted}"
        );
        let macro_form_start = hoisted.find("(defmacro").unwrap();
        assert!(
            !hoisted[macro_form_start..].contains("(param xyz"),
            "macro body must no longer declare the param:\n{hoisted}"
        );
        assert!(
            hoisted.contains("(mod xyz)"),
            "the mod reference stays in the body:\n{hoisted}"
        );
    }

    #[test]
    fn duplicate_names_hoist_once() {
        let source = "(param xyz @min 0 @max 1)\n(defmacro a (x) (param xyz @min 0 @max 1) (* x (mod xyz)))\n(def sig (in 1))\n(out (a sig) 1)";
        let hoisted = hoist_defmacro_params(source);
        assert_eq!(
            hoisted.matches("(param xyz").count(),
            1,
            "already-declared params drop from the body without re-hoisting:\n{hoisted}"
        );
    }

    #[test]
    fn sources_without_macro_params_pass_through_verbatim() {
        let source =
            "; comment survives\n(param xyz @min 0 @max 1)\n(def sig (in 1))\n(out sig 1)\n";
        assert_eq!(hoist_defmacro_params(source), source);
    }
}
