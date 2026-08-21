//! Lisp package manifests and validation.
//!
//! A package is a source repository installed beneath `~/.eseq.d/packages`.
//! Its manifest owns one author-scoped module prefix and declares every
//! external asset needed before its Lisp can be put on the module load path.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::modules::{inspect_exports, is_valid_module_name};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    /// Distribution identity (`author/name`).
    pub name: String,
    #[serde(deserialize_with = "deserialize_version")]
    pub version: String,
    /// Package identities required by this package. Version selection is
    /// deliberately deferred; dependencies are identity-only in format v1.
    #[serde(default)]
    pub deps: Vec<String>,
    /// Module evaluated by consumers as the package entry point.
    pub entry: String,
    #[serde(default, alias = "assets")]
    pub external_assets: Vec<ExternalAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalAsset {
    /// Package-relative path and user-facing asset name.
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub root: PathBuf,
    pub source_root: PathBuf,
    pub module_prefix: String,
    pub manifest: PackageManifest,
}

#[derive(Debug, Clone, Default)]
pub struct PackageCatalog {
    packages: BTreeMap<String, InstalledPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid package '{}': {}",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for PackageError {}

impl PackageCatalog {
    pub fn scan(root: impl AsRef<Path>) -> Result<Self, Vec<PackageError>> {
        let root = root.as_ref();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(vec![package_error(root, error)]),
        };
        let mut directories = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir() && !is_hidden(path))
            .collect::<Vec<_>>();
        directories.sort();

        let mut packages = BTreeMap::new();
        let mut errors = Vec::new();
        for directory in directories {
            match InstalledPackage::load(&directory) {
                Ok(package) => {
                    if packages.contains_key(&package.manifest.name) {
                        errors.push(PackageError {
                            path: directory,
                            message: format!(
                                "duplicate package identity `{}`",
                                package.manifest.name
                            ),
                        });
                    } else {
                        packages.insert(package.manifest.name.clone(), package);
                    }
                }
                Err(error) => errors.push(error),
            }
        }

        let names = packages.keys().cloned().collect::<BTreeSet<_>>();
        for package in packages.values() {
            for dependency in &package.manifest.deps {
                if !names.contains(dependency) {
                    errors.push(PackageError {
                        path: package.root.join("manifest.json"),
                        message: format!("dependency `{dependency}` is not installed"),
                    });
                }
            }
        }
        if errors.is_empty() {
            Ok(Self { packages })
        } else {
            Err(errors)
        }
    }

    pub fn packages(&self) -> &BTreeMap<String, InstalledPackage> {
        &self.packages
    }

    pub fn module_roots(&self) -> Vec<(PathBuf, String)> {
        self.packages
            .values()
            .map(|package| (package.source_root.clone(), package.module_prefix.clone()))
            .collect()
    }
}

impl InstalledPackage {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, PackageError> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join("manifest.json");
        let bytes =
            fs::read(&manifest_path).map_err(|error| package_error(&manifest_path, error))?;
        let manifest: PackageManifest =
            serde_json::from_slice(&bytes).map_err(|error| PackageError {
                path: manifest_path.clone(),
                message: error.to_string(),
            })?;
        let module_prefix =
            validate_package_name(&manifest.name).map_err(|message| PackageError {
                path: manifest_path.clone(),
                message,
            })?;
        if manifest.version.trim().is_empty() {
            return Err(PackageError {
                path: manifest_path,
                message: "version must not be empty".into(),
            });
        }
        for dependency in &manifest.deps {
            validate_package_name(dependency).map_err(|message| PackageError {
                path: root.join("manifest.json"),
                message: format!("invalid dependency `{dependency}`: {message}"),
            })?;
        }
        if !manifest.entry.starts_with(&format!("{module_prefix}.")) {
            return Err(PackageError {
                path: root.join("manifest.json"),
                message: format!(
                    "entry module `{}` is outside owned namespace `{module_prefix}`",
                    manifest.entry
                ),
            });
        }
        let source_root = root.join("src");
        if !source_root.is_dir() {
            return Err(PackageError {
                path: source_root,
                message: "missing src directory".into(),
            });
        }
        validate_modules(&source_root, &module_prefix, &manifest.entry)?;
        for asset in &manifest.external_assets {
            verify_asset(&root, asset)?;
        }
        Ok(Self {
            root,
            source_root,
            module_prefix,
            manifest,
        })
    }
}

