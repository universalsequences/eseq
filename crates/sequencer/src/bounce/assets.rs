//! Job-owned copies of the exact assets used by a loaded DSP compile.

use super::*;
use crate::lisp_host::dylib_cache::{self, CompiledAsset};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;

pub(crate) struct FrozenDgenSource {
    pub source: String,
    storage: tempfile::TempDir,
}

impl FrozenDgenSource {
    pub(crate) fn capture(
        source: &str,
        asset_base: Option<&Path>,
        lease: &crate::lisp_host::DylibLease,
        cancel: &BounceCancellation,
    ) -> io::Result<Self> {
        let assets = lease.compiled_assets().map_err(io::Error::other)?;
        Self::capture_assets(source, asset_base, &assets, cancel)
    }

    fn capture_assets(
        source: &str,
        asset_base: Option<&Path>,
        assets: &[CompiledAsset],
        cancel: &BounceCancellation,
    ) -> io::Result<Self> {
        cancel.check()?;
        let storage = tempfile::Builder::new().prefix("eseq-bounce-assets-").tempdir()?;
        let mut captured = HashMap::new();
        let mut buffer = [0_u8; BUFFER_BYTES];
        for (index, asset) in assets.iter().enumerate() {
            cancel.check()?;
            let path = std::fs::canonicalize(&asset.path)?;
            let mut name = std::ffi::OsString::from(index.to_string());
            if let Some(extension) = path.extension() {
                name.push(".");
                name.push(extension);
            }
            let target = storage.path().join(name);
            let mut input = File::open(&path)?;
            let mut output = File::create(&target)?;
            let mut hash = Sha256::new();
            loop {
                cancel.check()?;
                let count = input.read(&mut buffer)?;
                if count == 0 { break; }
                hash.update(&buffer[..count]);
                output.write_all(&buffer[..count])?;
            }
            output.sync_all()?;
            let actual = format!("{:x}", hash.finalize());
            if actual != asset.sha256 {
                return Err(io::Error::other(format!(
                    "Asset changed since the instrument/effect was compiled: {}. Reload it before exporting.",
                    path.display(),
                )));
            }
            captured.insert(path, target);
        }
        let source = dylib_cache::rewrite_asset_references(source, |reference| {
            let resolved = dylib_cache::resolve_asset_reference(reference, asset_base)?;
            let path = std::fs::canonicalize(&resolved)
                .map_err(|error| format!("resolve export asset {}: {error}", resolved.display()))?;
            captured.get(&path).cloned().map(Some).ok_or_else(|| format!(
                "Asset {} does not match the loaded compile's asset inventory", path.display(),
            ))
        }).map_err(io::Error::other)?;
        cancel.check()?;
        Ok(Self { source, storage })
    }

    pub(crate) fn asset_base(&self) -> &Path { self.storage.path() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_compile_can_be_rebuilt_after_original_asset_is_removed() {
        use dylib_cache::{DylibCacheManager, DGenCompileKind, DGenSourceOrigin};
        let original = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let manager = DylibCacheManager::new(cache.path().to_path_buf());
        let path = original.path().join("wave.json");
        std::fs::write(&path, "[0.25]").unwrap();
        let source = "(def t (tensor @shape [1] @file \"wave.json\"))\n(out (peek t 0) 1 @name out)";
        let loaded = manager.acquire(DGenCompileKind::Effect, DGenSourceOrigin::Draft,
            source, 48_000, Some(original.path())).unwrap();
        let frozen = FrozenDgenSource::capture(source, Some(original.path()),
            loaded.lease.as_ref().unwrap(), &BounceCancellation::default()).unwrap();
        std::fs::remove_file(path).unwrap();
        let rebuilt = manager.acquire(DGenCompileKind::Effect, DGenSourceOrigin::Draft,
            &frozen.source, 48_000, Some(frozen.asset_base())).unwrap();
        assert_eq!(rebuilt.manifest.process_abi, loaded.manifest.process_abi);
        let assets = rebuilt.lease.as_ref().unwrap().compiled_assets().unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(std::fs::read(&assets[0].path).unwrap(), b"[0.25]");
        drop(rebuilt);
        drop(loaded);
    }

    #[test]
    fn frozen_source_preserves_loaded_bytes_and_rewrites_only_asset_literals() {
        let original = tempfile::tempdir().unwrap();
        let path = original.path().join("wave.json");
        std::fs::write(&path, b"[0, 1, -1]").unwrap();
        let assets = vec![CompiledAsset {
            path: path.clone(), sha256: format!("{:x}", Sha256::digest(b"[0, 1, -1]")),
        }];
        let source = "; café @file \"wave.json\"\n(def label \"wave.json\")\n(tensor @file \"wave.json\")\n(tensor-param @default-file \"./wave.json\")";
        let frozen = FrozenDgenSource::capture_assets(source, Some(original.path()), &assets,
            &BounceCancellation::default()).unwrap();
        assert!(frozen.source.starts_with("; café @file \"wave.json\"\n(def label \"wave.json\")"));
        let refs = dylib_cache::asset_references(&frozen.source).unwrap();
        assert_eq!(refs.len(), 1, "aliases share one frozen file");
        assert_eq!(std::fs::read(&refs[0]).unwrap(), b"[0, 1, -1]");
        assert!(Path::new(&refs[0]).starts_with(frozen.asset_base()));
        std::fs::write(&path, b"[9]").unwrap();
        assert_eq!(std::fs::read(&refs[0]).unwrap(), b"[0, 1, -1]");
        let storage = frozen.asset_base().to_path_buf();
        drop(frozen);
        assert!(!storage.exists());
        let error = FrozenDgenSource::capture_assets(source, Some(original.path()), &assets,
            &BounceCancellation::default()).err().unwrap();
        assert!(error.to_string().contains("changed since"));
    }

    #[test]
    fn missing_inventory_and_cancelled_capture_fail() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("wave.json"), b"[]").unwrap();
        let error = FrozenDgenSource::capture_assets("(tensor @file \"wave.json\")",
            Some(root.path()), &[], &BounceCancellation::default()).err().unwrap();
        assert!(error.to_string().contains("inventory"));
        let cancel = BounceCancellation::default();
        cancel.cancel();
        assert_eq!(FrozenDgenSource::capture_assets("", None, &[], &cancel)
            .err().unwrap().kind(), io::ErrorKind::Interrupted);
    }
}
