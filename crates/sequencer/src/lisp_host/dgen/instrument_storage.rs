/*!
On-disk instrument storage (sources, metadata, presets) and the global
registry of loaded instrument process functions.

The storage half resolves instrument names to source/metadata/preset paths
(supporting both flat files and `name/dsp.lisp` folder layouts) and
loads/saves `InstrumentPreset` banks and `CustomInstrumentRunMode` metadata.
The registry half is a set of lock-free static tables indexed by
engine/voice slot (`DGEN_INSTRUMENT_FNS`, output counts, enabled-voice
counts, process-call stats) that the audio thread reads through
`dgenlisp_instrument_vtable()` while the UI/compile side swaps entries in
(`set_dgen_instrument_fn`, ...).
*/

use super::super::*;
use crate::sequencer::MAX_INSTRUMENT_ENGINES;
use crate::audio::MAX_VOICES;

#[derive(Clone, Serialize, Deserialize)]
pub struct InstrumentPreset {
    pub id: String,
    pub name: String,
    pub base_note_offset: f32,
    pub params: std::collections::BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub key_locks: std::collections::BTreeMap<u8, std::collections::BTreeMap<String, f32>>,
}

#[derive(Serialize, Deserialize)]
pub(in crate::lisp_host) struct InstrumentMetadataFile {
    version: u32,
    run_mode: String,
}

#[derive(Serialize, Deserialize)]
pub(in crate::lisp_host) struct InstrumentPresetBank {
    version: u32,
    engine_name: String,
    source_file: String,
    presets: Vec<InstrumentPreset>,
}

/// Memo for the recursive-walk fallback in `resolve_instrument_storage_path`.
/// Names that don't resolve via the cheap exact-path probes trigger a full
/// scan of the AppPaths instrument roots; hot callers (the glyph feeds re-read sources
/// every reactive tick) must not pay that walk repeatedly. Hits are
/// revalidated with `exists()`, so deleting/moving a source re-resolves on
/// the next call. Known staleness: adding a SECOND source with the same leaf
/// name mid-session won't surface the ambiguity error until the cached path
/// goes away.
fn resolved_walk_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<(String, String), PathBuf>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), PathBuf>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstrumentTier {
    Factory,
    User,
}

impl InstrumentTier {
    fn prefix(self) -> &'static str {
        match self {
            Self::Factory => "factory",
            Self::User => "user",
        }
    }

    fn root(self, paths: &crate::app_paths::AppPaths) -> PathBuf {
        match self {
            Self::Factory => paths.instruments_dir(),
            Self::User => paths.user_instruments_dir(),
        }
    }
}

fn parse_instrument_id(name: &str) -> io::Result<Option<(InstrumentTier, &str)>> {
    let trimmed = name.trim_end_matches('/');
    let qualified = if let Some(path) = trimmed.strip_prefix("factory:") {
        Some((InstrumentTier::Factory, path))
    } else if let Some(path) = trimmed.strip_prefix("user:") {
        Some((InstrumentTier::User, path))
    } else if trimmed.contains(':') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported instrument id '{name}'"),
        ));
    } else {
        None
    };
    let path = qualified.map(|(_, path)| path).unwrap_or(trimmed);
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid instrument id '{name}'"),
        ));
    }
    Ok(qualified)
}

