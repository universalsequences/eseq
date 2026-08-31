/*!
Content-addressed cache of compiled DGenLisp dylibs.

Compiling through the external dgenlisp tool is slow, so `DylibCacheManager`
(see `global_cache_manager()`) keys each artifact by a fingerprint of the
effective source, referenced assets, compile kind (`DGenCompileKind`), sample
rate, the dgenlisp tool binary itself, and the platform compile-policy
identity (policy hash, vendored ABI header hash, target triple, and optional
deployment target). A cache hit hands out a `DylibLease`; a miss compiles into
a fresh artifact directory and records `CacheMetadata`. Concurrent misses on the same
key are serialized by a per-key in-flight latch so exactly one compilation
runs; different keys still compile concurrently. Includes a small lisp
tokenizer used to discover `(asset ...)` references that must participate in
the fingerprint.

Leases are exclusive (eseq-599): an artifact directory is held by at most one
live lease, because every lease dlopens the artifact's dylib and dyld returns
one image per path — the generated code's file-scope scratch statics
(`static float tN_g[...]`, indexed by a hard-coded voice 0 for effects) would
be shared mutable state between instances, a data race under the audiograph's
worker threads. When every matching artifact is already leased, the manager
*clones* the artifact directory (a file copy, no recompilation) under a fresh
artifact id and dlopens the clone; the distinct path (and inode) forces dyld
to map an independent image with private statics. Released artifacts become
free and are reused by later acquires, so the on-disk artifact count per key
is bounded by the high-water mark of simultaneous instances. This design was
chosen over compiling effects with fixed polyphony (wastes memory for
instances that never exist, imposes an arbitrary instance cap, and still
collides across engines) and over toolchain-level runtime contexts (requires
a dgen codegen ABI change for state the host already passes per node).

An artifact directory is never evicted or mutated while a lease points into
it, and loaded images are never dlclosed, so sequential reuse of a freed
artifact re-observes the same image (never concurrently with another lease).

On-disk layout (impl spec, slice E6 / decision 7):

```text
<cache_root>/<schema>/<target-triple>/dylibs/<cache-key>/<artifact-id>/
<cache_root>/<schema>/<target-triple>/staging/<artifact-id>.tmp
```

Schema-1 leftovers (`dylibs/`, `staging/` directly under the cache root) are
ignored and deleted opportunistically by the construction-time sweep, which
also clears orphaned staging dirs and artifact dirs whose metadata fails to
parse. Because the cache root is shared between *processes* (each nextest
test is its own process; a second app instance is possible too), in-flight
staging dirs are protected by an advisory `flock` on a sibling
`<artifact-id>.tmp.lock` file (see [`StagingLock`], eseq-linux.51): a sweep
only deletes staging it can prove ownerless. Sibling dirs under the cache
root (e.g. `ir-prep/`, owned by the convolution reverb) are never touched.

Everything here runs on control threads only (edit sessions, agent tasks,
effect setup); nothing is reachable from the audio process callback.
*/

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::dgen_manifest::DGEN_SHARED_LIBRARY_EXTENSION;
use super::super::{
    compile_effective_dgen_source_to_dir, dgenlisp_tool_path, effective_dgen_source,
    load_dylib_prewarmed, parse_manifest_with_base, CompileResult,
};

const CACHE_SCHEMA_VERSION: u32 = 3;
/// The native lowering selected by the bundled compiler. Part of the on-disk
/// path tier and cache key so artifacts can never cross target boundaries.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const CACHE_TARGET_TRIPLE: &str = "arm64-apple-macos";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const CACHE_TARGET_TRIPLE: &str = "x86_64-unknown-linux-gnu";
#[cfg(target_os = "macos")]
const CACHE_DEPLOYMENT_TARGET: Option<&str> = Some("11.0");
#[cfg(target_os = "linux")]
const CACHE_DEPLOYMENT_TARGET: Option<&str> = None;
const INSTRUMENT_VOICES: u32 = 12;

/// The vendored ABI header is a build input: `dgen_ffi.rs` mirrors it as
/// `#[repr(C)]` types compiled into this binary. Hashing the compiled-in
/// bytes (rather than re-reading a file at runtime) guarantees the key
/// material matches the ABI this exact binary implements, works identically
/// in the release bundle (which ships no header file), and cannot drift from
/// filesystem state.
const ABI_HEADER_BYTES: &[u8] = include_bytes!("../../../audiograph/dgen_abi_v1.h");

fn abi_header_sha256() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| sha256_hex(ABI_HEADER_BYTES))
}

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
    BuiltinFilterTable,
    Draft,
}

#[derive(Clone, Debug)]
pub struct DylibCacheManager {
    inner: Arc<Mutex<DylibCacheInner>>,
}

#[derive(Debug)]
struct DylibCacheInner {
    root: PathBuf,
    /// Artifact directories currently held by a live (exclusive) lease.
    live_leases: HashSet<PathBuf>,
    /// Per-key compile latches: present while a compilation for that cache
    /// key is running. Guarded by the manager mutex, but the mutex is never
    /// held across compilation — waiters block on the latch, not on this map.
    in_flight: HashMap<String, Arc<InFlightLatch>>,
    next_artifact_seq: u64,
}

/// Latch a second requester for an in-flight cache key waits on. The leader
/// marks it done (successful or not) when its compile attempt finishes;
/// waiters then re-check the cache and, if the leader failed, take their own
/// turn as leader (retry-compile — simplest failure policy, so a failed
/// leader never strands waiters).
#[derive(Debug, Default)]
struct InFlightLatch {
    done: Mutex<bool>,
    cond: Condvar,
}

impl InFlightLatch {
    fn wait_done(&self) {
        let mut done = self.done.lock().unwrap_or_else(|p| p.into_inner());
        while !*done {
            done = self.cond.wait(done).unwrap_or_else(|p| p.into_inner());
        }
    }

    fn mark_done(&self) {
        *self.done.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.cond.notify_all();
    }
}

/// RAII leadership token for one cache key. Dropping it (normal return or
/// unwind) removes the in-flight entry and wakes every waiter, so a
/// panicking or failing leader can never hang the queue.
struct CompileTurnGuard {
    manager: Weak<Mutex<DylibCacheInner>>,
    key: String,
    latch: Arc<InFlightLatch>,
}

impl Drop for CompileTurnGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            if let Ok(mut inner) = manager.lock() {
                if let Some(current) = inner.in_flight.get(&self.key) {
                    if Arc::ptr_eq(current, &self.latch) {
                        inner.in_flight.remove(&self.key);
                    }
                }
            }
        }
        self.latch.mark_done();
    }
}

enum CompileTurn {
    Leader(CompileTurnGuard),
    Waiter(Arc<InFlightLatch>),
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

/// Identity of the platform compile policy + vendored ABI (impl spec, slice
/// E6). `policy_sha256` covers macOS's staged `VERSION.json`, or Linux's ELF
/// symbol-policy files; `abi_header_sha256` is the compiled-in vendored header
/// (see [`ABI_HEADER_BYTES`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ToolchainFingerprint {
    policy_sha256: String,
    abi_header_sha256: String,
    target_triple: String,
    deployment_target: Option<String>,
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
    toolchain: ToolchainFingerprint,
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
    toolchain: ToolchainFingerprint,
    assets: Vec<AssetFingerprint>,
}

