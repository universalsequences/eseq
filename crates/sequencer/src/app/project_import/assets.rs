//! Shared samples for controlled import (docs/rig-spec.md §8, "Assets are
//! shared"): a sample an import decodes is kept, so the next import of the
//! same kit reuses the graph buffer instead of decoding it again.
//!
//! Sharing is by FILE: one canonical path, unchanged on disk (same length
//! and modification time). Two different paths with the same content keep
//! separate buffers, because the App remembers one path per buffer
//! (`sample_buffer_path_registry`) and saving and slice edits go through it.
//! Graph sample buffers are immutable and never freed, so a cached buffer
//! stays valid; the cache is still cleared whenever the project is replaced
//! (`App::start_new_project`, the load's clear phase).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::super::{App, ProjectSampleAsset};

/// A file's identity on disk: its length and modification time.
type FileStamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// The samples imports have decoded, by canonical path.
#[derive(Default)]
pub(crate) struct ImportSampleCache {
    /// While an import runs: Sound loads and lane conversions go through the
    /// cache. Outside an import every load decodes as it always did.
    active: bool,
    /// The decoded assets, in the map shape the pattern loader shares.
    assets: HashMap<PathBuf, ProjectSampleAsset>,
    /// The stamp each asset was decoded at.
    stamps: HashMap<PathBuf, FileStamp>,
    /// Decodes this cache performed (a test reads it: the same kit imported
    /// twice decodes each sample once).
    decodes: usize,
}

impl ImportSampleCache {
    #[cfg(test)]
    pub(crate) fn decodes(&self) -> usize {
        self.decodes
    }

    /// Forget every sample (the project is being replaced).
    pub(crate) fn clear(&mut self) {
        self.assets.clear();
        self.stamps.clear();
    }

    /// Record what a decode produced.
    fn remember(&mut self, path: PathBuf, asset: ProjectSampleAsset) {
        if let Some(stamp) = stamp(&path) {
            self.decodes += 1;
            self.stamps.insert(path.clone(), stamp);
            self.assets.insert(path, asset);
        }
    }
}

impl App {
    /// Route sample loads through the import cache until
    /// [`App::end_import_sample_sharing`]. Entries whose file changed on
    /// disk since they were decoded are dropped here, once per import.
    pub(super) fn begin_import_sample_sharing(&mut self) {
        let cache = &mut self.import_samples;
        let stale: Vec<PathBuf> = cache
            .stamps
            .iter()
            .filter(|(path, recorded)| stamp(path).as_ref() != Some(*recorded))
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale {
            cache.stamps.remove(&path);
            cache.assets.remove(&path);
        }
        cache.active = true;
    }

    pub(super) fn end_import_sample_sharing(&mut self) {
        self.import_samples.active = false;
    }

    /// The canonical-path map a pattern conversion
    /// (`project_pattern_into_snapshot_with_policy`) decodes through: the
    /// cache itself while an import runs (moved out, not copied), empty
    /// otherwise. Hand it back with [`App::return_import_sample_assets`].
    pub(in crate::app) fn take_import_sample_assets(&mut self) -> HashMap<PathBuf, ProjectSampleAsset> {
        if self.import_samples.active {
            std::mem::take(&mut self.import_samples.assets)
        } else {
            HashMap::new()
        }
    }

    /// Keep what a conversion decoded (see `take_import_sample_assets`).
    pub(in crate::app) fn return_import_sample_assets(
        &mut self,
        assets: HashMap<PathBuf, ProjectSampleAsset>,
    ) {
        let cache = &mut self.import_samples;
        if !cache.active {
            return;
        }
        for (path, asset) in assets {
            if cache.stamps.contains_key(&path) {
                cache.assets.insert(path, asset);
            } else {
                cache.remember(path, asset);
            }
        }
    }

    /// Decode the WAV a Sound's sampler slot names, or, while an import
    /// runs, reuse the buffer an earlier import decoded from the same file.
    pub(in crate::app) fn load_sound_sample_asset(
        &mut self,
        sample_path: &Path,
    ) -> Result<ProjectSampleAsset, String> {
        let canonical = self
            .import_samples
            .active
            .then(|| crate::app_paths::resolve_sample_ref(sample_path).canonicalize().ok())
            .flatten();
        if let Some(asset) = canonical.as_ref().and_then(|path| self.import_samples.assets.get(path)) {
            return Ok(asset.clone());
        }
        let loaded = crate::instruments::sampler::load_wav_buffer(self.graph.lg.0, sample_path)?;
        self.submit_sample_analysis(&loaded);
        let asset = ProjectSampleAsset {
            buffer_id: loaded.buffer_id,
            sample_rate: loaded.sample_rate,
            decoded_name: loaded.name,
        };
        if let Some(canonical) = canonical {
            self.import_samples.remember(canonical, asset.clone());
        }
        Ok(asset)
    }
}
