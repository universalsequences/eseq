use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sample_import::{collect_audio_files, default_title_for_path, normalize_tags};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SampleManifestLine {
    Sample(PackageSample),
    Source(PackageSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSample {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSource {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contributors: Vec<SourceContributor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<SourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<SourceAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceContributor {
    pub role: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub provider: String,
    pub ref_kind: String,
    pub ref_value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAsset {
    pub kind: String,
    pub hash: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

pub fn index_package(package_dir: &Path) -> Result<Vec<SampleManifestLine>, String> {
    let samples_dir = package_dir.join("samples");
    let mut files = collect_audio_files(&[samples_dir.clone()]);
    files.sort();
    let mut lines = Vec::with_capacity(files.len());
    for file in files {
        let relative = file.strip_prefix(&samples_dir).map_err(|_| {
            format!(
                "sample escaped package samples directory: {}",
                file.display()
            )
        })?;
        let bytes = fs::read(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let path = portable_relative_path(relative)?;
        let tags = relative.parent().map(path_tags).unwrap_or_default();
        lines.push(SampleManifestLine::Sample(PackageSample {
            hash: sha256_hex(&bytes),
            path: Some(path),
            title: Some(default_title_for_path(&file)),
            tags,
            source: None,
        }));
    }
    write_manifest(&package_dir.join("samples.jsonl"), &lines)?;
    Ok(lines)
}

pub fn read_manifest(path: &Path) -> Result<Vec<SampleManifestLine>, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut lines = Vec::new();
    let mut hashes = HashSet::new();
    let mut paths = HashSet::new();
    let mut source_ids = HashSet::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let mut value: serde_json::Value = serde_json::from_str(&line)
            .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
        let parsed = if value.get("kind").and_then(|kind| kind.as_str()) == Some("source") {
            SampleManifestLine::Source(serde_json::from_value(value).map_err(|error| {
                format!("{}:{}: invalid source: {error}", path.display(), index + 1)
            })?)
        } else {
            if let Some(object) = value.as_object_mut() {
                object.remove("kind");
            }
            SampleManifestLine::Sample(serde_json::from_value(value).map_err(|error| {
                format!("{}:{}: invalid sample: {error}", path.display(), index + 1)
            })?)
        };
        match &parsed {
            SampleManifestLine::Sample(sample) => {
                validate_hash(&sample.hash)?;
                if !hashes.insert(sample.hash.clone()) {
                    return Err(format!(
                        "{}:{}: duplicate sample hash {}",
                        path.display(),
                        index + 1,
                        sample.hash
                    ));
                }
                if let Some(relative) = &sample.path {
                    validate_relative_path(relative)?;
                    if !paths.insert(relative.clone()) {
                        return Err(format!(
                            "{}:{}: duplicate sample path {relative}",
                            path.display(),
                            index + 1
                        ));
                    }
                }
            }
            SampleManifestLine::Source(source) => {
                if source.id.trim().is_empty() || !source_ids.insert(source.id.clone()) {
                    return Err(format!(
                        "{}:{}: empty or duplicate source id",
                        path.display(),
                        index + 1
                    ));
                }
            }
        }
        lines.push(parsed);
    }
    Ok(lines)
}

pub fn write_manifest(path: &Path, lines: &[SampleManifestLine]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let tmp = path.with_extension("jsonl.tmp");
    let file = File::create(&tmp)
        .map_err(|error| format!("failed to create {}: {error}", tmp.display()))?;
    let mut writer = BufWriter::new(file);
    for line in lines {
        serde_json::to_writer(&mut writer, line)
            .map_err(|error| format!("failed to encode {}: {error}", path.display()))?;
        writer
            .write_all(b"\n")
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("failed to flush {}: {error}", path.display()))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| format!("failed to sync {}: {error}", path.display()))?;
    fs::rename(&tmp, path).map_err(|error| format!("failed to replace {}: {error}", path.display()))
}

pub fn verify_source_asset(package_dir: &Path, asset: &SourceAsset) -> Result<PathBuf, String> {
    validate_hash(&asset.hash)?;
    validate_relative_path(&asset.path)?;
    let path = package_dir.join(&asset.path);
    let bytes = fs::read(&path)
        .map_err(|error| format!("failed to read source asset {}: {error}", path.display()))?;
    let actual = sha256_hex(&bytes);
    if actual != asset.hash {
        return Err(format!(
            "source asset hash mismatch for {}: expected {}, got {actual}",
            path.display(),
            asset.hash
        ));
    }
    Ok(path)
}

pub fn verify_payload(
    package_dir: &Path,
    sample: &PackageSample,
) -> Result<Option<PathBuf>, String> {
    let Some(relative) = &sample.path else {
        return Ok(None);
    };
    validate_relative_path(relative)?;
    let path = package_dir.join("samples").join(relative);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let actual = sha256_hex(&bytes);
    if actual != sample.hash {
        return Err(format!(
            "sample payload hash mismatch for {}: expected {}, got {actual}",
            path.display(),
            sample.hash
        ));
    }
    Ok(Some(path))
}

fn validate_hash(hash: &str) -> Result<(), String> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!("invalid SHA-256 hash {hash:?}"))
    }
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "manifest path must be a normalized relative path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn portable_relative_path(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| format!("sample path is not UTF-8: {}", path.display()))?,
            ),
            _ => return Err(format!("sample path is not relative: {}", path.display())),
        }
    }
    Ok(parts.join("/"))
}

fn path_tags(path: &Path) -> Vec<String> {
    normalize_tags(
        &path
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => name.to_str().map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>(),
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rejects_parent_traversal() {
        assert!(validate_relative_path("../outside.wav").is_err());
    }
}