fn resolve_instrument_storage_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
    extension: &str,
) -> io::Result<PathBuf> {
    fn is_hidden(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }

    fn collect_file_matches(dir: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                collect_file_matches(&path, file_name, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
                out.push(path);
            }
        }
    }

    fn resolve_in_root(
        root: &Path,
        logical_name: &str,
        extension: &str,
        allow_leaf_fallback: bool,
    ) -> io::Result<Option<PathBuf>> {
        let trimmed = logical_name.trim_end_matches('/');
        let exact = root.join(format!("{trimmed}.{extension}"));
        if exact.exists() {
            return Ok(Some(exact));
        }
        if extension == "lisp" {
            let dsp = root.join(trimmed).join("dsp.lisp");
            if dsp.exists() {
                return Ok(Some(dsp));
            }
        }

        if !allow_leaf_fallback {
            return Ok(None);
        }

        let basename = Path::new(trimmed)
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or(trimmed);
        let mut matches = Vec::new();
        if extension == "lisp" {
            collect_folder_source_matches(root, basename, &mut matches);
        }
        collect_file_matches(root, &format!("{basename}.{extension}"), &mut matches);
        matches.sort_by_key(|path| path.to_string_lossy().to_lowercase());
        matches.dedup();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            _ => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Ambiguous instrument '{logical_name}': found multiple matching instrument sources under {}",
                    root.display()
                ),
            )),
        }
    }

    let qualified = parse_instrument_id(name)?;
    let logical_name = qualified
        .map(|(_, logical_name)| logical_name)
        .unwrap_or_else(|| name.trim_end_matches('/'));
    let tiers: &[InstrumentTier] = match qualified {
        Some((InstrumentTier::Factory, _)) => &[InstrumentTier::Factory],
        Some((InstrumentTier::User, _)) => &[InstrumentTier::User],
        None => &[InstrumentTier::Factory, InstrumentTier::User],
    };

    let cache_key = (name.to_string(), extension.to_string());
    if let Some(cached) = resolved_walk_cache().lock().unwrap().get(&cache_key).cloned() {
        if cached.exists() {
            return Ok(cached);
        }
        resolved_walk_cache().lock().unwrap().remove(&cache_key);
    }

    for tier in tiers {
        if let Some(resolved) = resolve_in_root(
            &tier.root(paths),
            logical_name,
            extension,
            qualified.is_none(),
        )? {
            resolved_walk_cache()
                .lock()
                .unwrap()
                .insert(cache_key, resolved.clone());
            return Ok(resolved);
        }
    }

    Ok(tiers[0]
        .root(paths)
        .join(format!("{logical_name}.{extension}")))
}

pub(in crate::lisp_host) fn resolve_instrument_storage_path(name: &str, extension: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path_with_paths(crate::app_paths::app_paths(), name, extension)
}

/// Return the stable project id for an instrument. Legacy bare names prefer
/// factory content when both tiers contain the same name; only a missing
/// factory source falls through to the user tier.
pub fn qualify_instrument_id(name: &str) -> io::Result<String> {
    qualify_instrument_id_with_paths(crate::app_paths::app_paths(), name)
}

fn qualify_instrument_id_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<String> {
    let source = resolve_instrument_storage_path_with_paths(paths, name, "lisp")?;
    if !source.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("instrument '{name}' does not exist"),
        ));
    }
    for tier in [InstrumentTier::Factory, InstrumentTier::User] {
        let root = tier.root(paths);
        if let Ok(relative) = source.strip_prefix(&root) {
            let logical = if relative.file_name().and_then(|part| part.to_str()) == Some("dsp.lisp") {
                relative.parent().unwrap_or(relative).to_path_buf()
            } else {
                relative.with_extension("")
            };
            let logical = logical.to_string_lossy().replace('\\', "/");
            return Ok(format!("{}:{logical}", tier.prefix()));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("instrument source '{}' is outside the configured tiers", source.display()),
    ))
}

pub(in crate::lisp_host) fn collect_folder_source_matches(dir: &Path, folder_name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.starts_with('.') || !path.is_dir() {
            continue;
        }
        if name == folder_name {
            let dsp = path.join("dsp.lisp");
            if dsp.exists() {
                out.push(dsp);
            }
        }
        collect_folder_source_matches(&path, folder_name, out);
    }
}

pub(in crate::lisp_host) fn resolve_instrument_folder_path(name: &str) -> io::Result<PathBuf> {
    let source = resolve_instrument_storage_path(name, "lisp")?;
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Resolved folder-style instrument '{name}' has no parent directory"),
            )
        })
    } else {
        Ok(source.with_extension(""))
    }
}

