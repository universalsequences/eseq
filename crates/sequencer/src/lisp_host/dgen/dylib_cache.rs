/*!
Content-addressed cache of compiled DGenLisp dylibs.

Compiling through the external dgenlisp tool is slow, so `DylibCacheManager`
(see `global_cache_manager()`) keys each artifact by a fingerprint of the
effective source, referenced assets, compile kind (`DGenCompileKind`), source
origin, sample rate, and the dgenlisp tool binary itself. A cache hit hands out
a `DylibLease` (refcounted so an artifact directory is not evicted while a
loaded dylib still points into it); a miss compiles into a fresh artifact
directory and records `CacheMetadata`. Includes a small lisp tokenizer used to
discover `(asset ...)` references that must participate in the fingerprint.
*/

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{
    compile_effective_dgen_source_to_dir, dgenlisp_tool_path, effective_dgen_source,
    load_dylib_prewarmed, parse_manifest_with_base, CompileResult,
};

const CACHE_SCHEMA_VERSION: u32 = 1;
const INSTRUMENT_VOICES: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DGenCompileKind {
    Effect,
    Instrument,
}

impl DGenCompileKind {
    fn as_str(self) -> &'static str {
        match self {
            DGenCompileKind::Effect => "effect",
            DGenCompileKind::Instrument => "instrument",
        }
    }

    fn voices(self) -> Option<u32> {
        match self {
            DGenCompileKind::Effect => None,
            DGenCompileKind::Instrument => Some(INSTRUMENT_VOICES),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DGenSourceOrigin {
    Custom,
    BuiltinConvolutionReverb,
    Draft,
}

#[derive(Clone, Debug)]
pub struct DylibCacheManager {
    inner: Arc<Mutex<DylibCacheInner>>,
}

#[derive(Debug)]
struct DylibCacheInner {
    root: PathBuf,
    live_leases: HashMap<PathBuf, usize>,
    next_artifact_seq: u64,
}

#[derive(Debug)]
pub struct DylibLease {
    manager: Weak<Mutex<DylibCacheInner>>,
    artifact_dir: PathBuf,
    released: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct AssetFingerprint {
    reference: String,
    path: String,
    exists: bool,
    len: Option<u64>,
    modified_unix_ms: Option<u128>,
    sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ToolFingerprint {
    path: String,
    exists: bool,
    len: Option<u64>,
    modified_unix_ms: Option<u128>,
    sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheMetadata {
    schema_version: u32,
    key: String,
    kind: DGenCompileKind,
    origin: DGenSourceOrigin,
    sample_rate: u32,
    voices: Option<u32>,
    effective_source_sha256: String,
    tool: ToolFingerprint,
    assets: Vec<AssetFingerprint>,
    dylib_name: String,
}

#[derive(Clone, Debug)]
struct CacheRequest {
    key: String,
    kind: DGenCompileKind,
    origin: DGenSourceOrigin,
    sample_rate: u32,
    voices: Option<u32>,
    effective_source: String,
    effective_source_sha256: String,
    tool: ToolFingerprint,
    assets: Vec<AssetFingerprint>,
}

impl DylibCacheManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DylibCacheInner {
                root,
                live_leases: HashMap::new(),
                next_artifact_seq: 0,
            })),
        }
    }

    pub fn workspace_default() -> Self {
        Self::new(crate::app_paths::app_paths().dgen_cache_root())
    }

    pub fn acquire(
        &self,
        kind: DGenCompileKind,
        origin: DGenSourceOrigin,
        source: &str,
        sample_rate: u32,
        asset_base: Option<&Path>,
    ) -> Result<CompileResult, String> {
        let request = build_request(kind, origin, source, sample_rate, asset_base)?;

        if let Some(result) = self.try_load_free_artifact(&request)? {
            return Ok(result);
        }

        self.compile_new_artifact(&request, asset_base)
    }

    fn try_load_free_artifact(
        &self,
        request: &CacheRequest,
    ) -> Result<Option<CompileResult>, String> {
        let artifact_dirs = self.artifact_dirs(&request.key)?;
        for artifact_dir in artifact_dirs {
            match metadata_matches(&artifact_dir, request) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    eprintln!(
                        "[dgenlisp cache] ignoring cached artifact {}: {error}",
                        artifact_dir.display()
                    );
                    continue;
                }
            }
            let Some(lease) = self.try_lease_artifact(&artifact_dir)? else {
                continue;
            };
            match self.load_leased_artifact(&artifact_dir, lease) {
                Ok(result) => return Ok(Some(result)),
                Err(error) => {
                    eprintln!(
                        "[dgenlisp cache] ignoring cached artifact {}: {error}",
                        artifact_dir.display()
                    );
                }
            }
        }
        Ok(None)
    }

    fn artifact_dirs(&self, key: &str) -> Result<Vec<PathBuf>, String> {
        let root = self
            .inner
            .lock()
            .map_err(|_| "DGenLisp cache lock poisoned".to_string())?
            .root
            .join("dylibs")
            .join(key);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(Vec::new());
        };
        let mut dirs = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        dirs.sort();
        Ok(dirs)
    }

    fn compile_new_artifact(
        &self,
        request: &CacheRequest,
        asset_base: Option<&Path>,
    ) -> Result<CompileResult, String> {
        let (cache_root, artifact_id) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "DGenLisp cache lock poisoned".to_string())?;
            let artifact_id = format!(
                "{}-{}-{}",
                process_id(),
                now_unix_ms(),
                inner.next_artifact_seq
            );
            inner.next_artifact_seq = inner.next_artifact_seq.wrapping_add(1);
            (inner.root.clone(), artifact_id)
        };

        let key_dir = cache_root.join("dylibs").join(&request.key);
        let staging_parent = cache_root.join("staging");
        std::fs::create_dir_all(&key_dir)
            .map_err(|e| format!("create dylib cache key dir: {e}"))?;
        std::fs::create_dir_all(&staging_parent)
            .map_err(|e| format!("create dylib cache staging dir: {e}"))?;

        let staging_dir = staging_parent.join(format!("{artifact_id}.tmp"));
        let artifact_dir = key_dir.join(&artifact_id);
        if staging_dir.exists() {
            std::fs::remove_dir_all(&staging_dir)
                .map_err(|e| format!("remove stale cache staging dir: {e}"))?;
        }
        std::fs::create_dir_all(&staging_dir)
            .map_err(|e| format!("create dylib cache staging artifact: {e}"))?;

        let dylib_name = format!(
            "dgen_{}_{}",
            request.kind.as_str(),
            sanitize_name(&artifact_id)
        );
        let manifest_json = match compile_effective_dgen_source_to_dir(
            request.kind,
            &request.effective_source,
            request.sample_rate,
            asset_base,
            &staging_dir,
            &dylib_name,
        ) {
            Ok(json) => json,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging_dir);
                return Err(error);
            }
        };

        let metadata = CacheMetadata {
            schema_version: CACHE_SCHEMA_VERSION,
            key: request.key.clone(),
            kind: request.kind,
            origin: request.origin,
            sample_rate: request.sample_rate,
            voices: request.voices,
            effective_source_sha256: request.effective_source_sha256.clone(),
            tool: request.tool.clone(),
            assets: request.assets.clone(),
            dylib_name,
        };
        write_text_file(&staging_dir.join("source.lisp"), &request.effective_source)?;
        write_text_file(&staging_dir.join("manifest.json"), &manifest_json)?;
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| format!("serialize dylib cache metadata: {e}"))?;
        write_text_file(
            &staging_dir.join("metadata.json"),
            &format!("{metadata_json}\n"),
        )?;

        std::fs::rename(&staging_dir, &artifact_dir)
            .map_err(|e| format!("commit dylib cache artifact: {e}"))?;
        self.load_artifact(&artifact_dir)
    }

    fn load_artifact(&self, artifact_dir: &Path) -> Result<CompileResult, String> {
        let lease = self.try_lease_artifact(artifact_dir)?.ok_or_else(|| {
            format!(
                "cached artifact is already in use: {}",
                artifact_dir.display()
            )
        })?;
        self.load_leased_artifact(artifact_dir, lease)
    }

    fn load_leased_artifact(
        &self,
        artifact_dir: &Path,
        lease: DylibLease,
    ) -> Result<CompileResult, String> {
        let manifest_json = std::fs::read_to_string(artifact_dir.join("manifest.json"))
            .map_err(|e| format!("read cached manifest: {e}"))?;
        let manifest = parse_manifest_with_base(&manifest_json, artifact_dir)?;
        let lib = match load_dylib_prewarmed(&manifest) {
            Ok(lib) => lib,
            Err(error) => {
                drop(lease);
                return Err(error);
            }
        };
        Ok(CompileResult {
            manifest,
            lib,
            lease: Some(lease),
        })
    }

    fn lease_artifact(&self, artifact_dir: &Path) -> Result<DylibLease, String> {
        self.try_lease_artifact(artifact_dir)?.ok_or_else(|| {
            format!(
                "cached artifact is already in use: {}",
                artifact_dir.display()
            )
        })
    }

    fn try_lease_artifact(&self, artifact_dir: &Path) -> Result<Option<DylibLease>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "DGenLisp cache lock poisoned".to_string())?;
        if inner.live_leases.contains_key(artifact_dir) {
            return Ok(None);
        }
        inner.live_leases.insert(artifact_dir.to_path_buf(), 1);
        Ok(Some(DylibLease {
            manager: Arc::downgrade(&self.inner),
            artifact_dir: artifact_dir.to_path_buf(),
            released: false,
        }))
    }

    fn release_artifact(&self, artifact_dir: &Path) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.release(artifact_dir);
        }
    }

    #[cfg(test)]
    pub(crate) fn test_lease(&self, artifact_dir: &Path) -> DylibLease {
        self.lease_artifact(artifact_dir)
            .expect("test artifact should be available for leasing")
    }

    #[cfg(test)]
    pub(crate) fn live_lease_count(&self, artifact_dir: &Path) -> usize {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.live_leases.get(artifact_dir).copied())
            .unwrap_or(0)
    }
}