/// Validate `author/name` and return its owned Lisp module prefix
/// (`author.name`). Core namespaces and unscoped names can never be claimed.
pub fn validate_package_name(name: &str) -> Result<String, String> {
    let Some((author, package)) = name.split_once('/') else {
        return Err("name must be author-scoped (`author/name`)".into());
    };
    if package.contains('/') || !valid_segment(author) || !valid_segment(package) {
        return Err("author and package must be non-empty ASCII identifier segments".into());
    }
    let prefix = format!("{author}.{package}");
    if !is_valid_module_name(&prefix) || prefix.starts_with("eseq.") {
        return Err("package cannot claim a reserved core namespace".into());
    }
    Ok(prefix)
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_modules(source_root: &Path, prefix: &str, entry: &str) -> Result<(), PackageError> {
    let mut files = Vec::new();
    collect_lisp_files(source_root, &mut files)?;
    let mut modules = BTreeSet::new();
    for path in files {
        let source = fs::read_to_string(&path).map_err(|error| package_error(&path, error))?;
        let (module, _) = inspect_exports(&source).map_err(|message| PackageError {
            path: path.clone(),
            message,
        })?;
        let Some(module) = module else {
            return Err(PackageError {
                path,
                message: "package Lisp files must declare a module".into(),
            });
        };
        if !module.starts_with(&format!("{prefix}.")) {
            return Err(PackageError {
                path,
                message: format!("module `{module}` is outside package namespace `{prefix}`"),
            });
        }
        if !modules.insert(module.clone()) {
            return Err(PackageError {
                path,
                message: format!("duplicate module `{module}`"),
            });
        }
    }
    if !modules.contains(entry) {
        return Err(PackageError {
            path: source_root.to_path_buf(),
            message: format!("entry module `{entry}` was not found under src"),
        });
    }
    Ok(())
}

fn collect_lisp_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), PackageError> {
    let entries = fs::read_dir(directory).map_err(|error| package_error(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| package_error(directory, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| package_error(&path, error))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_lisp_files(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("lisp") {
            files.push(path);
        }
    }
    files.sort();
    Ok(())
}

fn verify_asset(root: &Path, asset: &ExternalAsset) -> Result<(), PackageError> {
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackageError {
            path: root.join("manifest.json"),
            message: format!("asset `{}` has an invalid SHA-256", asset.name),
        });
    }
    let relative = Path::new(&asset.name);
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PackageError {
            path: root.join("manifest.json"),
            message: format!("asset `{}` is not a safe package-relative path", asset.name),
        });
    }
    let path = root.join(relative);
    let mut file = fs::File::open(&path).map_err(|error| PackageError {
        path: path.clone(),
        message: if error.kind() == std::io::ErrorKind::NotFound {
            format!("required external asset `{}` is missing", asset.name)
        } else {
            error.to_string()
        },
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| package_error(&path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if !actual.eq_ignore_ascii_case(&asset.sha256) {
        return Err(PackageError {
            path,
            message: format!(
                "asset `{}` hash mismatch: expected {}, got {actual}",
                asset.name, asset.sha256
            ),
        });
    }
    Ok(())
}

fn deserialize_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(value) => Ok(value),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        _ => Err(serde::de::Error::custom(
            "version must be a string or number",
        )),
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn package_error(path: impl AsRef<Path>, error: impl fmt::Display) -> PackageError {
    PackageError {
        path: path.as_ref().to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("eseqlisp-package-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn package_manifest_enforces_namespace_and_assets() {
        let root = temp_root("valid");
        let package = root.join("acid-tools");
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(
            package.join("src/ui.lisp"),
            "(module alec.acid-tools.ui)\n(export panel)\n(def panel 1)",
        )
        .unwrap();
        fs::write(package.join("impulse.wav"), b"asset").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"asset"));
        fs::write(
            package.join("manifest.json"),
            format!(
                r#"{{
            "name": "alec/acid-tools", "version": "1.2.0", "entry": "alec.acid-tools.ui",
            "external_assets": [{{"name": "impulse.wav", "sha256": "{hash}"}}]
        }}"#
            ),
        )
        .unwrap();
        let catalog = PackageCatalog::scan(&root).expect("valid catalog");
        assert_eq!(
            catalog.module_roots(),
            vec![(package.join("src"), "alec.acid-tools".into())]
        );
    }

    #[test]
    fn package_reports_missing_assets_instead_of_becoming_loadable() {
        let root = temp_root("missing-asset");
        let package = root.join("pack");
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(package.join("src/main.lisp"), "(module alec.pack.main)").unwrap();
        fs::write(package.join("manifest.json"), r#"{
            "name":"alec/pack", "version":1, "entry":"alec.pack.main",
            "assets":[{"name":"samples/kick.wav","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}]
        }"#).unwrap();
        let errors = PackageCatalog::scan(&root).expect_err("missing asset must reject package");
        assert!(errors.iter().any(|error| {
            error
                .to_string()
                .contains("required external asset `samples/kick.wav` is missing")
        }));
    }

    #[test]
    fn package_cannot_publish_another_authors_module() {
        let root = temp_root("namespace");
        let package = root.join("pack");
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(package.join("src/main.lisp"), "(module bob.stolen.main)").unwrap();
        fs::write(
            package.join("manifest.json"),
            r#"{"name":"alec/pack","version":"1","entry":"alec.pack.main"}"#,
        )
        .unwrap();
        let errors = PackageCatalog::scan(&root).expect_err("foreign module must reject package");
        assert!(errors[0].to_string().contains("outside package namespace"));
    }
}