pub fn instrument_source_path(name: &str) -> io::Result<PathBuf> {
    resolve_instrument_storage_path(name, "lisp")
}

pub(in crate::lisp_host) fn instrument_metadata_path_for_source_path(source: &Path) -> io::Result<PathBuf> {
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        source
            .parent()
            .map(|parent| parent.join("instrument.json"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Resolved folder-style instrument source '{}' has no parent directory",
                        source.display()
                    ),
                )
            })
    } else {
        Ok(source.with_extension("instrument.json"))
    }
}

pub fn instrument_metadata_path(name: &str) -> io::Result<PathBuf> {
    let source = instrument_source_path(name)?;
    instrument_metadata_path_for_source_path(&source)
}

pub fn load_instrument_run_mode(name: &str) -> io::Result<CustomInstrumentRunMode> {
    let path = instrument_metadata_path(name)?;
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CustomInstrumentRunMode::Instrument);
        }
        Err(error) => return Err(error),
    };
    let metadata: InstrumentMetadataFile = serde_json::from_str(&source).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "failed to parse instrument metadata '{}': {error}",
                path.display()
            ),
        )
    })?;
    CustomInstrumentRunMode::parse(&metadata.run_mode).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid instrument run_mode '{}'", metadata.run_mode),
        )
    })
}

pub fn save_instrument_run_mode(name: &str, run_mode: CustomInstrumentRunMode) -> io::Result<()> {
    let source = writable_instrument_source_path(name)?;
    let path = instrument_metadata_path_for_source_path(&source)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let metadata = InstrumentMetadataFile {
        version: 1,
        run_mode: run_mode.as_str().to_string(),
    };
    let json = serde_json::to_string_pretty(&metadata).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to encode instrument metadata: {error}"),
        )
    })?;
    std::fs::write(path, format!("{json}\n"))
}

/// Strip a content root from `parent`: any AppPaths root in `roots`, or the
/// bare relative dir name (`instruments/…`, `effects/…`) that production path
/// strings still carry (patcher buffers, UI state, saved descriptors).
fn strip_source_root(parent: &Path, roots: &[PathBuf], relative_dir: &str) -> Option<PathBuf> {
    for root in roots {
        if let Ok(rel) = parent.strip_prefix(root) {
            return Some(rel.to_path_buf());
        }
    }
    parent
        .strip_prefix(relative_dir)
        .ok()
        .map(Path::to_path_buf)
}

pub(in crate::lisp_host) fn instrument_name_from_source_path(path: &Path) -> Option<String> {
    if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        if let Some(parent) = path.parent() {
            if let Some(rel) = strip_source_root(
                parent,
                &crate::app_paths::app_paths().instrument_dirs(),
                "instruments",
            ) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !rel.is_empty() {
                    return Some(format!("{rel}/"));
                }
            }
        }
    }

    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
}

pub(in crate::lisp_host) fn source_name_from_path(kind: &CompileKind, path: &Path) -> Option<String> {
    match kind {
        CompileKind::Instrument => instrument_name_from_source_path(path),
        CompileKind::Effect => {
            if path.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
                path.parent()
                    .and_then(|parent| {
                        strip_source_root(
                            parent,
                            &crate::app_paths::app_paths().effect_dirs(),
                            "effects",
                        )
                    })
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            } else {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
            }
        }
    }
}

/// Preset banks are user data, not instrument source: saving presets on a
/// factory instrument must not require forking it. Factory-qualified ids keep
/// their bank in the user tier under the same logical path — exactly where
/// legacy bare names wrote it — so a factory id without a factory-shipped
/// bank falls through to that user-tier location on load.
fn user_tier_preset_path(paths: &crate::app_paths::AppPaths, logical_name: &str) -> PathBuf {
    paths
        .user_instruments_dir()
        .join(format!("{logical_name}.presets"))
}

