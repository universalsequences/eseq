use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use eseqlisp::package::{InstalledPackage, PackageCatalog};
use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::sample_db::SampleDb;
use crate::sample_import::{decode_audio_file, transcode_to_store};
use crate::sample_manifest::{
    read_manifest, verify_payload, verify_source_asset, SampleManifestLine,
};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PackageSampleIngestReport {
    pub ingested_origins: Vec<String>,
    pub unchanged_origins: Vec<String>,
    pub removed_origins: Vec<String>,
    pub errors: Vec<String>,
}

/// Reconcile the sample database with every currently valid installed package.
///
/// Invalid packages and invalid sample manifests are isolated and reported.
/// Origins that are no longer backed by a valid installed package are removed,
/// while packages whose manifest digest has not changed are left untouched.
pub fn reconcile_installed_package_samples(
    packages_dir: &Path,
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<PackageSampleIngestReport, String> {
    let (catalog, package_errors) = PackageCatalog::scan_reporting(packages_dir);
    let mut report = PackageSampleIngestReport {
        errors: package_errors.into_iter().map(|error| error.to_string()).collect(),
        ..PackageSampleIngestReport::default()
    };
    let previous = package_ingest_state(db)?;
    let mut retained_origins = HashSet::new();

    for package in catalog.packages().values() {
        let manifest_path = package.root.join("samples.jsonl");
        if !manifest_path.is_file() {
            continue;
        }
        let origin = format!("pkg:{}", package.module_prefix);
        let digest = match manifest_digest(&manifest_path) {
            Ok(digest) => digest,
            Err(error) => {
                db.remove_origin(&origin, sample_dir).map_err(|cleanup_error| {
                    format!(
                        "{error}; cleanup of {origin} also failed: {cleanup_error}"
                    )
                })?;
                delete_package_ingest_state(db, &origin)?;
                report.errors.push(error);
                continue;
            }
        };
        if previous.get(&origin).is_some_and(|stored| stored == &digest) {
            retained_origins.insert(origin.clone());
            report.unchanged_origins.push(origin);
            continue;
        }

        match ingest_package_samples(&package.root, sample_dir, db) {
            Ok(ingested_origin) => {
                set_package_ingest_state(db, &ingested_origin, &digest)?;
                retained_origins.insert(ingested_origin.clone());
                report.ingested_origins.push(ingested_origin);
            }
            Err(error) => {
                db.remove_origin(&origin, sample_dir).map_err(|cleanup_error| {
                    format!(
                        "failed to ingest package samples from '{}': {error}; \
                         cleanup of {origin} also failed: {cleanup_error}",
                        package.root.display()
                    )
                })?;
                delete_package_ingest_state(db, &origin)?;
                report.errors.push(format!(
                    "failed to ingest package samples from '{}': {error}",
                    package.root.display()
                ));
            }
        }
    }

    for origin in previous.keys() {
        if retained_origins.contains(origin) {
            continue;
        }
        db.remove_origin(origin, sample_dir)
            .map_err(|error| format!("failed to remove stale {origin}: {error}"))?;
        delete_package_ingest_state(db, origin)?;
        report.removed_origins.push(origin.clone());
    }

    report.ingested_origins.sort();
    report.unchanged_origins.sort();
    report.removed_origins.sort();
    Ok(report)
}

pub fn reconcile_app_package_samples(
    paths: &crate::app_paths::AppPaths,
) -> Result<PackageSampleIngestReport, String> {
    let db_path = paths.sample_db_path();
    let mut db = SampleDb::open(&db_path)
        .map_err(|error| format!("failed to open {}: {error}", db_path.display()))?;
    reconcile_installed_package_samples(&paths.packages_dir(), &paths.samples_dir(), &mut db)
}

pub fn ingest_package_samples(
    package_dir: &Path,
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<String, String> {
    let package = InstalledPackage::load(package_dir).map_err(|error| error.to_string())?;
    let origin = format!("pkg:{}", package.module_prefix);
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
    package_identity: &str,
    sample_dir: &Path,
    db: &mut SampleDb,
) -> Result<(), String> {
    let module_prefix = eseqlisp::package::validate_package_name(package_identity)?;
    let origin = format!("pkg:{module_prefix}");
    db.remove_origin(&origin, sample_dir)
        .map_err(|error| format!("failed to uninstall {origin}: {error}"))?;
    delete_package_ingest_state(db, &origin)?;
    Ok(())
}

fn package_ingest_state(db: &SampleDb) -> Result<HashMap<String, String>, String> {
    let mut statement = db
        .connection()
        .prepare("SELECT origin, manifest_sha256 FROM package_sample_ingest_state")
        .map_err(|error| format!("failed to read package sample ingest state: {error}"))?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| format!("failed to read package sample ingest state: {error}"))?;
    rows.collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(|error| format!("failed to read package sample ingest state: {error}"))
}

fn set_package_ingest_state(db: &SampleDb, origin: &str, digest: &str) -> Result<(), String> {
    db.connection()
        .execute(
            "INSERT INTO package_sample_ingest_state(origin, manifest_sha256) VALUES (?, ?) \
             ON CONFLICT(origin) DO UPDATE SET manifest_sha256 = excluded.manifest_sha256",
            params![origin, digest],
        )
        .map_err(|error| format!("failed to record package sample ingest state: {error}"))?;
    Ok(())
}

fn delete_package_ingest_state(db: &SampleDb, origin: &str) -> Result<(), String> {
    db.connection()
        .execute(
            "DELETE FROM package_sample_ingest_state WHERE origin = ?",
            params![origin],
        )
        .map_err(|error| format!("failed to remove package sample ingest state: {error}"))?;
    Ok(())
}

fn manifest_digest(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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

    fn make_package(root: &Path, identity: &str) -> String {
        let module_prefix = eseqlisp::package::validate_package_name(identity).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/main.lisp"),
            format!("(module {module_prefix}.main)"),
        )
        .unwrap();
        fs::write(
            root.join("manifest.json"),
            format!(
                r#"{{"name":"{identity}","version":"1","entry":"{module_prefix}.main"}}"#
            ),
        )
        .unwrap();
        write_wav(&root.join("samples/drums/kick.wav"));
        index_package(root).unwrap();
        let manifest_path = root.join("samples.jsonl");
        let mut lines = read_manifest(&manifest_path).unwrap();
        let source_id = format!("pkg:{module_prefix}/record");
        let SampleManifestLine::Sample(sample) = &mut lines[0] else {
            unreachable!()
        };
        sample.source = Some(source_id.clone());
        lines.push(SampleManifestLine::Sample(PackageSample {
            hash: if identity == "test/one" {
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
    fn reconcile_skips_unchanged_packages_and_removes_deleted_package_claims() {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-reconcile-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let packages = root.join("packages");
        let package = packages.join("one");
        let hash = make_package(&package, "test/one");
        let store = root.join("store/samples");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        let mut db = SampleDb::open(&root.join("store/samples.db")).unwrap();

        let first = reconcile_installed_package_samples(&packages, &store, &mut db).unwrap();
        assert_eq!(first.ingested_origins, vec!["pkg:test.one"]);
        assert!(first.errors.is_empty());
        db.contribute_sample(
            &hash,
            Some("My title wins"),
            &["personal".to_string()],
            "user",
        )
        .unwrap();

        let unchanged = reconcile_installed_package_samples(&packages, &store, &mut db).unwrap();
        assert_eq!(unchanged.unchanged_origins, vec!["pkg:test.one"]);
        assert!(unchanged.ingested_origins.is_empty());

        fs::remove_dir_all(&package).unwrap();
        let removed = reconcile_installed_package_samples(&packages, &store, &mut db).unwrap();
        assert_eq!(removed.removed_origins, vec!["pkg:test.one"]);
        let rows = db.query(&[], &[], None, false, &["user"]).unwrap();
        assert!(rows.iter().any(|row| row.hash == hash));
        assert!(db
            .query(&[], &[], None, false, &["pkg:test.one"])
            .unwrap()
            .is_empty());
        assert!(store.join(format!("{hash}.wav")).is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconcile_reports_malformed_sample_manifest_without_rejecting_other_packages() {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-reconcile-invalid-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let packages = root.join("packages");
        make_package(&packages.join("valid"), "test/one");
        make_package(&packages.join("broken"), "test/two");
        fs::write(packages.join("broken/samples.jsonl"), "not json\n").unwrap();
        let store = root.join("store/samples");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        let mut db = SampleDb::open(&root.join("store/samples.db")).unwrap();

        let report = reconcile_installed_package_samples(&packages, &store, &mut db).unwrap();
        assert_eq!(report.ingested_origins, vec!["pkg:test.one"]);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("broken"));
        assert_eq!(
            db.query(&[], &[], None, false, &["pkg:test.one"])
                .unwrap()
                .len(),
            2
        );
        assert!(db
            .query(&[], &[], None, false, &["pkg:test.two"])
            .unwrap()
            .is_empty());

        let _ = fs::remove_dir_all(root);
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
        let hash = make_package(&root.join("one"), "test/one");
        assert_eq!(hash, make_package(&root.join("two"), "test/two"));
        let store = root.join("store/samples");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        let mut db = SampleDb::open(&root.join("store/samples.db")).unwrap();
        ingest_package_samples(&root.join("one"), &store, &mut db).unwrap();
        ingest_package_samples(&root.join("two"), &store, &mut db).unwrap();
        let rows = db
            .query(&[], &[], None, false, &["pkg:test.one", "pkg:test.two"])
            .unwrap();
        assert_eq!(rows.len(), 1);
        let package_one_rows = db
            .query_samples_for_browser_with_origins(&[], &["pkg:test.one"], None, 16)
            .unwrap();
        assert!(package_one_rows
            .iter()
            .any(|row| row.title.as_deref() == Some("Metadata only") && !row.available));
        let facets = db
            .adjacent_tags_with_origins(&[], &["pkg:test.one"], None, 16)
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
        uninstall_package_samples("test/one", &store, &mut db).unwrap();
        assert!(store.join(format!("{hash}.wav")).is_file());
        uninstall_package_samples("test/two", &store, &mut db).unwrap();
        assert!(!store.join(format!("{hash}.wav")).exists());
        let source_count: i64 = db
            .connection()
            .query_row("SELECT COUNT(*) FROM sources", [], |row| row.get(0))
            .unwrap();
        assert_eq!(source_count, 0);
        let _ = fs::remove_dir_all(root);
    }
}
