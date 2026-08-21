use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::sample_db::SampleDb;
use crate::sample_import::{decode_audio_file, transcode_to_store};
use crate::sample_manifest::{
    read_manifest, verify_payload, verify_source_asset, SampleManifestLine,
};

#[derive(Debug, Deserialize)]
struct PackageIdentity {
    name: String,
}

pub fn ingest_installed_packages(
    packages_dir: &Path,
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<Vec<String>, String> {
    let mut packages = fs::read_dir(packages_dir)
        .map_err(|error| format!("failed to scan {}: {error}", packages_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir() && path.join("samples.jsonl").is_file())
        .collect::<Vec<_>>();
    packages.sort();
    packages
        .iter()
        .map(|package| ingest_package_samples(package, sample_dir, db))
        .collect()
}

pub fn ingest_package_samples(
    package_dir: &Path,
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<String, String> {
    let identity_path = package_dir.join("manifest.json");
    let identity: PackageIdentity = serde_json::from_slice(
        &fs::read(&identity_path)
            .map_err(|error| format!("failed to read {}: {error}", identity_path.display()))?,
    )
    .map_err(|error| format!("invalid {}: {error}", identity_path.display()))?;
    validate_package_name(&identity.name)?;
    let origin = format!("pkg:{}", identity.name);
    let manifest_path = package_dir.join("samples.jsonl");
    let lines = read_manifest(&manifest_path)?;

    let source_ids: HashSet<_> = lines
        .iter()
        .filter_map(|line| match line {
            SampleManifestLine::Source(source) => Some(source.id.as_str()),
            _ => None,
        })
        .collect();
    let mut payloads = HashMap::new();
    for line in &lines {
        if let SampleManifestLine::Sample(sample) = line {
            if let Some(path) = verify_payload(package_dir, sample)? {
                decode_audio_file(&path).map_err(|error| {
                    format!("invalid package sample {}: {error}", path.display())
                })?;
                payloads.insert(sample.hash.clone(), path);
            }
            if let Some(source) = &sample.source {
                validate_source_id(&origin, source)?;
                if !source_ids.contains(source.as_str()) {
                    return Err(format!(
                        "sample {} references missing source {source}",
                        sample.hash
                    ));
                }
            }
        } else if let SampleManifestLine::Source(source) = line {
            validate_source_id(&origin, &source.id)?;
            db.validate_source_merge(source)
                .map_err(|error| format!("source {} cannot be merged: {error}", source.id))?;
            for asset in &source.assets {
                verify_source_asset(package_dir, asset)?;
            }
        }
    }

    db.remove_origin(&origin, sample_dir)
        .map_err(|error| format!("failed to replace {origin}: {error}"))?;
    for (hash, path) in &payloads {
        transcode_to_store(path, hash, sample_dir)?;
    }
    for line in &lines {
        if let SampleManifestLine::Source(source) = line {
            validate_source_id(&origin, &source.id)?;
            db.contribute_source(&origin, source)
                .map_err(|error| format!("failed to merge source {}: {error}", source.id))?;
        }
    }
    for line in &lines {
        if let SampleManifestLine::Sample(sample) = line {
            db.contribute_sample(&sample.hash, sample.title.as_deref(), &sample.tags, &origin)
                .map_err(|error| format!("failed to merge sample {}: {error}", sample.hash))?;
            if let Some(source) = &sample.source {
                db.associate_package_source(&sample.hash, &origin, source)
                    .map_err(|error| {
                        format!(
                            "failed to associate sample {} with {source}: {error}",
                            sample.hash
                        )
                    })?;
            }
        }
    }
    Ok(origin)
}

pub fn uninstall_package_samples(
    package_name: &str,
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<(), String> {
    validate_package_name(package_name)?;
    let origin = format!("pkg:{package_name}");
    db.remove_origin(&origin, sample_dir)
        .map_err(|error| format!("failed to uninstall {origin}: {error}"))
}

fn validate_package_name(name: &str) -> Result<(), String> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(format!("invalid package name {name:?}"))
    }
}

fn validate_source_id(origin: &str, id: &str) -> Result<(), String> {
    if id
        .strip_prefix(origin)
        .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1)
    {
        Ok(())
    } else {
        Err(format!(
            "package source id {id:?} must be namespaced under {origin}/"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_manifest::{
        index_package, read_manifest, write_manifest, PackageSample, PackageSource,
        SampleManifestLine, SourceRef,
    };

    fn write_wav(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 44_100,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(123i16).unwrap();
        writer.finalize().unwrap();
    }

    fn make_package(root: &Path, name: &str) -> String {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join("manifest.json"),
            format!(r#"{{"name":"{name}"}}"#),
        )
        .unwrap();
        write_wav(&root.join("samples/drums/kick.wav"));
        index_package(root).unwrap();
        let manifest_path = root.join("samples.jsonl");
        let mut lines = read_manifest(&manifest_path).unwrap();
        let source_id = format!("pkg:{name}/record");
        let SampleManifestLine::Sample(sample) = &mut lines[0] else {
            unreachable!()
        };
        sample.source = Some(source_id.clone());
        lines.push(SampleManifestLine::Sample(PackageSample {
            hash: if name == "one.pack" {
                "1".repeat(64)
            } else {
                "2".repeat(64)
            },
            path: None,
            title: Some("Metadata only".to_string()),
            tags: vec!["rare".to_string()],
            source: None,
        }));
        lines.push(SampleManifestLine::Source(PackageSource {
            kind: "source".to_string(),
            id: source_id,
            title: Some("Shared record".to_string()),
            release_title: None,
            contributors: Vec::new(),
            refs: vec![SourceRef {
                provider: "discogs".to_string(),
                ref_kind: "release".to_string(),
                ref_value: "123".to_string(),
                url: None,
            }],
            assets: Vec::new(),
        }));
        let hash = match &lines[0] {
            SampleManifestLine::Sample(sample) => sample.hash.clone(),
            _ => unreachable!(),
        };
        write_manifest(&manifest_path, &lines).unwrap();
        hash
    }

    #[test]
    fn package_ingest_dedupes_refs_and_uninstall_refcounts_shared_payload() {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-samples-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let hash = make_package(&root.join("one"), "one.pack");
        assert_eq!(hash, make_package(&root.join("two"), "two.pack"));
        let store = root.join("store/samples");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        let mut db = SampleDb::open(&root.join("store/samples.db")).unwrap();
        ingest_package_samples(&root.join("one"), &store, &mut db).unwrap();
        ingest_package_samples(&root.join("two"), &store, &mut db).unwrap();
        let rows = db
            .query(&[], &[], None, false, &["pkg:one.pack", "pkg:two.pack"])
            .unwrap();
        assert_eq!(rows.len(), 1);
        let package_one_rows = db
            .query_samples_for_browser_with_origins(&[], &["pkg:one.pack"], None, 16)
            .unwrap();
        assert!(package_one_rows
            .iter()
            .any(|row| row.title.as_deref() == Some("Metadata only") && !row.available));
        let facets = db
            .adjacent_tags_with_origins(&[], &["pkg:one.pack"], None, 16)
            .unwrap();
        assert!(facets
            .iter()
            .any(|facet| facet.name == "drums" && facet.count == 1));
        let ref_count: i64 = db
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM source_refs WHERE provider = 'discogs' AND ref_value = '123'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ref_count, 1);
        uninstall_package_samples("one.pack", &store, &mut db).unwrap();
        assert!(store.join(format!("{hash}.wav")).is_file());
        uninstall_package_samples("two.pack", &store, &mut db).unwrap();
        assert!(!store.join(format!("{hash}.wav")).exists());
        let source_count: i64 = db
            .connection()
            .query_row("SELECT COUNT(*) FROM sources", [], |row| row.get(0))
            .unwrap();
        assert_eq!(source_count, 0);
        let _ = fs::remove_dir_all(root);
    }
}