pub(in crate::lisp_host) fn instrument_preset_path(name: &str) -> io::Result<PathBuf> {
    instrument_preset_path_with_paths(crate::app_paths::app_paths(), name)
}

fn instrument_preset_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    let resolved = resolve_instrument_storage_path_with_paths(paths, name, "presets")?;
    if resolved.is_file() {
        return Ok(resolved);
    }
    if let Some((InstrumentTier::Factory, logical_name)) = parse_instrument_id(name)? {
        let user = user_tier_preset_path(paths, logical_name);
        if user.is_file() {
            return Ok(user);
        }
    }
    Ok(resolved)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PresetBankFingerprint {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Clone)]
struct CachedPresetBank {
    fingerprint: Option<PresetBankFingerprint>,
    presets: std::sync::Arc<Vec<InstrumentPreset>>,
}

fn preset_bank_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, CachedPresetBank>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, CachedPresetBank>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn preset_bank_fingerprint(path: &Path) -> io::Result<Option<PresetBankFingerprint>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(PresetBankFingerprint {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn cached_instrument_presets(name: &str) -> io::Result<std::sync::Arc<Vec<InstrumentPreset>>> {
    let path = instrument_preset_path(name)?;
    let fingerprint = preset_bank_fingerprint(&path)?;
    let mut cache = preset_bank_cache().lock().unwrap();
    if let Some(cached) = cache.get(&path) {
        if cached.fingerprint == fingerprint {
            return Ok(cached.presets.clone());
        }
    }

    let presets = match std::fs::read_to_string(&path) {
        Ok(src) => {
            let bank: InstrumentPresetBank = serde_json::from_str(&src).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse preset bank '{}': {e}", path.display()),
                )
            })?;
            bank.presets
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let presets = std::sync::Arc::new(presets);
    cache.insert(
        path,
        CachedPresetBank {
            fingerprint,
            presets: presets.clone(),
        },
    );
    Ok(presets)
}

pub fn load_instrument_presets(name: &str) -> io::Result<Vec<InstrumentPreset>> {
    Ok(cached_instrument_presets(name)?.as_ref().clone())
}

pub fn load_instrument_preset_names(name: &str) -> io::Result<Vec<String>> {
    Ok(cached_instrument_presets(name)?
        .iter()
        .map(|preset| preset.name.clone())
        .collect())
}

fn instrument_preset_save_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    if let Some((InstrumentTier::Factory, logical_name)) = parse_instrument_id(name)? {
        return Ok(user_tier_preset_path(paths, logical_name));
    }
    let source = writable_instrument_source_path_with_paths(paths, name)?;
    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        Ok(source.parent().unwrap_or(&source).with_extension("presets"))
    } else {
        Ok(source.with_extension("presets"))
    }
}

pub fn save_instrument_presets(name: &str, presets: &[InstrumentPreset]) -> io::Result<()> {
    let path = instrument_preset_save_path_with_paths(crate::app_paths::app_paths(), name)?;
    let mut cache = preset_bank_cache().lock().unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bank = InstrumentPresetBank {
        version: 1,
        engine_name: name.to_string(),
        source_file: format!("instruments/{name}.lisp"),
        presets: presets.to_vec(),
    };
    let json = serde_json::to_string_pretty(&bank).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to serialize preset bank '{}': {e}", path.display()),
        )
    })?;
    std::fs::write(&path, json)?;
    cache.remove(&path);
    Ok(())
}

pub(in crate::lisp_host) const INSTRUMENT_REGISTRY_SIZE: usize = MAX_INSTRUMENT_ENGINES * MAX_VOICES;
pub(in crate::lisp_host) static DGEN_INSTRUMENT_FNS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
pub(in crate::lisp_host) static DGEN_INSTRUMENT_OUTPUT_COUNTS: [AtomicUsize; INSTRUMENT_REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; INSTRUMENT_REGISTRY_SIZE]
};
pub(in crate::lisp_host) static DGEN_ENGINE_ENABLED_VOICES: [AtomicUsize; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicUsize = AtomicUsize::new(1);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
pub(in crate::lisp_host) static DGEN_ENGINE_PROCESS_CALLS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
pub(in crate::lisp_host) static DGEN_ENGINE_PROCESS_BLOCKS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};