impl DylibCacheManager {
    pub fn new(root: PathBuf) -> Self {
        sweep_cache_root(&root);
        Self {
            inner: Arc::new(Mutex::new(DylibCacheInner {
                root,
                live_leases: HashSet::new(),
                in_flight: HashMap::new(),
                next_artifact_seq: 0,
            })),
        }
    }

    /// The process-wide manager for the workspace cache root. Lease
    /// exclusivity (eseq-599) only holds if every acquirer of one cache root
    /// shares one lease table, so this returns a clone of the global
    /// singleton rather than a fresh manager. (Other *processes* on the same
    /// root need no coordination: dlopen'd images are per-process, so their
    /// leases can never alias mutable state with ours.)
    pub fn workspace_default() -> Self {
        global_cache_manager().clone()
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

        loop {
            if let Some(result) = self.try_satisfy_from_cache(&request)? {
                return Ok(result);
            }
            match self.begin_compile_turn(&request.key)? {
                CompileTurn::Leader(_guard) => {
                    // Double-check under leadership: a previous leader may
                    // have published between this thread's cache miss and it
                    // winning the compile turn (publication happens-before
                    // its guard drop, which happens-before this turn).
                    if let Some(result) = self.try_satisfy_from_cache(&request)? {
                        return Ok(result);
                    }
                    // `_guard` wakes waiters + clears the in-flight entry on
                    // any exit (Ok, Err, unwind).
                    return self.compile_new_artifact(&request, asset_base);
                }
                CompileTurn::Waiter(latch) => {
                    latch.wait_done();
                    // Loop: on leader success the re-check leases the
                    // published artifact; on leader failure this thread takes
                    // its own compile turn.
                }
            }
        }
    }

    fn begin_compile_turn(&self, key: &str) -> Result<CompileTurn, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "DGenLisp cache lock poisoned".to_string())?;
        if let Some(latch) = inner.in_flight.get(key) {
            return Ok(CompileTurn::Waiter(Arc::clone(latch)));
        }
        let latch = Arc::new(InFlightLatch::default());
        inner.in_flight.insert(key.to_string(), Arc::clone(&latch));
        Ok(CompileTurn::Leader(CompileTurnGuard {
            manager: Arc::downgrade(&self.inner),
            key: key.to_string(),
            latch,
        }))
    }

    /// Try to satisfy a request from already-compiled artifacts: lease a
    /// free matching artifact if one exists; otherwise, if every matching
    /// artifact is live-leased by another instance, clone one into a fresh
    /// artifact directory so this instance gets its own dyld image
    /// (eseq-599). Returns `Ok(None)` only when compilation is required —
    /// clone failures also fall back to compilation rather than poisoning
    /// the usable cache entries they were copied from.
    fn try_satisfy_from_cache(
        &self,
        request: &CacheRequest,
    ) -> Result<Option<CompileResult>, String> {
        let artifact_dirs = self.artifact_dirs(&request.key)?;
        let mut clone_template: Option<PathBuf> = None;
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
                // Live-leased elsewhere: usable as a clone template, never as
                // a shared image.
                clone_template.get_or_insert(artifact_dir);
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
        if let Some(template_dir) = clone_template {
            match self.clone_artifact(request, &template_dir) {
                Ok(result) => return Ok(Some(result)),
                Err(error) => {
                    eprintln!(
                        "[dgenlisp cache] cloning live artifact {} failed ({error}); \
                         falling back to compilation",
                        template_dir.display()
                    );
                }
            }
        }
        Ok(None)
    }

    fn cache_root(&self) -> Result<PathBuf, String> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| "DGenLisp cache lock poisoned".to_string())?
            .root
            .clone())
    }

    fn artifact_dirs(&self, key: &str) -> Result<Vec<PathBuf>, String> {
        let root = dylibs_root(&self.cache_root()?).join(key);
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

    /// Allocate a fresh artifact id and create its (empty) staging dir plus
    /// the key dir it will be committed into. Returns
    /// `(artifact_id, staging_dir, staging_lock, artifact_dir)`; the caller
    /// must keep the lock alive until the staging dir has been renamed into
    /// place or removed, or another process's sweep may delete it mid-write
    /// (eseq-linux.51).
    fn allocate_artifact_dirs(
        &self,
        key: &str,
    ) -> Result<(String, PathBuf, StagingLock, PathBuf), String> {
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

        let key_dir = dylibs_root(&cache_root).join(key);
        let staging_parent = staging_root(&cache_root);
        std::fs::create_dir_all(&key_dir)
            .map_err(|e| format!("create dylib cache key dir: {e}"))?;
        std::fs::create_dir_all(&staging_parent)
            .map_err(|e| format!("create dylib cache staging dir: {e}"))?;

        let staging_dir = staging_parent.join(format!("{artifact_id}.tmp"));
        let artifact_dir = key_dir.join(&artifact_id);
        // Lock before the staging dir exists: a sweeping process that can see
        // the dir must always find its lock held.
        let staging_lock = StagingLock::acquire(&staging_dir)?;
        if staging_dir.exists() {
            std::fs::remove_dir_all(&staging_dir)
                .map_err(|e| format!("remove stale cache staging dir: {e}"))?;
        }
        std::fs::create_dir_all(&staging_dir)
            .map_err(|e| format!("create dylib cache staging artifact: {e}"))?;
        Ok((artifact_id, staging_dir, staging_lock, artifact_dir))
    }

    /// eseq-599: materialize an independent copy of a live-leased artifact.
    /// A byte-identical dylib at a *different path* (and inode — the copy is
    /// a new file, even when APFS clones the blocks) makes dyld map a second,
    /// independent image, so the generated code's file-scope scratch statics
    /// are private to this instance. No recompilation happens, keeping the
    /// slice-E6 "one compilation per key" property.
    fn clone_artifact(
        &self,
        request: &CacheRequest,
        template_dir: &Path,
    ) -> Result<CompileResult, String> {
        let (_artifact_id, staging_dir, _staging_lock, artifact_dir) =
            self.allocate_artifact_dirs(&request.key)?;
        if let Err(error) = copy_artifact_files(template_dir, &staging_dir) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(error);
        }
        std::fs::rename(&staging_dir, &artifact_dir)
            .map_err(|e| format!("commit cloned dylib cache artifact: {e}"))?;
        self.load_artifact(&artifact_dir)
    }

    fn compile_new_artifact(
        &self,
        request: &CacheRequest,
        asset_base: Option<&Path>,
    ) -> Result<CompileResult, String> {
        let (artifact_id, staging_dir, _staging_lock, artifact_dir) =
            self.allocate_artifact_dirs(&request.key)?;

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

        // Rust binary audit (impl spec, slice E5) on the exact bytes about to
        // be published. The subprocess ran with --skip-inline-audit, so this
        // is the only audit on the production path; failure publishes
        // nothing.
        if let Err(error) = crate::lisp_host::dgen::dgen_audit::audit_dylib(
            &staging_dir.join(format!("{dylib_name}.{DGEN_SHARED_LIBRARY_EXTENSION}")),
        ) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(error);
        }

        let metadata = CacheMetadata {
            schema_version: CACHE_SCHEMA_VERSION,
            key: request.key.clone(),
            kind: request.kind,
            origin: request.origin,
            sample_rate: request.sample_rate,
            voices: request.voices,
            effective_source_sha256: request.effective_source_sha256.clone(),
            tool: request.tool.clone(),
            toolchain: request.toolchain.clone(),
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
                "freshly created dylib cache artifact {} is unexpectedly leased",
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

    /// Exclusive (eseq-599): at most one live lease per artifact directory,
    /// because a lease dlopens the artifact's dylib and shared images alias
    /// the generated code's mutable file-scope scratch statics across
    /// instances. Returns `Ok(None)` when the directory is already leased;
    /// the caller then clones the artifact instead of sharing it.
    /// `DylibLease` drop/release frees the directory for reuse.
    fn try_lease_artifact(&self, artifact_dir: &Path) -> Result<Option<DylibLease>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "DGenLisp cache lock poisoned".to_string())?;
        if !inner.live_leases.insert(artifact_dir.to_path_buf()) {
            return Ok(None);
        }
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
        self.try_lease_artifact(artifact_dir)
            .expect("test lease should not hit a poisoned lock")
            .expect("test artifact should be free for leasing")
    }

    #[cfg(test)]
    pub(crate) fn live_lease_count(&self, artifact_dir: &Path) -> usize {
        self.inner
            .lock()
            .ok()
            .map(|inner| usize::from(inner.live_leases.contains(artifact_dir)))
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
        self.live_leases.remove(artifact_dir);
    }
}