impl Default for DylibCacheManager {
    fn default() -> Self {
        Self::workspace_default()
    }
}

impl DylibLease {
    pub fn artifact_dir(&self) -> &Path {
        &self.artifact_dir
    }

    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        if let Ok(mut inner) = manager.lock() {
            inner.release(&self.artifact_dir);
        };
    }
}

impl Drop for DylibLease {
    fn drop(&mut self) {
        self.release_inner();
    }
}

impl DylibCacheInner {
    fn release(&mut self, artifact_dir: &Path) {
        let Some(count) = self.live_leases.get_mut(artifact_dir) else {
            return;
        };
        if *count <= 1 {
            self.live_leases.remove(artifact_dir);
        } else {
            *count -= 1;
        }
    }
}

pub fn global_cache_manager() -> &'static DylibCacheManager {
    static GLOBAL: OnceLock<DylibCacheManager> = OnceLock::new();
    GLOBAL.get_or_init(DylibCacheManager::workspace_default)
}

fn build_request(
    kind: DGenCompileKind,
    origin: DGenSourceOrigin,
    source: &str,
    sample_rate: u32,
    asset_base: Option<&Path>,
) -> Result<CacheRequest, String> {
    let effective_source = effective_dgen_source(kind, source, sample_rate)?;
    let effective_source_sha256 = sha256_hex(effective_source.as_bytes());
    let tool = fingerprint_tool(&dgenlisp_tool_path())?;
    let assets = fingerprint_source_assets(&effective_source, asset_base)?;
    let voices = kind.voices();
    let key_material = serde_json::json!({
        "schemaVersion": CACHE_SCHEMA_VERSION,
        "kind": kind,
        "sampleRate": sample_rate,
        "voices": voices,
        "effectiveSourceSha256": effective_source_sha256,
        "tool": tool,
        "assets": assets,
    });
    let key = sha256_hex(
        serde_json::to_string(&key_material)
            .map_err(|e| format!("serialize dylib cache key: {e}"))?
            .as_bytes(),
    );
    Ok(CacheRequest {
        key,
        kind,
        origin,
        sample_rate,
        voices,
        effective_source,
        effective_source_sha256,
        tool,
        assets,
    })
}

