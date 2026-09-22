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
    /// Mid-render parameter changes, applied at block granularity (the render
    /// loop splits blocks at event frames, so an event lands exactly on its
    /// frame). Uses the same name resolution as `param_overrides`.
    pub param_events: Vec<InstrumentParamEvent>,
    pub tensor_overrides: Vec<(String, Vec<f32>)>,
    pub input_overrides: Vec<(usize, f32)>,
    /// Sustained sine probes: (input channel, frequency Hz, amplitude). The
    /// first tone on a channel replaces the default probe signal; further
    /// tones on the same channel sum. Applied after `input_overrides`.
    pub input_tones: Vec<(usize, f32, f32)>,
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
    /// Full interleaved L/R output, for tests that need waveform-level
    /// metrics (discontinuity energy, windowed RMS) rather than aggregates.
    pub samples: Vec<f32>,
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
    let mut result = dylib_cache::global_cache_manager().acquire(
        DGenCompileKind::Effect,
        origin,
        source,
        sample_rate,
        asset_base,
    )?;
    result.manifest.asset_base = asset_base.map(|base| {
        eseqlisp::widget_render::patcher::register_asset_source_root(base)
    });
    Ok(result)
}

pub fn compile_and_load_uncached_with_asset_base(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CompileResult, String> {
    compile_and_load_uncached_with_host_services(
        source,
        sample_rate,
        asset_base,
        dgen_host_services_v1(),
    )
}

pub(in crate::lisp_host) fn compile_and_load_uncached_with_host_services(
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    host_services: *const DGenHostServicesV1,
) -> Result<CompileResult, String> {
    let json = compile_lisp_with_asset_base(source, sample_rate, asset_base)?;
    let mut manifest = parse_manifest(&json)?;
    manifest.asset_base = asset_base.map(|base| {
        eseqlisp::widget_render::patcher::register_asset_source_root(base)
    });
    // Uncached path: the subprocess skipped its inline audit, so audit here
    // before the dylib is loaded (impl spec, slice E5).
    crate::lisp_host::dgen::dgen_audit::audit_dylib(&manifest.dylib_path)?;
    let lib = load_dylib_prewarmed_with_host_services(&manifest, host_services)?;
    Ok(CompileResult {
        manifest,
        lib,
        lease: None,
    })
}

// ── Effect library storage ──

use crate::app_paths::{ContentRoot, ContentTier};

/// Split an effect name into its package (when it is a `pkg:author.name/…`
/// id) and its logical path. Effects have no `factory:`/`user:` qualifiers:
/// bare names resolve factory tier first, then user, as they always have.
fn parse_effect_id(name: &str) -> io::Result<(Option<String>, &str)> {
    let trimmed = name.trim_end_matches('/');
    match ContentTier::parse_id(trimmed) {
        Ok(Some((ContentTier::Package(prefix), logical))) => Ok((Some(prefix), logical)),
        Ok(Some((_, logical))) => Ok((None, logical)),
        Ok(None) => Ok((None, trimmed)),
        Err(message) => Err(io::Error::new(io::ErrorKind::InvalidInput, message)),
    }
}

fn package_effect_root<'a>(roots: &'a [ContentRoot], prefix: &str) -> Option<&'a ContentRoot> {
    roots
        .iter()
        .find(|root| root.tier == ContentTier::Package(prefix.to_string()))
}

/// Where a `pkg:` effect name lands when its package is not installed: a
/// hidden, never-listed corner of the user tier, so the path exists to
/// report "missing" against and can never alias a real effect.
fn missing_package_effect_path(roots: &[ContentRoot], prefix: &str, relative: &Path) -> PathBuf {
    roots
        .iter()
        .find(|root| root.tier == ContentTier::User)
        .or_else(|| roots.first())
        .map(|root| root.path.clone())
        .unwrap_or_default()
        .join(".missing-package")
        .join(prefix)
        .join(relative)
}