pub fn global_cache_manager() -> &'static DylibCacheManager {
    static GLOBAL: OnceLock<DylibCacheManager> = OnceLock::new();
    GLOBAL.get_or_init(|| DylibCacheManager::new(crate::app_paths::app_paths().dgen_cache_root()))
}

/// Path tier for the current schema:
/// `<cache_root>/<schema>/<target-triple>/` (impl spec, decision 7).
fn schema_tier_root(cache_root: &Path) -> PathBuf {
    cache_root
        .join(CACHE_SCHEMA_VERSION.to_string())
        .join(CACHE_TARGET_TRIPLE)
}

fn dylibs_root(cache_root: &Path) -> PathBuf {
    schema_tier_root(cache_root).join("dylibs")
}

fn staging_root(cache_root: &Path) -> PathBuf {
    schema_tier_root(cache_root).join("staging")
}

/// Sibling lock file marking a staging dir as in-flight:
/// `<staging_dir>.lock` next to `<artifact-id>.tmp`.
fn staging_lock_path(staging_dir: &Path) -> PathBuf {
    let mut name = staging_dir
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    staging_dir.with_file_name(name)
}

/// Cross-process liveness marker for an in-flight staging dir (eseq-linux.51).
///
/// The construction-time sweep runs in every process sharing a cache root, so
/// "is this staging dir orphaned?" needs an answer that survives across
/// processes. An advisory exclusive `flock` on the sibling `.lock` file is
/// that answer: the compiling process holds it from *before* the staging dir
/// exists until *after* the dir is renamed into place (or cleaned up), and the
/// OS releases it automatically if the holder dies, so a sweeper that can
/// acquire the lock knows the dir has no live owner. `flock` handles from
/// separate `open`s conflict even within one process, so this also covers a
/// second manager instance sharing the root in-process.
struct StagingLock {
    lock_path: PathBuf,
    _file: std::fs::File,
}

impl StagingLock {
    /// Create and exclusively lock `<staging_dir>.lock`. Must be called
    /// before the staging dir itself is created, so a concurrent sweeper can
    /// never observe the dir without a held lock.
    ///
    /// A sweeper deletes an unlocked lock file while briefly holding its
    /// lock, so create-then-lock can race it: our freshly created file may be
    /// unlinked before we lock it, leaving us with a lock on an anonymous
    /// inode no sweeper will ever check. After locking, verify the fd still
    /// names the path and retry on mismatch (or on momentary contention).
    fn acquire(staging_dir: &Path) -> Result<Self, String> {
        let lock_path = staging_lock_path(staging_dir);
        let contended_os_error = fs2::lock_contended_error().raw_os_error();
        for _ in 0..64 {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&lock_path)
                .map_err(|e| format!("create staging lock {}: {e}", lock_path.display()))?;
            if let Err(error) = file.try_lock_exclusive() {
                if error.raw_os_error() == contended_os_error {
                    // A sweeper holds it for the instant it takes to unlink
                    // an orphan; artifact ids are unique, so no other writer
                    // can contend. Yield and retry.
                    std::thread::yield_now();
                    continue;
                }
                return Err(format!(
                    "lock staging lock {}: {error}",
                    lock_path.display()
                ));
            }
            let locked_ino = file
                .metadata()
                .map_err(|e| format!("stat staging lock {}: {e}", lock_path.display()))?;
            match std::fs::metadata(&lock_path) {
                Ok(on_disk) if same_inode(&locked_ino, &on_disk) => {
                    return Ok(Self {
                        lock_path,
                        _file: file,
                    })
                }
                // Unlinked (or replaced) by a sweeper between create and
                // lock: this lock protects nothing. Retry with a fresh file.
                _ => continue,
            }
        }
        Err(format!(
            "staging lock {} kept being swept while being acquired",
            lock_path.display()
        ))
    }
}

impl Drop for StagingLock {
    fn drop(&mut self) {
        // Unlink while still holding the lock, so no sweeper can acquire the
        // path between release and removal; the fd close releases the lock.
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn same_inode(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev() && a.ino() == b.ino()
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
    // Validate before cache lookup as well as before compilation. Otherwise a
    // cache entry produced by the old Linux system-compiler fallback could be
    // loaded even though no hermetic stage exists on this machine.
    let toolchain_root = crate::app_paths::app_paths().dgen_toolchain_root_checked()?;
    let toolchain = fingerprint_toolchain(&toolchain_root)?;
    let assets = fingerprint_source_assets(&effective_source, asset_base)?;
    let voices = kind.voices();
    let key = cache_key(
        kind,
        sample_rate,
        voices,
        &effective_source_sha256,
        &tool,
        &toolchain,
        &assets,
    )?;
    Ok(CacheRequest {
        key,
        kind,
        origin,
        sample_rate,
        voices,
        effective_source,
        effective_source_sha256,
        tool,
        toolchain,
        assets,
    })
}

fn cache_key_material(
    kind: DGenCompileKind,
    sample_rate: u32,
    voices: Option<u32>,
    effective_source_sha256: &str,
    tool: &ToolFingerprint,
    toolchain: &ToolchainFingerprint,
    assets: &[AssetFingerprint],
) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": CACHE_SCHEMA_VERSION,
        "kind": kind,
        "sampleRate": sample_rate,
        "voices": voices,
        "effectiveSourceSha256": effective_source_sha256,
        "tool": tool,
        "toolchain": toolchain,
        "assets": assets,
    })
}

fn cache_key(
    kind: DGenCompileKind,
    sample_rate: u32,
    voices: Option<u32>,
    effective_source_sha256: &str,
    tool: &ToolFingerprint,
    toolchain: &ToolchainFingerprint,
    assets: &[AssetFingerprint],
) -> Result<String, String> {
    let material = cache_key_material(
        kind,
        sample_rate,
        voices,
        effective_source_sha256,
        tool,
        toolchain,
        assets,
    );
    Ok(sha256_hex(
        serde_json::to_string(&material)
            .map_err(|e| format!("serialize dylib cache key: {e}"))?
            .as_bytes(),
    ))
}