#[derive(Clone, Copy, Debug)]
pub struct DGenEngineProcessStats {
    pub engine_id: usize,
    pub enabled_voices: usize,
    pub process_calls: u64,
    pub process_blocks: u64,
}

pub fn set_dgen_instrument_fn(slot_id: usize, f: DGenProcessFn) {
    DGEN_INSTRUMENT_FNS[slot_id % INSTRUMENT_REGISTRY_SIZE].store(f as usize, Ordering::Release);
}

pub fn set_dgen_instrument_output_count(slot_id: usize, count: usize) {
    DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
        .store(count.max(1), Ordering::Release);
}

pub fn set_dgen_engine_enabled_voices(engine_id: usize, count: usize) {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id].store(count.min(MAX_VOICES), Ordering::Release);
    }
}

pub fn get_dgen_engine_enabled_voices(engine_id: usize) -> usize {
    if engine_id < MAX_INSTRUMENT_ENGINES {
        DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .min(MAX_VOICES)
    } else {
        1
    }
}

pub fn reset_dgen_engine_enabled_voices(engine_id: usize) {
    set_dgen_engine_enabled_voices(engine_id, 1);
}

pub fn take_dgen_engine_process_stats() -> Vec<DGenEngineProcessStats> {
    (0..MAX_INSTRUMENT_ENGINES)
        .map(|engine_id| DGenEngineProcessStats {
            engine_id,
            enabled_voices: get_dgen_engine_enabled_voices(engine_id),
            process_calls: DGEN_ENGINE_PROCESS_CALLS[engine_id].swap(0, Ordering::AcqRel),
            process_blocks: DGEN_ENGINE_PROCESS_BLOCKS[engine_id].swap(0, Ordering::AcqRel),
        })
        .collect()
}

/// Wrapper process function for instrument nodes — reads from DGEN_INSTRUMENT_FNS.
unsafe extern "C" fn dgenlisp_instrument_wrapper_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    if state.is_null() {
        return;
    }
    let s = state as *mut f32;
    let slot_id = (*s) as usize;
    if slot_id >= INSTRUMENT_REGISTRY_SIZE {
        return;
    }
    if (*s.add(2)).to_bits() != HEADER_CANARY.to_bits() {
        return;
    }
    if *s.add(DGEN_ENABLED_PARAM_IDX) <= 0.5 {
        let nf = nframes as usize;
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
        if !out.is_null() {
            for ch in 0..output_count {
                let out_ch = *out.add(ch);
                if !out_ch.is_null() {
                    for i in 0..nf {
                        *out_ch.add(i) = 0.0;
                    }
                }
            }
        }
        return;
    }
    let engine_id = slot_id / MAX_VOICES;
    let voice_idx = slot_id % MAX_VOICES;
    if engine_id < MAX_INSTRUMENT_ENGINES {
        let enabled = DGEN_ENGINE_ENABLED_VOICES[engine_id]
            .load(Ordering::Acquire)
            .min(MAX_VOICES);
        if voice_idx >= enabled {
            let nf = nframes as usize;
            let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
                .load(Ordering::Acquire)
                .max(1);
            if !out.is_null() {
                for ch in 0..output_count {
                    let out_ch = *out.add(ch);
                    if !out_ch.is_null() {
                        for i in 0..nf {
                            *out_ch.add(i) = 0.0;
                        }
                    }
                }
            }
            return;
        }
    }
    let fn_ptr = DGEN_INSTRUMENT_FNS[slot_id % INSTRUMENT_REGISTRY_SIZE].load(Ordering::Acquire);
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let memory = dgen_memory_ptr(s) as *mut c_void;
        if inp.is_null() || out.is_null() {
            return;
        }
        if (*out.add(0)).is_null() {
            return;
        }
        if engine_id < MAX_INSTRUMENT_ENGINES {
            DGEN_ENGINE_PROCESS_CALLS[engine_id].fetch_add(1, Ordering::Relaxed);
            if voice_idx == 0 {
                DGEN_ENGINE_PROCESS_BLOCKS[engine_id].fetch_add(1, Ordering::Relaxed);
            }
        }
        let context = dgen_process_context_v1(dgen_host_sample_rate(s));
        process_fn(
            inp as *const *const f32,
            out,
            nframes.max(0) as u32,
            memory,
            &context,
            dgen_host_services_v1(),
        );
    } else {
        let nf = nframes as usize;
        let output_count = DGEN_INSTRUMENT_OUTPUT_COUNTS[slot_id % INSTRUMENT_REGISTRY_SIZE]
            .load(Ordering::Acquire)
            .max(1);
        if !out.is_null() {
            for ch in 0..output_count {
                let out_ch = *out.add(ch);
                if !out_ch.is_null() {
                    for i in 0..nf {
                        *out_ch.add(i) = 0.0;
                    }
                }
            }
        }
    }
}

