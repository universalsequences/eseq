//! Cache validated display names without retaining deserialized sound data.
//! Exact file bytes keep external edits, replacements and renames visible;
//! correctness does not depend on file timestamps or explicit invalidation.

use super::{ProjectSoundPreset, ProjectTrackKind};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct RackPresetCatalog {
    entries: HashMap<PathBuf, (Vec<u8>, Option<String>)>,
}

impl RackPresetCatalog {
    pub(super) fn names(&mut self, directories: &[PathBuf]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for directory in directories {
            let Ok(entries) = std::fs::read_dir(directory) else { continue };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("rackpreset") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let read_name = || {
                    serde_json::from_slice::<ProjectSoundPreset>(&bytes).ok()
                        .filter(|preset| matches!(preset.track.kind, ProjectTrackKind::Rack { .. }))
                        .and_then(|preset| {
                            let name = preset.metadata.name.trim();
                            if name.is_empty() {
                                path.file_stem().and_then(|stem| stem.to_str()).map(str::to_owned)
                            } else { Some(name.to_owned()) }
                        })
                };
                let cached = self.entries.entry(path.clone()).or_insert_with(|| (bytes.clone(), read_name()));
                if cached.0 != bytes {
                    let name = read_name();
                    *cached = (bytes, name);
                }
                if let Some(name) = &cached.1 {
                    names.push(name.clone());
                }
                seen.insert(path);
            }
        }
        self.entries.retain(|path, _| seen.contains(path));
        names.sort();
        names.dedup();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_observes_content_changes_additions_deletions_and_invalid_presets() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [dir.path().to_owned()];
        let path = dir.path().join("fallback.rackpreset");
        let mut data = serde_json::json!({
            "version": 13, "metadata": {"name": "Alpha", "tags": [], "author": ""},
            "track": {"kind": "rack"}, "rack": {"slots": []}
        });
        let write = |path: &std::path::Path, data: &serde_json::Value| {
            std::fs::write(path, serde_json::to_vec(data).unwrap()).unwrap();
        };
        write(&path, &data);
        let mut catalog = RackPresetCatalog::default();
        assert_eq!(catalog.names(&roots), ["Alpha"]);
        assert_eq!(catalog.names(&roots), ["Alpha"]);
        data["metadata"]["name"] = "Bravo".into(); // same byte length
        write(&path, &data);
        assert_eq!(catalog.names(&roots), ["Bravo"]);
        data["metadata"]["name"] = " ".into();
        write(&path, &data);
        assert_eq!(catalog.names(&roots), ["fallback"]);
        let second = dir.path().join("new.rackpreset");
        write(&second, &data);
        assert_eq!(catalog.names(&roots), ["fallback", "new"]);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(catalog.names(&roots), ["new"]);
        data["track"]["kind"] = "empty".into();
        write(&second, &data);
        assert!(catalog.names(&roots).is_empty());
        std::fs::write(&second, b"not json").unwrap();
        assert!(catalog.names(&roots).is_empty());
        data["track"]["kind"] = "rack".into();
        write(&second, &data);
        assert_eq!(catalog.names(&roots), ["new"]);
    }
}