/// Fingerprint every external compile-policy input not already covered by the
/// DGenLisp executable hash. Every host includes the mandatory staged
/// `VERSION.json`; Linux also includes the distribution's ELF audit policy.
fn fingerprint_toolchain(toolchain_root: &Path) -> Result<ToolchainFingerprint, String> {
    let version_path = toolchain_root.join("VERSION.json");
    let version = std::fs::read(&version_path).map_err(|e| {
        format!(
            "read staged toolchain VERSION.json for cache fingerprint at {}: {e}. \
             Run ./rebuild_dgenlisp_tool.sh at the repo root to stage the toolchain \
             (or fix ESEQ_DGEN_TOOLCHAIN_ROOT if the override is active).",
            version_path.display()
        )
    })?;
    #[cfg(target_os = "macos")]
    let policy_bytes = version;
    #[cfg(target_os = "linux")]
    let policy_bytes = {
        let abi_dir = crate::app_paths::app_paths().dgen_abi_dir();
        let mut bytes = version;
        for name in ["exports-v1-elf.txt", "libsystem-symbols-v1-elf.txt"] {
            let path = abi_dir.join(name);
            bytes.extend(std::fs::read(&path).map_err(|e| {
                format!("read Linux DGen ABI policy {}: {e}", path.display())
            })?);
            bytes.push(0);
        }
        bytes
    };
    Ok(ToolchainFingerprint {
        policy_sha256: sha256_hex(&policy_bytes),
        abi_header_sha256: abi_header_sha256().to_string(),
        target_triple: CACHE_TARGET_TRIPLE.to_string(),
        deployment_target: CACHE_DEPLOYMENT_TARGET.map(str::to_string),
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
        || metadata.toolchain != request.toolchain
        || metadata.assets != request.assets
    {
        return Ok(false);
    }
    Ok(artifact_dir.join("manifest.json").is_file()
        && artifact_dir.join("source.lisp").is_file()
        && artifact_dir
            .join(format!("{}.{}", metadata.dylib_name, DGEN_SHARED_LIBRARY_EXTENSION))
            .is_file())
}

/// Sweep-side view of a staging dir's sibling lock file (eseq-linux.51).
enum OrphanLock {
    /// The lock file does not exist: the dir's owner is gone (it unlinks the
    /// lock only after the staging dir has been renamed away or removed), or
    /// the dir predates the lock protocol. Safe to sweep; nothing to unlink.
    NoLockFile,
    /// The lock was acquired, so its owner died mid-compile. Dropping the
    /// guard unlinks the lock file while the lock is still held.
    Acquired(StagingLock),
}

/// Try to establish that a staging dir has no live owner. Returns `None`
/// when the lock is held by a live process (or the lock file is unreadable)
/// — in which case the dir must not be swept.
fn try_acquire_orphan_lock(lock_path: &Path) -> Option<OrphanLock> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(OrphanLock::NoLockFile)
        }
        // Unreadable for any other reason: assume live (sweeping is best
        // effort; deleting an in-flight compile is the one unsafe outcome).
        Err(_) => return None,
    };
    if file.try_lock_exclusive().is_err() {
        return None;
    }
    Some(OrphanLock::Acquired(StagingLock {
        lock_path: lock_path.to_path_buf(),
        _file: file,
    }))
}

/// Construction-time cache hygiene (impl spec, slice E6). Deletes, best
/// effort:
///
/// - schema-1 leftovers: the old `dylibs/` + `staging/` dirs directly under
///   the cache root (only those two names — sibling dirs like `ir-prep/`
///   belong to other subsystems and are never touched);
/// - orphaned staging dirs under the current schema tier. "Orphaned" is a
///   *liveness* question, not a pid-identity one: the cache root is shared by
///   other processes (nextest runs one process per test; a second manager
///   instance in this process is possible too), any of which may be
///   mid-compile right now, and pids say nothing about that (eseq-linux.51).
///   A live compile holds an exclusive [`StagingLock`] on the dir's sibling
///   `.lock` file for its whole lifetime, so a staging dir is an orphan
///   exactly when that lock can be acquired (or its lock file is gone);
/// - stale `.lock` files whose staging dir no longer exists and whose lock
///   can be acquired (their holder is gone);
/// - artifact dirs under the current tier whose `metadata.json` is missing
///   or fails to parse (quarantine-by-delete; they recompile from source).
///
/// Live leased artifacts are safe by construction: leasing requires a
/// parseable, matching `metadata.json`, and in-flight staging is lock-held.
fn sweep_cache_root(cache_root: &Path) {
    let mut swept_old_schema = 0usize;
    let mut swept_staging = 0usize;
    let mut swept_corrupt = 0usize;

    // Old schema-1 layout: dylibs/ + staging/ directly under the root.
    for legacy in ["dylibs", "staging"] {
        let dir = cache_root.join(legacy);
        if dir.is_dir() && std::fs::remove_dir_all(&dir).is_ok() {
            swept_old_schema += 1;
        }
    }

    // Orphaned staging dirs and stale lock files in the current tier. A
    // staging dir whose sibling lock is held belongs to a live compile in
    // some process sharing this root — never touch it (eseq-linux.51).
    if let Ok(entries) = std::fs::read_dir(staging_root(cache_root)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Missing lock file ⇒ no live owner (a live compile creates
                // and locks it before the dir exists and unlinks it only
                // after the dir is gone). Held lock ⇒ live; skip. Acquired
                // lock ⇒ the owner died; sweep the dir while holding it, so
                // the owner-side create-then-lock protocol cannot interleave.
                let Some(_lock) = try_acquire_orphan_lock(&staging_lock_path(&path)) else {
                    continue;
                };
                if std::fs::remove_dir_all(&path).is_ok() {
                    swept_staging += 1;
                }
            } else if path.extension().is_some_and(|ext| ext == "lock")
                && !path.with_extension("").exists()
            {
                // Lock file whose staging dir is gone: its holder died
                // between creating the lock and the dir. If acquired, the
                // guard's drop unlinks the file (while the lock is held); if
                // held, the owner is live (mid-allocation) — leave it.
                let _lock = try_acquire_orphan_lock(&path);
            }
        }
    }

    // Artifact dirs with missing/corrupt metadata in the current tier.
    if let Ok(key_entries) = std::fs::read_dir(dylibs_root(cache_root)) {
        for key_entry in key_entries.flatten() {
            let key_dir = key_entry.path();
            if !key_dir.is_dir() {
                continue;
            }
            let Ok(artifact_entries) = std::fs::read_dir(&key_dir) else {
                continue;
            };
            for artifact_entry in artifact_entries.flatten() {
                let artifact_dir = artifact_entry.path();
                if !artifact_dir.is_dir() {
                    continue;
                }
                let parses = std::fs::read_to_string(artifact_dir.join("metadata.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<CacheMetadata>(&text).ok())
                    .is_some();
                if !parses && std::fs::remove_dir_all(&artifact_dir).is_ok() {
                    swept_corrupt += 1;
                }
            }
        }
    }

    if swept_old_schema + swept_staging + swept_corrupt > 0 {
        eprintln!(
            "[dgenlisp cache] startup sweep at {}: removed {swept_old_schema} old-schema dir(s), \
             {swept_staging} orphaned staging dir(s), {swept_corrupt} corrupt artifact dir(s)",
            cache_root.display()
        );
    }
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
    let mut by_path = BTreeMap::<PathBuf, String>::new();
    for reference in references {
        let resolved = resolve_asset_reference(&reference, asset_base)?;
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
    String {
        value: String,
        /// Character offsets covering the complete quoted literal.
        start: usize,
        end: usize,
    },
}

fn asset_reference_tokens(source: &str) -> Result<Vec<(String, usize, usize)>, String> {
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
            let Token::String { value, start, end } = next else {
                return Err("DGenLisp asset keyword must be followed by a string path".to_string());
            };
            refs.push((value.clone(), *start, *end));
            idx += 2;
        } else {
            idx += 1;
        }
    }
    Ok(refs)
}