pub fn dgenlisp_instrument_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_instrument_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

/// Build init message for a voice-aware instrument node.
/// Sets slot_id, total_memory_slots, param defaults, tensor data,
/// and voice_cell_id = voice_index.
pub fn build_init_message_for_voice(
    slot_id: usize,
    manifest: &DGenManifest,
    voice_index: usize,
) -> Vec<f32> {
    let mut entries = init_state_entries(manifest);

    // Set voice cell to voice_index
    if let Some(cell) = manifest.voice_cell_id {
        if cell < manifest.total_memory_slots {
            entries.push((cell, voice_index as f32));
        }
    }

    // Header (10) + pairs (2 * N). Instrument nodes resolve their process
    // function through DGEN_INSTRUMENT_FNS, so the pointer chunks stay zero.
    let mut msg = Vec::with_capacity(10 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(HEADER_CANARY);
    msg.push(manifest.n_inputs as f32);
    msg.push(1.0);
    msg.extend([0.0; DGEN_PROCESS_FN_CHUNKS]);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
}

// ── Instrument storage ──

fn writable_instrument_source_path(name: &str) -> io::Result<PathBuf> {
    writable_instrument_source_path_with_paths(crate::app_paths::app_paths(), name)
}

fn writable_instrument_source_path_with_paths(
    paths: &crate::app_paths::AppPaths,
    name: &str,
) -> io::Result<PathBuf> {
    let logical_name = match parse_instrument_id(name)? {
        Some((InstrumentTier::Factory, _)) => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("factory instrument '{name}' is read-only; fork it before editing"),
            ));
        }
        Some((InstrumentTier::User, logical_name)) => {
            let existing = resolve_instrument_storage_path_with_paths(paths, name, "lisp")?;
            if existing.is_file() {
                return Ok(existing);
            }
            logical_name
        }
        None => name.trim_end_matches('/'),
    };
    let root = paths.user_instruments_dir();
    if name.ends_with('/') {
        Ok(root.join(logical_name).join("dsp.lisp"))
    } else {
        Ok(root.join(format!("{logical_name}.lisp")))
    }
}

pub fn save_instrument(name: &str, source: &str) -> io::Result<()> {
    let path = writable_instrument_source_path(name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)
}

pub fn save_instrument_ui(name: &str, source: &str) -> io::Result<()> {
    let instrument_source = writable_instrument_source_path(name)?;
    let path = if instrument_source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        instrument_source.parent().unwrap_or(&instrument_source).join("ui.lisp")
    } else {
        instrument_source.with_extension("").join("ui.lisp")
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, source)
}