fn metadata_matches(artifact_dir: &Path, request: &CacheRequest) -> Result<bool, String> {
    let metadata_path = artifact_dir.join("metadata.json");
    let Ok(source) = std::fs::read_to_string(&metadata_path) else {
        return Ok(false);
    };
    let metadata: CacheMetadata = serde_json::from_str(&source).map_err(|e| {
        format!(
            "parse dylib cache metadata {}: {e}",
            metadata_path.display()
        )
    })?;
    if metadata.schema_version != CACHE_SCHEMA_VERSION
        || metadata.key != request.key
        || metadata.kind != request.kind
        || metadata.sample_rate != request.sample_rate
        || metadata.voices != request.voices
        || metadata.effective_source_sha256 != request.effective_source_sha256
        || metadata.tool != request.tool
        || metadata.assets != request.assets
    {
        return Ok(false);
    }
    Ok(artifact_dir.join("manifest.json").is_file()
        && artifact_dir.join("source.lisp").is_file()
        && artifact_dir
            .join(format!("{}.dylib", metadata.dylib_name))
            .is_file())
}

fn fingerprint_tool(path: &Path) -> Result<ToolFingerprint, String> {
    let path_string = canonical_or_absolute(path).to_string_lossy().to_string();
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("read DGenLisp tool for cache fingerprint: {e}"))?;
            Ok(ToolFingerprint {
                path: path_string,
                exists: true,
                len: Some(metadata.len()),
                modified_unix_ms: modified_unix_ms(&metadata),
                sha256: Some(sha256_hex(&bytes)),
            })
        }
        _ => Ok(ToolFingerprint {
            path: path_string,
            exists: false,
            len: None,
            modified_unix_ms: None,
            sha256: None,
        }),
    }
}