fn read_only_effect_error(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("package effect '{name}' is read-only; fork it before editing"),
    )
}

pub fn save_effect(name: &str, source: &str) -> io::Result<()> {
    if parse_effect_id(name)?.0.is_some() {
        return Err(read_only_effect_error(name));
    }
    let root = crate::app_paths::app_paths().user_effects_dir();
    let path = if name.ends_with('/') {
        root.join(name.trim_end_matches('/')).join("dsp.lisp")
    } else {
        root.join(format!("{name}.lisp"))
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, source)
}

pub fn save_effect_ui(name: &str, source: &str) -> io::Result<()> {
    if parse_effect_id(name)?.0.is_some() {
        return Err(read_only_effect_error(name));
    }
    let path = crate::app_paths::app_paths()
        .user_effects_dir()
        .join(name.trim_end_matches('/'))
        .join("ui.lisp");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, source)
}

pub fn list_saved_effects() -> Vec<String> {
    list_saved_effects_in_roots(&crate::app_paths::app_paths().effect_roots())
}

fn list_saved_effects_in_roots(roots: &[ContentRoot]) -> Vec<String> {
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
                    // A folder effect owns everything beneath it (captures,
                    // research snapshots, helper Lisp); none of it is an effect.
                    continue;
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

    let mut names = Vec::new();
    for root in roots {
        if root.tier.is_package() {
            // Package effects list under their qualified id so the browser and
            // the picker hand back a name that resolves into that package.
            let mut package_names = Vec::new();
            collect(&root.path, &root.path, &mut package_names);
            names.extend(package_names.into_iter().map(|name| root.tier.qualify(&name)));
        } else {
            collect(&root.path, &root.path, &mut names);
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The tier an effect name resolves in: the package it is qualified with,
/// or `None` for the factory/user library.
pub fn effect_package(name: &str) -> Option<String> {
    parse_effect_id(name).ok().and_then(|(package, _)| package)
}

pub fn load_effect_source(name: &str) -> io::Result<String> {
    let path = effect_source_path(name);
    std::fs::read_to_string(&path)
}

pub fn load_effect_ui_source(name: &str) -> io::Result<String> {
    std::fs::read_to_string(effect_ui_path(name))
}

pub fn effect_source_path(name: &str) -> PathBuf {
    effect_source_path_in_roots(&crate::app_paths::app_paths().effect_roots(), name)
}

fn effect_source_path_in_roots(roots: &[ContentRoot], name: &str) -> PathBuf {
    if let Ok((Some(prefix), logical)) = parse_effect_id(name) {
        // Package effects never fall through to another tier.
        let Some(root) = package_effect_root(roots, &prefix) else {
            return missing_package_effect_path(
                roots,
                &prefix,
                Path::new(&format!("{logical}.lisp")),
            );
        };
        let folder_dsp = root.path.join(logical).join("dsp.lisp");
        if folder_dsp.exists() || name.ends_with('/') {
            return folder_dsp;
        }
        return root.path.join(format!("{logical}.lisp"));
    }
    let roots = roots.iter().map(|root| root.path.clone()).collect::<Vec<_>>();
    for root in &roots {
        let path = if name.ends_with('/') {
            root.join(name.trim_end_matches('/')).join("dsp.lisp")
        } else {
            let folder_dsp = root.join(name).join("dsp.lisp");
            if folder_dsp.exists() {
                folder_dsp
            } else {
                root.join(format!("{name}.lisp"))
            }
        };
        if path.exists() {
            return path;
        }
    }
    roots[0].join(format!("{name}.lisp"))
}

pub fn effect_ui_path(name: &str) -> PathBuf {
    effect_ui_path_in_roots(&crate::app_paths::app_paths().effect_roots(), name)
}

fn effect_ui_path_in_roots(roots: &[ContentRoot], name: &str) -> PathBuf {
    if let Ok((Some(prefix), logical)) = parse_effect_id(name) {
        let relative = Path::new(logical).join("ui.lisp");
        return match package_effect_root(roots, &prefix) {
            Some(root) => root.path.join(relative),
            None => missing_package_effect_path(roots, &prefix, &relative),
        };
    }
    let relative = Path::new(name.trim_end_matches('/')).join("ui.lisp");
    roots
        .iter()
        .map(|root| root.path.join(&relative))
        .find(|path| path.exists())
        .unwrap_or_else(|| {
            roots
                .iter()
                .find(|root| root.tier == ContentTier::Factory)
                .or_else(|| roots.first())
                .map(|root| root.path.clone())
                .unwrap_or_default()
                .join(relative)
        })
}

#[cfg(test)]
mod package_effect_tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("eseq-package-effects-{tag}-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_folder_effect(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dsp.lisp"), "(out (in 1) 1)").unwrap();
        std::fs::write(dir.join("ui.lisp"), "(def ui 1)").unwrap();
    }

    fn roots(root: &Path) -> Vec<ContentRoot> {
        vec![
            ContentRoot { tier: ContentTier::Factory, path: root.join("factory") },
            ContentRoot { tier: ContentTier::User, path: root.join("user") },
            ContentRoot {
                tier: ContentTier::Package("alec.fx".into()),
                path: root.join("packages/alec.fx/effects"),
            },
        ]
    }

    #[test]
    fn lisp_files_inside_a_folder_effect_are_not_listed_as_effects() {
        let root = temp_root("folder-contents");
        let roots = roots(&root);
        write_folder_effect(&roots[1].path, "channel");
        let effect = roots[1].path.join("channel");
        std::fs::write(effect.join("capture.lisp"), "(out (in 1) 1)").unwrap();
        let snapshots = effect.join("research/snapshots");
        std::fs::create_dir_all(&snapshots).unwrap();
        std::fs::write(snapshots.join("0a4df8e569fe.lisp"), "(out (in 1) 1)").unwrap();
        std::fs::write(roots[1].path.join("flat.lisp"), "(out (in 1) 1)").unwrap();

        assert_eq!(
            list_saved_effects_in_roots(&roots),
            vec!["channel".to_string(), "flat".into()]
        );
    }

    #[test]
    fn package_effects_list_qualified_and_resolve_only_in_their_package() {
        let root = temp_root("resolve");
        let roots = roots(&root);
        write_folder_effect(&roots[0].path, "comp");
        write_folder_effect(&roots[1].path, "comp");
        write_folder_effect(&roots[2].path, "comp");
        write_folder_effect(&roots[2].path, "chains/verb");
        std::fs::write(roots[2].path.join("flat.lisp"), "(out (in 1) 1)").unwrap();

        let mut names = list_saved_effects_in_roots(&roots);
        names.sort();
        assert_eq!(
            names,
            vec![
                "comp".to_string(),
                "pkg:alec.fx/chains/verb".into(),
                "pkg:alec.fx/comp".into(),
                "pkg:alec.fx/flat".into(),
            ],
            "library names stay bare, package names carry their qualifier"
        );

        // The same bare name resolves factory-first as always; the package
        // id resolves into the package and nowhere else.
        assert_eq!(
            effect_source_path_in_roots(&roots, "comp"),
            roots[0].path.join("comp/dsp.lisp")
        );
        assert_eq!(
            effect_source_path_in_roots(&roots, "pkg:alec.fx/comp"),
            roots[2].path.join("comp/dsp.lisp")
        );
        assert_eq!(
            effect_source_path_in_roots(&roots, "pkg:alec.fx/chains/verb/"),
            roots[2].path.join("chains/verb/dsp.lisp")
        );
        assert_eq!(
            effect_source_path_in_roots(&roots, "pkg:alec.fx/flat"),
            roots[2].path.join("flat.lisp")
        );
        assert_eq!(
            effect_ui_path_in_roots(&roots, "pkg:alec.fx/comp"),
            roots[2].path.join("comp/ui.lisp")
        );

        // A package effect missing from its package never borrows the user
        // or factory copy of the same name.
        let missing = effect_source_path_in_roots(&roots, "pkg:alec.fx/nope");
        assert!(missing.starts_with(&roots[2].path));
        assert!(!missing.exists());
        // An uninstalled package lands in a hidden, never-listed corner.
        let uninstalled = effect_source_path_in_roots(&roots, "pkg:nobody.pack/comp");
        assert!(uninstalled.starts_with(roots[1].path.join(".missing-package")));
        assert!(!uninstalled.exists());
        assert_eq!(effect_package("pkg:alec.fx/comp"), Some("alec.fx".into()));
        assert_eq!(effect_package("comp"), None);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_effects_are_read_only() {
        let error = save_effect("pkg:alec.fx/comp/", "(out 0)").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let error = save_effect_ui("pkg:alec.fx/comp", "(def ui 1)").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        // Malformed package ids are rejected, not written somewhere odd.
        assert_eq!(
            save_effect("pkg:no-slash", "(out 0)").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
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

pub fn dgenlisp_tool_path() -> PathBuf {
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
    let (compiler_source, latency_samples) = super::effect_latency::prepare(
        effective_source, sample_rate, kind == DGenCompileKind::Effect,
    )?;
    let effective_source = finalize_effective_dgen_source(&compiler_source);
    let effective_source = super::dylib_cache::rewrite_library_asset_references(
        &effective_source,
        asset_base,
    )?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create output dir: {e}"))?;
    let source_name = match kind {
        DGenCompileKind::Effect => "effect",
        DGenCompileKind::Instrument => "instrument",
    };
    let src_path = dir.join(format!("{dylib_name}.lisp"));
    std::fs::write(&src_path, &effective_source)
        .map_err(|e| format!("Failed to write source: {e}"))?;

    // The stage is mandatory on every host. In particular, omitting this on
    // Linux makes DGenLisp silently select /usr/bin/clang and destroys the
    // reproducibility guarantee of the generated audio binary.
    let toolchain_root = crate::app_paths::app_paths().dgen_toolchain_root_checked()?;
    // Same hard-error contract for the compiler itself: it is fetched by lock
    // (scripts/fetch_dgenlisp.sh), never tracked, so its absence must name
    // the fetch command instead of surfacing as a spawn failure.
    let tool_path = crate::app_paths::app_paths().dgenlisp_tool_checked()?;
    let mut command = std::process::Command::new(&tool_path);
    command
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()]);
    command.arg("--toolchain-root").arg(&toolchain_root);
    command
        // The host audits the artifact itself (dgen_audit.rs); DGenLisp's
        // inline shell audit would reintroduce the nm/otool (Command Line
        // Tools) dependency this path must not have.
        .arg("--skip-inline-audit");
    if kind == DGenCompileKind::Instrument {
        command.args(["--voices", "12"]);
    }
    let effective_asset_base = super::dylib_cache::effective_asset_base(asset_base);
    command.arg("--asset-base").arg(&effective_asset_base);
    let output = command
        .output()
        .map_err(|e| format!("Failed to run DGenLisp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let error = format!("{}{}", stderr, stdout);
        log_dgenlisp_compile_failure(source_name, &src_path, &error, &effective_source);
        return Err(error);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let manifest = super::effect_latency::annotate_manifest(&stdout, latency_samples)?;
    // Persist the same host manifest returned to both cached and uncached
    // callers. The compiler's original manifest has no host declarations.
    std::fs::write(dir.join(format!("{dylib_name}.json")), &manifest)
        .map_err(|e| format!("Failed to write host manifest: {e}"))?;
    log_dgenlisp_compile_manifest(source_name, &src_path, &manifest);
    Ok(manifest)
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