pub fn instrument_ui_path(name: &str) -> io::Result<PathBuf> {
    if name.ends_with('/') {
        let direct = crate::app_paths::app_paths()
            .instrument_dirs()
            .into_iter()
            .map(|root| root.join(name.trim_end_matches('/')).join("ui.lisp"))
            .find(|path| path.exists());
        if let Some(direct) = direct {
            return Ok(direct);
        }
    } else {
        if let Some(direct) = crate::app_paths::app_paths()
            .instrument_dirs()
            .into_iter()
            .map(|root| root.join(name).join("ui.lisp"))
            .find(|path| path.exists())
        {
            return Ok(direct);
        }
    }
    Ok(resolve_instrument_folder_path(name)?.join("ui.lisp"))
}

pub fn load_instrument_ui_source(name: &str) -> io::Result<String> {
    std::fs::read_to_string(instrument_ui_path(name)?)
}

pub fn list_saved_instruments() -> Vec<String> {
    fn is_hidden(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with('.'))
            .unwrap_or(false)
    }

    fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            if path.is_dir() {
                if path.join("dsp.lisp").exists() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(format!("{}/", rel.to_string_lossy().replace('\\', "/")));
                    }
                }
                collect(&path, root, out);
            } else if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                let file_stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                if matches!(file_stem, "dsp" | "ui" | "presets") {
                    continue;
                }
                if let Ok(rel) = path.strip_prefix(root) {
                    let without_ext = rel.with_extension("");
                    out.push(without_ext.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }

    let mut names = Vec::new();
    for dir in crate::app_paths::app_paths().instrument_dirs() {
        collect(&dir, &dir, &mut names);
    }
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

pub(in crate::lisp_host) fn validate_instrument_relative_dir(path: &str) -> io::Result<PathBuf> {
    let trimmed = path.trim().trim_matches('/');
    let mut relative = PathBuf::new();
    if trimmed.is_empty() {
        return Ok(relative);
    }
    for component in Path::new(trimmed).components() {
        match component {
            std::path::Component::Normal(part) => relative.push(part),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid instrument folder '{path}'"),
                ));
            }
        }
    }
    Ok(relative)
}

pub fn move_saved_instrument(name: &str, target_folder: &str) -> io::Result<String> {
    let root = crate::app_paths::app_paths().user_instruments_dir();
    let source = writable_instrument_source_path(name)?;
    if !source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("instrument '{name}' does not exist"),
        ));
    }

    let target_dir = root.join(validate_instrument_relative_dir(target_folder)?);
    if !target_dir.exists() || !target_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "target instrument folder '{}' does not exist",
                target_dir.display()
            ),
        ));
    }

    if source.file_name().and_then(|file| file.to_str()) == Some("dsp.lisp") {
        let source_dir = source.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "instrument source '{}' has no parent directory",
                    source.display()
                ),
            )
        })?;
        if source_dir == target_dir {
            return Ok(name.to_string());
        }
        if target_dir.starts_with(source_dir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot move an instrument folder into itself",
            ));
        }
        let folder_name = source_dir.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument folder '{}' has no name", source_dir.display()),
            )
        })?;
        let dest = target_dir.join(folder_name);
        if dest.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("target instrument '{}' already exists", dest.display()),
            ));
        }
        std::fs::rename(source_dir, &dest)?;
        return dest
            .strip_prefix(root)
            .map(|rel| format!("{}/", rel.to_string_lossy().replace('\\', "/")))
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()));
    }

    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("instrument source '{}' has no file stem", source.display()),
            )
        })?;
    let dest_source = target_dir.join(format!("{stem}.lisp"));
    if dest_source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "target instrument '{}' already exists",
                dest_source.display()
            ),
        ));
    }
    let mut sidecars = Vec::new();
    for extension in ["presets", "instrument.json"] {
        let sidecar = source.with_extension(extension);
        if sidecar.exists() {
            let dest = target_dir.join(sidecar.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("sidecar '{}' has no file name", sidecar.display()),
                )
            })?);
            if dest.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("target sidecar '{}' already exists", dest.display()),
                ));
            }
            sidecars.push((sidecar, dest));
        }
    }
    std::fs::rename(&source, &dest_source)?;
    for (sidecar, dest) in sidecars {
        std::fs::rename(sidecar, dest)?;
    }
    dest_source
        .strip_prefix(root)
        .map(|rel| rel.with_extension("").to_string_lossy().replace('\\', "/"))
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))
}