fn fingerprint_source_assets(
    source: &str,
    asset_base: Option<&Path>,
) -> Result<Vec<AssetFingerprint>, String> {
    let references = asset_references(source)?;
    let base = match asset_base {
        Some(path) => path.to_path_buf(),
        None => crate::app_paths::app_paths().dgen_asset_fallback_base(),
    };
    let mut by_path = BTreeMap::<PathBuf, String>::new();
    for reference in references {
        let path = PathBuf::from(&reference);
        let resolved = if path.is_absolute() {
            path
        } else {
            base.join(path)
        };
        by_path.entry(resolved).or_insert(reference);
    }

    let mut out = Vec::with_capacity(by_path.len());
    for (path, reference) in by_path {
        let path_string = canonical_or_absolute(&path).to_string_lossy().to_string();
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let bytes = std::fs::read(&path).map_err(|e| {
                    format!(
                        "read DGenLisp asset '{}' for cache fingerprint: {e}",
                        path.display()
                    )
                })?;
                out.push(AssetFingerprint {
                    reference,
                    path: path_string,
                    exists: true,
                    len: Some(metadata.len()),
                    modified_unix_ms: modified_unix_ms(&metadata),
                    sha256: Some(sha256_hex(&bytes)),
                });
            }
            _ => out.push(AssetFingerprint {
                reference,
                path: path_string,
                exists: false,
                len: None,
                modified_unix_ms: None,
                sha256: None,
            }),
        }
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Symbol(String),
    String(String),
}

fn asset_references(source: &str) -> Result<Vec<String>, String> {
    let tokens = tokenize_lisp(source)?;
    let mut refs = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        let is_asset_keyword = matches!(
            &tokens[idx],
            Token::Symbol(symbol) if symbol == "@file" || symbol == "@default-file"
        );
        if is_asset_keyword {
            let Some(next) = tokens.get(idx + 1) else {
                return Err("DGenLisp asset keyword missing string path".to_string());
            };
            let Token::String(reference) = next else {
                return Err("DGenLisp asset keyword must be followed by a string path".to_string());
            };
            refs.push(reference.clone());
            idx += 2;
        } else {
            idx += 1;
        }
    }
    refs.sort();
    refs.dedup();
    Ok(refs)
}