pub(crate) fn asset_references(source: &str) -> Result<Vec<String>, String> {
    let mut refs = asset_reference_tokens(source)?
        .into_iter()
        .map(|(reference, _, _)| reference)
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    Ok(refs)
}

/// Resolve a DGenLisp `@file` reference with exactly the same precedence used
/// by compilation and cache fingerprinting: patch-local, user library, then
/// factory library. Absolute references retain their existing pass-through
/// behavior, including letting DGenLisp report a missing absolute file.
pub fn resolve_asset_reference(
    reference: &str,
    asset_base: Option<&Path>,
) -> Result<PathBuf, String> {
    resolve_asset_reference_with_paths(reference, asset_base, crate::app_paths::app_paths())
}

fn resolve_asset_reference_with_paths(
    reference: &str,
    asset_base: Option<&Path>,
    paths: &crate::app_paths::AppPaths,
) -> Result<PathBuf, String> {
    let reference_path = Path::new(reference);
    if reference_path.is_absolute() {
        return Ok(reference_path.to_path_buf());
    }

    let base = asset_base
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.dgen_asset_fallback_base());
    let local = base.join(reference_path);
    if local.is_file() {
        return Ok(canonical_or_absolute(&local));
    }

    for root in [paths.user_assets_dir(), paths.factory_assets_dir()] {
        if let Some(candidate) = join_library_reference(&root, reference_path) {
            if candidate.is_file() {
                return Ok(canonical_or_absolute(&candidate));
            }
        }
    }

    Err(format!(
        "DGenLisp asset reference '{reference}' was not found; searched asset base {}, user assets {}, and factory assets {}",
        base.display(),
        paths.user_assets_dir().display(),
        paths.factory_assets_dir().display()
    ))
}

/// Rewrite only library-backed relative references. Patch-local and absolute
/// spellings remain byte-for-byte authored; the returned source is the
/// transient compiler input and is never persisted to `dsp.lisp`.
pub(crate) fn rewrite_library_asset_references(
    source: &str,
    asset_base: Option<&Path>,
) -> Result<String, String> {
    let paths = crate::app_paths::app_paths();
    rewrite_library_asset_references_with_paths(source, asset_base, paths)
}

fn rewrite_library_asset_references_with_paths(
    source: &str,
    asset_base: Option<&Path>,
    paths: &crate::app_paths::AppPaths,
) -> Result<String, String> {
    let base = asset_base
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.dgen_asset_fallback_base());
    let mut replacements = Vec::new();
    for (reference, start, end) in asset_reference_tokens(source)? {
        let reference_path = Path::new(&reference);
        if reference_path.is_absolute() || base.join(reference_path).is_file() {
            continue;
        }
        let resolved = resolve_asset_reference_with_paths(&reference, Some(&base), paths)?;
        replacements.push((start, end, lisp_string(&resolved.to_string_lossy())));
    }
    if replacements.is_empty() {
        return Ok(source.to_string());
    }

    let chars = source.chars().collect::<Vec<_>>();
    let mut rewritten = String::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end, replacement) in replacements {
        rewritten.extend(chars[cursor..start].iter());
        rewritten.push_str(&replacement);
        cursor = end;
    }
    rewritten.extend(chars[cursor..].iter());
    Ok(rewritten)
}

