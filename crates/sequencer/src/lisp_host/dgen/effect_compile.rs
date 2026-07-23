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
    let lib = load_dylib(&manifest.dylib_path)?;
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

    let dir = Path::new(EFFECTS_DIR);
    let mut names = Vec::new();
    collect(dir, dir, &mut names);
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
    let root = Path::new(EFFECTS_DIR);
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
    Path::new(EFFECTS_DIR)
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
    std::env::temp_dir().join("sequencer_dgenlisp")
}

pub(crate) fn dgenlisp_tool_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join("tools/DGenLisp")
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
    let dylib_name = format!("effect_{}", seq);
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

pub(crate) fn compile_effective_dgen_source_to_dir(
    kind: DGenCompileKind,
    effective_source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
    dir: &Path,
    dylib_name: &str,
) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create output dir: {e}"))?;
    let source_name = match kind {
        DGenCompileKind::Effect => "effect",
        DGenCompileKind::Instrument => "instrument",
    };
    let src_path = dir.join(format!("{dylib_name}.lisp"));
    std::fs::write(&src_path, effective_source)
        .map_err(|e| format!("Failed to write source: {e}"))?;

    let tool_path = dgenlisp_tool_path();
    let mut command = std::process::Command::new(&tool_path);
    command
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()]);
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