pub fn load_instrument_source(name: &str) -> io::Result<String> {
    let path = resolve_instrument_storage_path(name, "lisp")?;
    std::fs::read_to_string(&path)
}

#[cfg(test)]
mod tier_id_tests {
    use super::*;

    fn test_paths(label: &str) -> (crate::app_paths::AppPaths, PathBuf) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("eseq-instrument-id-{label}-{unique}"));
        let paths = crate::app_paths::AppPaths::dev(
            root.join("crates/sequencer"),
            root.clone(),
            root.join("config"),
        );
        (paths, root)
    }

    fn write_folder_instrument(root: &Path, name: &str, marker: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dsp.lisp"), marker).unwrap();
    }

    #[test]
    fn bare_instrument_ids_prefer_factory_and_qualified_ids_select_exact_tier() {
        let (paths, root) = test_paths("collision");
        write_folder_instrument(&paths.instruments_dir(), "shared/lead", "factory");
        write_folder_instrument(&paths.user_instruments_dir(), "shared/lead", "user");

        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "shared/lead").unwrap(),
            "factory:shared/lead"
        );
        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(
                    &paths,
                    "user:shared/lead",
                    "lisp",
                )
                .unwrap(),
            )
            .unwrap(),
            "user"
        );
        assert_eq!(
            std::fs::read_to_string(
                resolve_instrument_storage_path_with_paths(
                    &paths,
                    "factory:shared/lead",
                    "lisp",
                )
                .unwrap(),
            )
            .unwrap(),
            "factory"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn factory_instrument_presets_are_user_data_saved_and_loaded_from_the_user_tier() {
        let (paths, root) = test_paths("factory-presets");
        write_folder_instrument(&paths.instruments_dir(), "core/drift", "factory");

        let save_path = instrument_preset_save_path_with_paths(&paths, "factory:core/drift")
            .expect("saving presets on a factory instrument must not require forking it");
        assert_eq!(
            save_path,
            paths.user_instruments_dir().join("core/drift.presets")
        );
        std::fs::create_dir_all(save_path.parent().unwrap()).unwrap();
        std::fs::write(&save_path, "user bank").unwrap();
        assert_eq!(
            instrument_preset_path_with_paths(&paths, "factory:core/drift").unwrap(),
            save_path
        );

        // A factory-shipped bank still wins on load, matching the legacy
        // factory-first root order for bare names.
        let factory_bank = paths.instruments_dir().join("core/drift.presets");
        std::fs::write(&factory_bank, "factory bank").unwrap();
        assert_eq!(
            instrument_preset_path_with_paths(&paths, "factory:core/drift").unwrap(),
            factory_bank
        );

        // User-tier instruments keep their bank next to the source.
        write_folder_instrument(&paths.user_instruments_dir(), "mine", "user");
        assert_eq!(
            instrument_preset_save_path_with_paths(&paths, "user:mine").unwrap(),
            paths.user_instruments_dir().join("mine.presets")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bare_instrument_id_falls_back_to_user_and_rejects_unknown_tiers() {
        let (paths, root) = test_paths("user-fallback");
        write_folder_instrument(&paths.user_instruments_dir(), "mine", "user");

        assert_eq!(
            qualify_instrument_id_with_paths(&paths, "mine/").unwrap(),
            "user:mine"
        );
        let error = qualify_instrument_id_with_paths(&paths, "pkg:someone/mine")
            .expect_err("package ids are not part of the T3 tier resolver");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        std::fs::remove_dir_all(root).unwrap();
    }
}

// ── Instrument compilation ──