fn tokenize_lisp(source: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars = source.chars().collect::<Vec<_>>();
    let mut idx = 0;
    while idx < chars.len() {
        match chars[idx] {
            ';' => {
                idx += 1;
                while idx < chars.len() && chars[idx] != '\n' {
                    idx += 1;
                }
            }
            '"' => {
                idx += 1;
                let mut value = String::new();
                while idx < chars.len() {
                    match chars[idx] {
                        '"' => {
                            idx += 1;
                            break;
                        }
                        '\\' => {
                            idx += 1;
                            if idx >= chars.len() {
                                return Err("unterminated escape in DGenLisp string".to_string());
                            }
                            value.push(match chars[idx] {
                                'n' => '\n',
                                'r' => '\r',
                                't' => '\t',
                                other => other,
                            });
                            idx += 1;
                        }
                        ch => {
                            value.push(ch);
                            idx += 1;
                        }
                    }
                }
                tokens.push(Token::String(value));
            }
            '(' | ')' | '[' | ']' | '{' | '}' => {
                idx += 1;
            }
            ch if ch.is_whitespace() => {
                idx += 1;
            }
            _ => {
                let start = idx;
                while idx < chars.len()
                    && !chars[idx].is_whitespace()
                    && !matches!(chars[idx], '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';')
                {
                    idx += 1;
                }
                tokens.push(Token::Symbol(chars[start..idx].iter().collect()));
            }
        }
    }
    Ok(tokens)
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative paths reaching here (e.g. a relative asset base from a
            // caller) were historically anchored at the post-chdir cwd, i.e.
            // the sequencer crate dir.
            crate::app_paths::app_paths()
                .dgen_asset_fallback_base()
                .join(path)
        }
    })
}