fn join_library_reference(root: &Path, reference: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in reference.components() {
        match component {
            std::path::Component::Normal(part) => relative.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !relative.pop() {
                    return None;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(root.join(relative))
}

fn lisp_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
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
                let start = idx;
                idx += 1;
                let mut value = String::new();
                let mut terminated = false;
                while idx < chars.len() {
                    match chars[idx] {
                        '"' => {
                            idx += 1;
                            terminated = true;
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
                if !terminated {
                    return Err("unterminated DGenLisp string".to_string());
                }
                tokens.push(Token::String { value, start, end: idx });
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

/// Copy the regular files of a committed artifact directory (dylib, .c,
/// manifest.json, metadata.json, source.lisp) into a staging dir. Artifact
/// dirs are flat; anything else is skipped. `std::fs::copy` creates new
/// files (new inodes even when APFS clones the blocks), which is what makes
/// the clone a distinct dyld image once loaded.
fn copy_artifact_files(from: &Path, to: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(from)
        .map_err(|e| format!("read artifact dir {}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read artifact dir {}: {e}", from.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        std::fs::copy(&path, to.join(name))
            .map_err(|e| format!("copy artifact file {}: {e}", path.display()))?;
    }
    Ok(())
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
            (def w (tensor-param @default-file "waves/b.json"))
            "#,
        )
        .expect("scan references");
        assert_eq!(refs, vec!["a.json".to_string(), "waves/b.json".to_string()]);
    }

    #[test]
    fn library_asset_resolution_and_rewrite_follow_shared_precedence() {
        let root = std::env::temp_dir().join(format!(
            "eseq-asset-resolution-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let workspace = root.join("workspace");
        let paths = crate::app_paths::AppPaths::dev(
            workspace.join("crates/sequencer"),
            workspace.clone(),
            root.join("config"),
        );
        let local = root.join("draft");
        let reference = Path::new("wavetables/shared.json");
        for directory in [
            local.join("wavetables"),
            paths.user_assets_dir().join("wavetables"),
            paths.factory_assets_dir().join("wavetables"),
        ] {
            std::fs::create_dir_all(directory).expect("create asset test directory");
        }
        let local_asset = local.join(reference);
        let user_asset = paths.user_assets_dir().join(reference);
        let factory_asset = paths.factory_assets_dir().join(reference);
        std::fs::write(&factory_asset, "factory").unwrap();
        std::fs::write(&user_asset, "user").unwrap();
        std::fs::write(&local_asset, "local").unwrap();

        assert_eq!(
            resolve_asset_reference_with_paths("wavetables/shared.json", Some(&local), &paths)
                .unwrap(),
            local_asset.canonicalize().unwrap()
        );
        let source = r#"(tensor @shape [1] @file "wavetables/shared.json")"#;
        assert_eq!(
            rewrite_library_asset_references_with_paths(source, Some(&local), &paths).unwrap(),
            source,
            "patch-local references retain their authored spelling"
        );
        std::fs::remove_file(&local_asset).unwrap();
        assert_eq!(
            resolve_asset_reference_with_paths("wavetables/shared.json", Some(&local), &paths)
                .unwrap(),
            user_asset.canonicalize().unwrap()
        );

        let rewritten =
            rewrite_library_asset_references_with_paths(source, Some(&local), &paths).unwrap();
        let canonical_user_asset = user_asset.canonicalize().unwrap();
        assert!(rewritten.contains(canonical_user_asset.to_string_lossy().as_ref()));
        assert!(!rewritten.contains("\"wavetables/shared.json\""));

        std::fs::remove_file(&user_asset).unwrap();
        assert_eq!(
            resolve_asset_reference_with_paths("wavetables/shared.json", Some(&local), &paths)
                .unwrap(),
            factory_asset.canonicalize().unwrap()
        );
        let error = resolve_asset_reference_with_paths("missing.json", Some(&local), &paths)
            .expect_err("missing reference must fail before compilation");
        for searched in [&local, &paths.user_assets_dir(), &paths.factory_assets_dir()] {
            assert!(error.contains(&searched.display().to_string()), "{error}");
        }
        assert!(resolve_asset_reference_with_paths("../escape.json", Some(&local), &paths)
            .is_err());
        let _ = std::fs::remove_dir_all(root);
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
            let lease = manager
                .try_lease_artifact(&artifact)
                .expect("lease")
                .expect("artifact free");
            assert_eq!(manager.live_lease_count(&artifact), 1);
            drop(lease);
        }
        assert_eq!(manager.live_lease_count(&artifact), 0);
    }

    /// eseq-599: leases are exclusive — a live artifact refuses a second
    /// lease (the acquirer clones instead), and release frees it for reuse.
    #[test]
    fn second_lease_on_live_artifact_is_refused() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-exclusive-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let manager = DylibCacheManager::new(root);
        let artifact = PathBuf::from("/tmp/exclusive-lease-test");
        let first = manager
            .try_lease_artifact(&artifact)
            .expect("lease")
            .expect("artifact free");
        assert!(manager
            .try_lease_artifact(&artifact)
            .expect("lease")
            .is_none());
        drop(first);
        assert!(manager
            .try_lease_artifact(&artifact)
            .expect("lease")
            .is_some());
    }

    #[test]
    fn asset_fingerprint_changes_cache_key() {
        if !staged_toolchain_present() {
            return;
        }
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
    fn editing_user_library_asset_compiles_a_new_cache_entry() {
        let unique = format!("cache-test-{}-{}", process_id(), now_unix_ms());
        let library_dir = crate::app_paths::app_paths()
            .user_assets_dir()
            .join("__tests")
            .join(&unique);
        std::fs::create_dir_all(&library_dir).expect("create user asset test directory");
        let asset = library_dir.join("wave.json");
        std::fs::write(&asset, "[0.0]").expect("write first library asset");
        let reference = format!("__tests/{unique}/wave.json");
        let source = format!(
            "(def t (tensor @shape [1] @file \"{reference}\"))\n(out (peek t 0) 1 @name out)"
        );
        let cache_root = std::env::temp_dir().join(format!("eseq-library-cache-{unique}"));
        let empty_asset_base = std::env::temp_dir().join(format!("eseq-library-base-{unique}"));
        std::fs::create_dir_all(&empty_asset_base).unwrap();
        let manager = DylibCacheManager::new(cache_root.clone());

        let first = manager
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Draft,
                &source,
                44_100,
                Some(&empty_asset_base),
            )
            .expect("compile with first library asset contents");
        let first_key_dir = first
            .lease
            .as_ref()
            .unwrap()
            .artifact_dir()
            .parent()
            .unwrap()
            .to_path_buf();
        drop(first);

        std::fs::write(&asset, "[1.0]").expect("edit library asset");
        let second = manager
            .acquire(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Draft,
                &source,
                44_100,
                Some(&empty_asset_base),
            )
            .expect("recompile after library asset edit");
        let second_key_dir = second
            .lease
            .as_ref()
            .unwrap()
            .artifact_dir()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_ne!(
            first_key_dir, second_key_dir,
            "asset edit must change the cache key"
        );
        drop(second);

        let _ = std::fs::remove_dir_all(cache_root);
        let _ = std::fs::remove_dir_all(empty_asset_base);
        let _ = std::fs::remove_dir_all(library_dir);
    }

    /// eseq-599 regression: two simultaneous acquires of identical source
    /// must yield *independent dyld images* (distinct artifact dirs, distinct
    /// dylib paths, distinct `dgen_process_v1` addresses), because a shared
    /// image aliases the generated code's mutable file-scope scratch statics
    /// across instances. Repeated `dlopen` of one path silently returns one
    /// image — this test is the explicit loader-identity validation that the
    /// clone path produces a genuinely separate mapping. Releasing both
    /// leases then lets a third acquire reuse a free artifact instead of
    /// growing the cache.
    #[test]
    fn simultaneous_acquires_get_independent_dylib_images() {
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
        let acquire = |label: &str| {
            manager
                .acquire(
                    DGenCompileKind::Effect,
                    DGenSourceOrigin::Custom,
                    source,
                    44_100,
                    None,
                )
                .unwrap_or_else(|e| panic!("{label} acquire: {e}"))
        };
        let artifact_count = |key_dir: &Path| {
            std::fs::read_dir(key_dir)
                .expect("read key dir")
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .count()
        };

        let first = acquire("first");
        let first_dir = first
            .lease
            .as_ref()
            .expect("first lease")
            .artifact_dir()
            .to_path_buf();
        assert_eq!(manager.live_lease_count(&first_dir), 1);

        let second = acquire("second");
        let second_dir = second
            .lease
            .as_ref()
            .expect("second lease")
            .artifact_dir()
            .to_path_buf();
        assert_ne!(
            second_dir, first_dir,
            "a live artifact must never be shared"
        );
        assert_eq!(manager.live_lease_count(&first_dir), 1);
        assert_eq!(manager.live_lease_count(&second_dir), 1);
        assert_ne!(
            first.manifest.dylib_path, second.manifest.dylib_path,
            "instances must load distinct dylib files"
        );
        assert_ne!(
            first.lib.process_fn as usize, second.lib.process_fn as usize,
            "instances must resolve process symbols in distinct dyld images"
        );

        let key_dir = first_dir.parent().expect("key dir").to_path_buf();
        assert_eq!(second_dir.parent().expect("key dir"), key_dir);
        assert_eq!(artifact_count(&key_dir), 2, "compile + one clone");

        // Release both, then recreate: a free artifact is reused, so the
        // cache does not grow past the simultaneous-instance high-water mark.
        drop(first);
        drop(second);
        assert_eq!(manager.live_lease_count(&first_dir), 0);
        assert_eq!(manager.live_lease_count(&second_dir), 0);
        let third = acquire("third");
        let third_dir = third
            .lease
            .as_ref()
            .expect("third lease")
            .artifact_dir()
            .to_path_buf();
        assert!(
            third_dir == first_dir || third_dir == second_dir,
            "released artifacts must be reused"
        );
        assert_eq!(artifact_count(&key_dir), 2, "no growth on reuse");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Slice E6 exit criterion, updated for eseq-599: two threads racing one
    /// cache key still run exactly one *compilation* (the loser clones the
    /// winner's artifact instead of compiling), but each ends up with its own
    /// artifact directory and lease.
    #[test]
    fn racing_threads_on_one_key_share_one_compilation() {
        if !dgenlisp_tool_path().exists() {
            eprintln!(
                "skipping: DGenLisp tool not found at {:?}",
                dgenlisp_tool_path()
            );
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-race-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let manager = DylibCacheManager::new(root.clone());
        let source = r#"
            (def input_l (in 1 @name Left))
            (out input_l 1 @name Left)
        "#;

        let results = std::thread::scope(|scope| {
            let handles = [0, 1].map(|_| {
                let manager = manager.clone();
                scope.spawn(move || {
                    manager.acquire(
                        DGenCompileKind::Effect,
                        DGenSourceOrigin::Custom,
                        source,
                        44_100,
                        None,
                    )
                })
            });
            handles.map(|handle| handle.join().expect("racing thread panicked"))
        });

        let dirs = results
            .iter()
            .map(|result| {
                result
                    .as_ref()
                    .expect("racing acquire failed")
                    .lease
                    .as_ref()
                    .expect("racing lease")
                    .artifact_dir()
                    .to_path_buf()
            })
            .collect::<Vec<_>>();
        assert_ne!(dirs[0], dirs[1], "racers must get independent artifacts");
        assert_eq!(manager.live_lease_count(&dirs[0]), 1);
        assert_eq!(manager.live_lease_count(&dirs[1]), 1);

        let key_dir = dirs[0].parent().expect("key dir");
        assert_eq!(dirs[1].parent().expect("key dir"), key_dir);
        let artifact_count = std::fs::read_dir(key_dir)
            .expect("read key dir")
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .count();
        assert_eq!(
            artifact_count, 2,
            "one compiled artifact plus one clone, no duplicate compile"
        );

        drop(results);
        assert_eq!(manager.live_lease_count(&dirs[0]), 0);
        assert_eq!(manager.live_lease_count(&dirs[1]), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    /// eseq-599 acceptance: two live instances built from identical source
    /// process with fully independent state. Each thread renders the same
    /// deterministic probe through its own instance *concurrently*; with a
    /// shared dyld image the generated code's file-scope scratch arrays are
    /// written by both threads at once and the outputs diverge from the
    /// sequential golden render. With independent images the outputs must
    /// match it exactly.
    #[test]
    fn concurrent_instances_render_independently() {
        if !dgenlisp_tool_path().exists() {
            eprintln!(
                "skipping: DGenLisp tool not found at {:?}",
                dgenlisp_tool_path()
            );
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-isolation-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let manager = DylibCacheManager::new(root.clone());
        // Several chained ops so the generated C uses multiple scratch
        // arrays; memoryless, so the render is a pure function of the probe.
        let source = r#"
            (def input_l (in 1 @name Left))
            (def input_r (in 2 @name Right))
            (def blend (+ (* input_l 0.9) (* input_r 0.1)))
            (def shaped (* blend blend))
            (out (+ blend (* shaped 0.05)) 1 @name Left)
            (out (* blend 0.8) 2 @name Right)
        "#;
        let acquire = || {
            manager
                .acquire(
                    DGenCompileKind::Effect,
                    DGenSourceOrigin::Custom,
                    source,
                    48_000,
                    None,
                )
                .expect("acquire isolation-test effect")
        };
        let options = crate::lisp_host::EffectRenderOptions {
            sample_rate: 48_000,
            block_size: 256,
            frames: 48_000,
            param_overrides: Vec::new(),
            param_events: Vec::new(),
            tensor_overrides: Vec::new(),
            input_overrides: Vec::new(),
            input_tones: Vec::new(),
        };
        let render = |result: &CompileResult| {
            crate::lisp_host::render_loaded_effect_for_test(
                &result.manifest,
                &result.lib,
                &options,
            )
            .expect("render isolation-test effect")
            .samples
        };

        let golden = {
            let solo = acquire();
            render(&solo)
        };

        let first = acquire();
        let second = acquire();
        assert_ne!(
            first.lease.as_ref().expect("first lease").artifact_dir(),
            second.lease.as_ref().expect("second lease").artifact_dir(),
        );
        let barrier = std::sync::Barrier::new(2);
        let (out_a, out_b) = std::thread::scope(|scope| {
            let handle_a = scope.spawn(|| {
                barrier.wait();
                render(&first)
            });
            let handle_b = scope.spawn(|| {
                barrier.wait();
                render(&second)
            });
            (
                handle_a.join().expect("render thread a"),
                handle_b.join().expect("render thread b"),
            )
        });

        assert_eq!(out_a, golden, "instance A corrupted by instance B");
        assert_eq!(out_b, golden, "instance B corrupted by instance A");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn toolchain_version_json_content_changes_cache_key() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-toolchain-key-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let stage_a = root.join("stage-a");
        let stage_b = root.join("stage-b");
        std::fs::create_dir_all(&stage_a).expect("create stage a");
        std::fs::create_dir_all(&stage_b).expect("create stage b");
        std::fs::write(stage_a.join("VERSION.json"), r#"{"dgen_abi_version":1}"#)
            .expect("write VERSION.json a");
        std::fs::write(stage_b.join("VERSION.json"), r#"{"dgen_abi_version":2}"#)
            .expect("write VERSION.json b");

        let toolchain_a = fingerprint_toolchain(&stage_a).expect("fingerprint a");
        let toolchain_b = fingerprint_toolchain(&stage_b).expect("fingerprint b");
        assert_ne!(
            toolchain_a.policy_sha256,
            toolchain_b.policy_sha256
        );

        let tool = ToolFingerprint {
            path: "/tools/DGenLisp-target-a".to_string(),
            exists: true,
            len: Some(1),
            modified_unix_ms: Some(1),
            sha256: Some("aa".to_string()),
        };
        let key = |toolchain: &ToolchainFingerprint| {
            cache_key(
                DGenCompileKind::Effect,
                44_100,
                None,
                "source-sha",
                &tool,
                toolchain,
                &[],
            )
            .expect("cache key")
        };
        assert_ne!(key(&toolchain_a), key(&toolchain_b));

        #[cfg(target_os = "macos")]
        {
            // The staged macOS policy is incomplete without VERSION.json.
            let err = fingerprint_toolchain(&root.join("missing-stage"))
                .expect_err("missing VERSION.json must fail");
            assert!(err.contains("VERSION.json"), "{err}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn key_material_carries_toolchain_identity_fields() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-key-material-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&root).expect("create stage");
        std::fs::write(root.join("VERSION.json"), r#"{"dgen_abi_version":1}"#)
            .expect("write VERSION.json");
        let toolchain = fingerprint_toolchain(&root).expect("fingerprint");
        assert_eq!(toolchain.abi_header_sha256.len(), 64);
        assert_eq!(toolchain.policy_sha256.len(), 64);
        assert_eq!(toolchain.target_triple, CACHE_TARGET_TRIPLE);
        assert_eq!(
            toolchain.deployment_target,
            CACHE_DEPLOYMENT_TARGET.map(str::to_string)
        );

        let tool = ToolFingerprint {
            path: "/tools/DGenLisp-target-a".to_string(),
            exists: true,
            len: Some(1),
            modified_unix_ms: Some(1),
            sha256: Some("aa".to_string()),
        };
        let material = cache_key_material(
            DGenCompileKind::Effect,
            44_100,
            None,
            "source-sha",
            &tool,
            &toolchain,
            &[],
        );
        let mut other_target_tool = tool.clone();
        other_target_tool.path = "/tools/DGenLisp-target-b".to_string();
        assert_ne!(
            cache_key(
                DGenCompileKind::Effect,
                44_100,
                None,
                "source-sha",
                &tool,
                &toolchain,
                &[],
            ).unwrap(),
            cache_key(
                DGenCompileKind::Effect,
                44_100,
                None,
                "source-sha",
                &other_target_tool,
                &toolchain,
                &[],
            ).unwrap(),
            "target-specific compiler paths must not collide in the cache"
        );
        assert_eq!(material["schemaVersion"], CACHE_SCHEMA_VERSION);
        let toolchain_value = &material["toolchain"];
        assert_eq!(
            toolchain_value["abi_header_sha256"],
            serde_json::json!(abi_header_sha256())
        );
        assert_eq!(
            toolchain_value["target_triple"],
            serde_json::json!(CACHE_TARGET_TRIPLE)
        );
        assert_eq!(
            toolchain_value["deployment_target"],
            serde_json::json!(CACHE_DEPLOYMENT_TARGET)
        );
        assert_eq!(
            toolchain_value["policy_sha256"],
            serde_json::json!(toolchain.policy_sha256)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cache_key_is_stable_across_manager_instances() {
        if !staged_toolchain_present() {
            return;
        }
        let source = r#"
            (def input_l (in 1 @name Left))
            (out input_l 1 @name Left)
        "#;
        let build = || {
            build_request(
                DGenCompileKind::Effect,
                DGenSourceOrigin::Custom,
                source,
                44_100,
                None,
            )
            .expect("build request")
        };
        // The key must derive only from content fingerprints — no manager
        // state, cache-root path, or timestamps.
        let _manager_a = DylibCacheManager::new(std::env::temp_dir().join(format!(
            "eseq-dylib-cache-stable-a-{}-{}",
            process_id(),
            now_unix_ms()
        )));
        let first = build();
        let _manager_b = DylibCacheManager::new(std::env::temp_dir().join(format!(
            "eseq-dylib-cache-stable-b-{}-{}",
            process_id(),
            now_unix_ms()
        )));
        let second = build();
        assert_eq!(first.key, second.key);
    }

    #[test]
    fn startup_sweep_clears_orphans_and_old_schema_but_keeps_valid_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-sweep-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));

        // Old schema-1 layout directly under the root.
        std::fs::create_dir_all(root.join("dylibs/oldkey/artifact")).expect("old dylibs");
        std::fs::create_dir_all(root.join("staging/old.tmp")).expect("old staging");
        // Sibling dir owned by another subsystem (conv-reverb IR cache).
        std::fs::create_dir_all(root.join("ir-prep")).expect("ir-prep");

        // Current tier: an orphaned staging dir from a dead process, plus a
        // corrupt-metadata artifact and a valid one.
        let staging = staging_root(&root);
        std::fs::create_dir_all(staging.join("999999999-1-0.tmp")).expect("orphan staging");
        let corrupt = dylibs_root(&root).join("key-a").join("artifact-corrupt");
        std::fs::create_dir_all(&corrupt).expect("corrupt artifact");
        std::fs::write(corrupt.join("metadata.json"), "not json").expect("corrupt metadata");
        let missing_meta = dylibs_root(&root).join("key-a").join("artifact-missing");
        std::fs::create_dir_all(&missing_meta).expect("missing-meta artifact");

        let valid = dylibs_root(&root).join("key-b").join("artifact-valid");
        std::fs::create_dir_all(&valid).expect("valid artifact");
        let metadata = CacheMetadata {
            schema_version: CACHE_SCHEMA_VERSION,
            key: "key-b".to_string(),
            kind: DGenCompileKind::Effect,
            origin: DGenSourceOrigin::Custom,
            sample_rate: 44_100,
            voices: None,
            effective_source_sha256: "aa".to_string(),
            tool: ToolFingerprint {
                path: "/tools/DGenLisp-target-a".to_string(),
                exists: true,
                len: Some(1),
                modified_unix_ms: Some(1),
                sha256: Some("aa".to_string()),
            },
            toolchain: ToolchainFingerprint {
                policy_sha256: "bb".to_string(),
                abi_header_sha256: "cc".to_string(),
                target_triple: CACHE_TARGET_TRIPLE.to_string(),
                deployment_target: CACHE_DEPLOYMENT_TARGET.map(str::to_string),
            },
            assets: Vec::new(),
            dylib_name: "dgen_effect_valid".to_string(),
        };
        std::fs::write(
            valid.join("metadata.json"),
            serde_json::to_string_pretty(&metadata).expect("serialize metadata"),
        )
        .expect("valid metadata");

        let _manager = DylibCacheManager::new(root.clone());

        assert!(!root.join("dylibs").exists(), "old-schema dylibs swept");
        assert!(!root.join("staging").exists(), "old-schema staging swept");
        assert!(root.join("ir-prep").exists(), "sibling dirs untouched");
        assert!(
            !staging.join("999999999-1-0.tmp").exists(),
            "orphan staging swept"
        );
        assert!(!corrupt.exists(), "corrupt-metadata artifact swept");
        assert!(!missing_meta.exists(), "missing-metadata artifact swept");
        assert!(valid.exists(), "valid artifact kept");
        let _ = std::fs::remove_dir_all(root);
    }

    /// eseq-linux.51: the sweep must not delete another manager's in-flight
    /// staging dir. Liveness is an advisory flock on the sibling `.lock`
    /// file, so a held lock protects the dir even though the sweeping
    /// "process" (here: this process, via an independent fd — flock treats
    /// separate opens identically across and within processes) cannot match
    /// it by pid. Once the lock is released the dir is an orphan and goes.
    #[test]
    fn startup_sweep_honors_staging_locks() {
        let root = std::env::temp_dir().join(format!(
            "eseq-dylib-cache-staging-lock-test-{}-{}",
            process_id(),
            now_unix_ms()
        ));
        let staging = staging_root(&root);
        std::fs::create_dir_all(&staging).expect("create staging root");

        // In-flight: lock acquired before the dir exists, as in
        // allocate_artifact_dirs.
        let in_flight = staging.join("12345-1-0.tmp");
        let lock = StagingLock::acquire(&in_flight).expect("acquire staging lock");
        std::fs::create_dir_all(&in_flight).expect("create in-flight staging");

        // Orphans: a dir whose lock file exists but is unheld (owner died),
        // a dir with no lock file at all (pre-lock-protocol leftover), and a
        // stale lock file whose dir is gone.
        let dead = staging.join("23456-1-0.tmp");
        std::fs::create_dir_all(&dead).expect("create dead staging");
        drop(StagingLock::acquire(&dead).expect("create dead lock file, then release"));
        // Recreate the lock file unheld: acquire's drop unlinks it.
        std::fs::write(staging_lock_path(&dead), "").expect("recreate unheld lock");
        let unlocked = staging.join("34567-1-0.tmp");
        std::fs::create_dir_all(&unlocked).expect("create lockless staging");
        let stale_lock = staging.join("45678-1-0.tmp.lock");
        std::fs::write(&stale_lock, "").expect("create stale lock");

        sweep_cache_root(&root);

        assert!(in_flight.exists(), "lock-held staging dir survives sweep");
        assert!(
            staging_lock_path(&in_flight).exists(),
            "held lock file survives sweep"
        );
        assert!(!dead.exists(), "unheld-lock staging dir swept");
        assert!(
            !staging_lock_path(&dead).exists(),
            "unheld lock file swept with its dir"
        );
        assert!(!unlocked.exists(), "lockless staging dir swept");
        assert!(!stale_lock.exists(), "stale lock file without a dir swept");

        drop(lock);
        assert!(
            !staging_lock_path(&in_flight).exists(),
            "released lock unlinks its file"
        );
        sweep_cache_root(&root);
        assert!(!in_flight.exists(), "released staging dir is an orphan");
        let _ = std::fs::remove_dir_all(root);
    }

    fn staged_toolchain_present() -> bool {
        #[cfg(target_os = "macos")]
        {
            let root = crate::app_paths::app_paths().dgen_toolchain_root();
            if root.join("VERSION.json").is_file() {
                return true;
            }
            eprintln!("skipping: staged toolchain not found at {root:?}");
            false
        }
        #[cfg(target_os = "linux")]
        {
            let paths = crate::app_paths::app_paths();
            let present = paths.dgenlisp_tool().is_file()
                && paths.dgen_abi_dir().join("exports-v1-elf.txt").is_file()
                && paths
                    .dgen_abi_dir()
                    .join("libsystem-symbols-v1-elf.txt")
                    .is_file();
            if !present {
                eprintln!("skipping: fetched Linux DGenLisp distribution is incomplete");
            }
            present
        }
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
    #[ignore = "eseq-4tl: chdirs the whole process; run alone with -- --ignored"]
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