fn modified_unix_ms(metadata: &std::fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn process_id() -> u32 {
    std::process::id()
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn write_text_file(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_reference_scanner_ignores_comments_and_strings() {
        let refs = asset_references(
            r#"
            ; (tensor @file "ignored.json")
            (def s "not @file \"also-ignored.json\"")
            (def t (tensor @shape [4] @file "a.json"))
            (def w (wavetable @default-file "waves/b.json"))
            "#,
        )
        .expect("scan references");
        assert_eq!(refs, vec!["a.json".to_string(), "waves/b.json".to_string()]);
    }

    #[test]
    fn lease_drop_releases_artifact() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let manager = DylibCacheManager::new(root);
        let artifact = PathBuf::from("/tmp/lease-drop-test");
        {
            let lease = manager.lease_artifact(&artifact).expect("lease");
            assert_eq!(manager.live_lease_count(&artifact), 1);
            drop(lease);
        }
        assert_eq!(manager.live_lease_count(&artifact), 0);
    }

    #[test]
    fn asset_fingerprint_changes_cache_key() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-asset-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let asset = root.join("asset.json");
        std::fs::write(&asset, "[1.0]").expect("write asset");
        let source = r#"
            (def t (tensor @shape [1] @file "asset.json"))
            (out (peek t 0) 1 @name out)
        "#;

        let first = build_request(
            DGenCompileKind::Effect,
            DGenSourceOrigin::Custom,
            source,
            44_100,
            Some(&root),
        )
        .expect("first request");
        std::fs::write(&asset, "[2.0]").expect("rewrite asset");
        let second = build_request(
            DGenCompileKind::Effect,
            DGenSourceOrigin::Custom,
            source,
            44_100,
            Some(&root),
        )
        .expect("second request");

        assert_ne!(first.key, second.key);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn acquire_compiles_duplicate_when_matching_artifact_is_live_then_reuses_after_release() {
        if !dgenlisp_tool_path().exists() {
            eprintln!(
                "skipping: DGenLisp tool not found at {:?}",
                dgenlisp_tool_path()
            );
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-acquire-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let manager = DylibCacheManager::new(root.clone());
        let source = r#"
            (def input_l (in 1 @name Left))
            (out input_l 1 @name Left)
        "#;

        let first = manager
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Custom,
                source,
                44_100,
                None,
            )
            .expect("first acquire");
        let first_dir = first
            .lease
            .as_ref()
            .expect("first lease")
            .artifact_dir()
            .to_path_buf();

        let second = manager
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Custom,
                source,
                44_100,
                None,
            )
            .expect("second acquire");
        let second_dir = second
            .lease
            .as_ref()
            .expect("second lease")
            .artifact_dir()
            .to_path_buf();
        assert_ne!(first_dir, second_dir);

        drop(first);
        let third = manager
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Custom,
                source,
                44_100,
                None,
            )
            .expect("third acquire");
        let third_dir = third
            .lease
            .as_ref()
            .expect("third lease")
            .artifact_dir()
            .to_path_buf();
        assert_eq!(third_dir, first_dir);

        drop(second);
        drop(third);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Slice E1 exit criterion (embedded-dgen-connector-impl-spec.md): with
    /// the working directory set to `/`, dgen compiles succeed using only
    /// `AppPaths`-resolved locations. `set_current_dir` is process-wide, so
    /// this must never run inside the threaded default suite; run it alone:
    ///
    /// ```text
    /// cargo test -p sequencer dgen_compile_succeeds_with_foreign_cwd -- --ignored
    /// ```
    #[test]
    #[ignore = "chdirs the whole process; run alone with -- --ignored"]
    fn dgen_compile_succeeds_with_foreign_cwd() {
        if !dgenlisp_tool_path().exists() {
            eprintln!(
                "skipping: DGenLisp tool not found at {:?}",
                dgenlisp_tool_path()
            );
            return;
        }

        // Mirror app startup: AppPaths captures its roots while the
        // workspace is locatable, then cwd becomes irrelevant.
        let _ = crate::app_paths::app_paths();
        let original_cwd = std::env::current_dir().ok();
        std::env::set_current_dir("/").expect("chdir to /");

        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-foreign-cwd-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let manager = DylibCacheManager::new(root.clone());

        // Effect compile through the cache manager.
        let effect_source = r#"
            (def input_l (in 1 @name Left))
            (out input_l 1 @name Left)
        "#;
        let effect = manager
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Custom,
                effect_source,
                44_100,
                None,
            )
            .expect("effect compile with cwd=/");
        assert!(Path::new(&effect.manifest.dylib_path).is_absolute());

        // Instrument compile through the cache manager (the spec's exit
        // criterion names an instrument compile).
        let instrument = manager
            .acquire(
                DGenCompileKind::Instrument,
                DGenSourceOrigin::Custom,
                crate::lisp_host::INSTRUMENT_TEMPLATE,
                44_100,
                None,
            )
            .expect("instrument compile with cwd=/");
        assert!(Path::new(&instrument.manifest.dylib_path).is_absolute());

        // Uncached compile path (exercises the scratch dir + tool path).
        crate::lisp_host::compile_lisp(effect_source, 44_100)
            .expect("uncached effect compile with cwd=/");

        drop(effect);
        drop(instrument);
        let _ = std::fs::remove_dir_all(root);
        if let Some(cwd) = original_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
    }

    #[test]
    fn persisted_restart_reuses_free_artifact() {
        if !dgenlisp_tool_path().exists() {
            eprintln!(
                "skipping: DGenLisp tool not found at {:?}",
                dgenlisp_tool_path()
            );
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-restart-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let source = r#"
            (def input_l (in 1 @name Left))
            (out input_l 1 @name Left)
        "#;

        let first_dir = {
            let manager = DylibCacheManager::new(root.clone());
            let result = manager
                .acquire(
                    DGenCompileKind::Effect,
                    DGenSourceOrigin::Custom,
                    source,
                    44_100,
                    None,
                )
                .expect("first acquire");
            result
                .lease
                .as_ref()
                .expect("first lease")
                .artifact_dir()
                .to_path_buf()
        };

        let restarted = DylibCacheManager::new(root.clone());
        let second = restarted
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Custom,
                source,
                44_100,
                None,
            )
            .expect("second acquire after restart");
        let second_dir = second
            .lease
            .as_ref()
            .expect("second lease")
            .artifact_dir()
            .to_path_buf();

        assert_eq!(second_dir, first_dir);
        drop(second);
        let _ = std::fs::remove_dir_all(root);
    }
}
