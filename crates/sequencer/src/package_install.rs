//! Atomic installation for Lisp/content packages, from a git repository or
//! from a directory / zip the user picked in a file dialog.
//!
//! Every route stages first: the candidate is materialized under a hidden
//! `.install-*` directory beside the installed packages, validated there with
//! [`InstalledPackage::load`], and only then renamed into place. A failed
//! clone, copy, unzip, or validation leaves no installed directory behind.

use std::path::{Path, PathBuf};
use std::process::Command;

use eseqlisp::package::{InstalledPackage, PACKAGE_CONTENT_DIRS};

#[derive(Debug, Clone)]
pub struct InstalledPackageResult {
    pub identity: String,
    pub path: PathBuf,
}

/// What a package carries, for the confirmation modal and status lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageSummary {
    pub identity: String,
    pub version: String,
    /// Lisp modules under `src/`.
    pub modules: usize,
    pub instruments: usize,
    pub effects: usize,
    pub midi_fx: usize,
    /// Samples declared in `samples.jsonl` (the ingestable set), or, when
    /// the package ships audio without an index, the audio files found.
    pub samples: usize,
    pub themes: usize,
}

impl PackageSummary {
    pub fn of(package: &InstalledPackage) -> Self {
        let modules = package
            .source_root
            .as_ref()
            .map(|root| count_files(root, |path| path.extension().is_some_and(|ext| ext == "lisp")))
            .unwrap_or(0);
        let samples = match crate::sample_manifest::read_manifest(&package.root.join("samples.jsonl"))
        {
            Ok(lines) => lines
                .iter()
                .filter(|line| matches!(line, crate::sample_manifest::SampleManifestLine::Sample(_)))
                .count(),
            Err(_) => package
                .content_dir("samples")
                .map(|dir| {
                    count_files(&dir, |path| {
                        path.extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| {
                                matches!(ext.to_ascii_lowercase().as_str(), "wav" | "aif" | "aiff" | "flac" | "mp3")
                            })
                    })
                })
                .unwrap_or(0),
        };
        Self {
            identity: package.manifest.name.clone(),
            version: package.manifest.version.clone(),
            modules,
            instruments: package
                .instruments_dir()
                .map(|dir| count_dsp_folders(&dir))
                .unwrap_or(0),
            effects: package.effects_dir().map(|dir| count_dsp_folders(&dir)).unwrap_or(0),
            midi_fx: package
                .content_dir("midi-fx")
                .map(|dir| count_dsp_folders(&dir))
                .unwrap_or(0),
            samples,
            themes: package
                .content_dir("themes")
                .map(|dir| count_files(&dir, |path| path.extension().is_some_and(|ext| ext == "lisp")))
                .unwrap_or(0),
        }
    }

    /// "3 instruments, 2 effects, 40 samples" — only the non-zero parts.
    pub fn describe_contents(&self) -> String {
        let mut parts = Vec::new();
        for (count, singular, plural) in [
            (self.modules, "module", "modules"),
            (self.instruments, "instrument", "instruments"),
            (self.effects, "effect", "effects"),
            (self.midi_fx, "MIDI effect", "MIDI effects"),
            (self.samples, "sample", "samples"),
            (self.themes, "theme", "themes"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {}", if count == 1 { singular } else { plural }));
            }
        }
        if parts.is_empty() {
            "no content".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Folder-style content (`<name>/dsp.lisp`) plus flat `<name>.lisp` sources,
/// the same two layouts the instrument and effect loaders accept.
fn count_dsp_folders(dir: &Path) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if path.join("dsp.lisp").is_file() {
                count += 1;
            } else {
                count += count_dsp_folders(&path);
            }
        } else if path.extension().is_some_and(|ext| ext == "lisp")
            && !matches!(path.file_stem().and_then(|s| s.to_str()), Some("dsp" | "ui"))
        {
            count += 1;
        }
    }
    count
}

fn count_files(dir: &Path, accept: impl Fn(&Path) -> bool + Copy) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            count += count_files(&path, accept);
        } else if accept(&path) {
            count += 1;
        }
    }
    count
}

/// A validated package sitting in staging, not yet visible to any scan.
/// Drop it with [`discard_staged_package`] or publish it with
/// [`publish_staged_package`]; leaking it leaves a hidden directory that the
/// package scan ignores.
#[derive(Debug)]
pub struct StagedPackage {
    staging: PathBuf,
    packages_dir: PathBuf,
    pub package: InstalledPackage,
    pub summary: PackageSummary,
}

impl StagedPackage {
    pub fn identity(&self) -> &str {
        &self.package.manifest.name
    }

    /// Where the package will live once published.
    pub fn destination(&self) -> PathBuf {
        self.packages_dir.join(&self.package.module_prefix)
    }

    /// Whether publishing would replace a package already installed under
    /// this identity.
    pub fn replaces_installed(&self) -> bool {
        self.destination().exists()
    }

    pub fn staging_path(&self) -> &Path {
        &self.staging
    }
}

fn staging_dir(packages_dir: &Path) -> PathBuf {
    packages_dir.join(format!(
        ".install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

fn validate_staging(
    staging: PathBuf,
    packages_dir: &Path,
    expected_identity: Option<&str>,
) -> Result<StagedPackage, String> {
    let package = InstalledPackage::load(&staging).map_err(|error| error.to_string())?;
    if let Some(expected) = expected_identity {
        let expected_prefix = eseqlisp::package::validate_package_name(expected)?;
        if package.manifest.name != expected {
            return Err(format!(
                "repository declares package `{}`, expected `{expected}`",
                package.manifest.name
            ));
        }
        if package.module_prefix != expected_prefix {
            return Err("package identity produced an inconsistent module namespace".into());
        }
    }
    let summary = PackageSummary::of(&package);
    Ok(StagedPackage {
        staging,
        packages_dir: packages_dir.to_path_buf(),
        package,
        summary,
    })
}

/// Clone a repository into staging and validate it. The result is not yet
/// installed: publish it with [`publish_staged_package`].
pub fn stage_git_package(
    repository: &str,
    expected_identity: &str,
    packages_dir: &Path,
) -> Result<StagedPackage, String> {
    eseqlisp::package::validate_package_name(expected_identity)?;
    std::fs::create_dir_all(packages_dir)
        .map_err(|error| format!("failed to create {}: {error}", packages_dir.display()))?;
    let staging = staging_dir(packages_dir);
    let output = match Command::new("git")
        .args(["clone", "--quiet", "--", repository])
        .arg(&staging)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("failed to launch git clone: {error}"));
        }
    };
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    validate_staging(staging.clone(), packages_dir, Some(expected_identity)).map_err(|error| {
        let _ = std::fs::remove_dir_all(&staging);
        error
    })
}

/// Clone, validate, and publish in one step (the `eseq package install` CLI).
/// Refuses to replace an installed package.
pub fn install_git_package(
    repository: &str,
    expected_identity: &str,
    packages_dir: &Path,
) -> Result<InstalledPackageResult, String> {
    let expected_prefix = eseqlisp::package::validate_package_name(expected_identity)?;
    if packages_dir.join(&expected_prefix).exists() {
        return Err(format!(
            "package `{expected_identity}` is already installed"
        ));
    }
    let staged = stage_git_package(repository, expected_identity, packages_dir)?;
    publish_staged_package(staged, false)
}

/// Whether a picked path looks like a zipped package.
pub fn is_package_archive(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "zip" | "eseqpack"))
}

/// Stage a package the user picked: a package directory, or a `.zip` /
/// `.eseqpack` archive of one. Archives whose single top-level entry is the
/// package directory (how Finder and `zip -r` produce them) are unwrapped.
/// The source is never modified. The result is validated but not installed.
pub fn stage_package_from_path(source: &Path, packages_dir: &Path) -> Result<StagedPackage, String> {
    std::fs::create_dir_all(packages_dir)
        .map_err(|error| format!("failed to create {}: {error}", packages_dir.display()))?;
    let staging = staging_dir(packages_dir);
    let result = (|| -> Result<StagedPackage, String> {
        if is_package_archive(source) {
            extract_archive(source, &staging)?;
        } else if source.is_dir() {
            if source.starts_with(packages_dir) {
                return Err(format!(
                    "{} is already inside the packages directory",
                    source.display()
                ));
            }
            copy_dir_recursive(source, &staging)?;
        } else {
            return Err(format!(
                "{} is neither a package directory nor a .zip/.eseqpack archive",
                source.display()
            ));
        }
        let package_root = unwrap_single_directory(&staging)?;
        if package_root != staging {
            // Hoist the wrapped package so `staging` itself is the package.
            let hoisted = staging_dir(packages_dir);
            std::fs::rename(&package_root, &hoisted)
                .map_err(|error| format!("failed to unwrap archive: {error}"))?;
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::rename(&hoisted, &staging)
                .map_err(|error| format!("failed to unwrap archive: {error}"))?;
        }
        if !staging.join("manifest.json").is_file() {
            return Err(format!(
                "{} has no manifest.json at its root (expected a package directory with manifest.json, src/, {})",
                source.display(),
                PACKAGE_CONTENT_DIRS.join("/, ")
            ));
        }
        validate_staging(staging.clone(), packages_dir, None)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Rename a staged package into place. With `replace`, an installed package
/// of the same identity is swapped out atomically (moved aside, then
/// deleted); without it, that case is an error and staging is discarded.
pub fn publish_staged_package(
    staged: StagedPackage,
    replace: bool,
) -> Result<InstalledPackageResult, String> {
    let destination = staged.destination();
    let identity = staged.identity().to_string();
    let cleanup = |message: String| {
        let _ = std::fs::remove_dir_all(&staged.staging);
        Err(message)
    };
    let mut displaced = None;
    if destination.exists() {
        if !replace {
            return cleanup(format!("package `{identity}` is already installed"));
        }
        let aside = staged.packages_dir.join(format!(
            ".replaced-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        if let Err(error) = std::fs::rename(&destination, &aside) {
            return cleanup(format!(
                "failed to move the installed package aside: {error}"
            ));
        }
        displaced = Some(aside);
    }
    if let Err(error) = std::fs::rename(&staged.staging, &destination) {
        if let Some(aside) = &displaced {
            let _ = std::fs::rename(aside, &destination);
        }
        return cleanup(format!(
            "failed to publish package at {}: {error}",
            destination.display()
        ));
    }
    if let Some(aside) = displaced {
        let _ = std::fs::remove_dir_all(aside);
    }
    crate::app_paths::invalidate_package_catalog_cache();
    Ok(InstalledPackageResult {
        identity,
        path: destination,
    })
}

pub fn discard_staged_package(staged: StagedPackage) {
    let _ = std::fs::remove_dir_all(&staged.staging);
}

/// Remove an installed package directory. Sample claims are reconciled by
/// the caller (`reconcile_app_package_samples`), which sweeps origins whose
/// package is gone.
pub fn uninstall_package(packages_dir: &Path, identity: &str) -> Result<PathBuf, String> {
    let prefix = eseqlisp::package::validate_package_name(identity)?;
    let path = packages_dir.join(prefix);
    if !path.is_dir() {
        return Err(format!("package `{identity}` is not installed"));
    }
    std::fs::remove_dir_all(&path)
        .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
    crate::app_paths::invalidate_package_catalog_cache();
    Ok(path)
}

/// If `dir` holds exactly one visible entry and it is a directory, return
/// that directory (an archive wrapping its package folder); otherwise `dir`.
fn unwrap_single_directory(dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("failed to read {}: {error}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.') || name == "__MACOSX")
        })
        .collect::<Vec<_>>();
    match entries.as_slice() {
        [only] if only.is_dir() && !dir.join("manifest.json").is_file() => Ok(only.clone()),
        _ => Ok(dir.to_path_buf()),
    }
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    // `ditto` is on every macOS; `unzip` is the portable fallback. Both keep
    // the process free of a zip dependency for an operation that happens a
    // few times per install, never per frame.
    let attempts: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("ditto", &["-x", "-k"]), ("unzip", &["-q", "-o"])]
    } else {
        &[("unzip", &["-q", "-o"]), ("ditto", &["-x", "-k"])]
    };
    let mut last_error = String::new();
    for (program, flags) in attempts {
        let mut command = Command::new(program);
        command.args(*flags).arg(archive);
        if *program == "unzip" {
            command.arg("-d");
        }
        command.arg(destination);
        match command.output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                last_error = format!(
                    "{program} failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(error) => last_error = format!("failed to launch {program}: {error}"),
        }
    }
    Err(format!("could not extract {}: {last_error}", archive.display()))
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read {}: {error}", source.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            // Symlinks are neither followed (they could point anywhere) nor
            // recreated: the package scan skips them too.
            continue;
        }
        if file_type.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|error| format!("failed to copy {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_samples::reconcile_installed_package_samples;
    use crate::sample_db::SampleDb;
    use crate::sample_manifest::index_package;

    fn initialize_git_repository(repo: &Path) {
        for args in [
            vec!["init", "--quiet"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ],
        ] {
            let status = Command::new("git")
                .args(args)
                .current_dir(repo)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    fn write_content_pack(dir: &Path, identity: &str) {
        std::fs::create_dir_all(dir.join("instruments/kick")).unwrap();
        std::fs::write(dir.join("instruments/kick/dsp.lisp"), "(out 0)").unwrap();
        std::fs::create_dir_all(dir.join("instruments/kits/808")).unwrap();
        std::fs::write(dir.join("instruments/kits/808/dsp.lisp"), "(out 0)").unwrap();
        std::fs::create_dir_all(dir.join("effects/comp")).unwrap();
        std::fs::write(dir.join("effects/comp/dsp.lisp"), "(out (in 1) 1)").unwrap();
        std::fs::write(dir.join("effects/comp/ui.lisp"), "(def ui 1)").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(r#"{{"name":"{identity}","version":"2.1"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn staging_from_a_directory_copies_validates_and_publishes() {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-stage-dir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let source = root.join("picked/alec.drums");
        let installed = root.join("packages");
        write_content_pack(&source, "alec/drums");

        let staged = stage_package_from_path(&source, &installed).unwrap();
        assert!(staged.staging_path().starts_with(&installed));
        assert!(staged
            .staging_path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(".install-"));
        assert_eq!(staged.identity(), "alec/drums");
        assert!(!staged.replaces_installed());
        assert_eq!(
            staged.summary,
            PackageSummary {
                identity: "alec/drums".into(),
                version: "2.1".into(),
                modules: 0,
                instruments: 2,
                effects: 1,
                midi_fx: 0,
                samples: 0,
                themes: 0,
            }
        );
        assert_eq!(staged.summary.describe_contents(), "2 instruments, 1 effect");

        let result = publish_staged_package(staged, false).unwrap();
        assert_eq!(result.path, installed.join("alec.drums"));
        assert!(result.path.join("instruments/kick/dsp.lisp").is_file());
        assert!(source.join("manifest.json").is_file(), "the picked source is untouched");
        // No staging directory survives a publish.
        assert!(std::fs::read_dir(&installed)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_str().unwrap().starts_with(".install-")));

        // Installing again refuses without `replace` and leaves no staging
        // behind; with `replace` the new version swaps in atomically.
        std::fs::write(source.join("manifest.json"), r#"{"name":"alec/drums","version":"3"}"#).unwrap();
        let staged = stage_package_from_path(&source, &installed).unwrap();
        assert!(staged.replaces_installed());
        let error = publish_staged_package(staged, false).unwrap_err();
        assert!(error.contains("already installed"), "{error}");
        let staged = stage_package_from_path(&source, &installed).unwrap();
        let result = publish_staged_package(staged, true).unwrap();
        let manifest = std::fs::read_to_string(result.path.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"3\""));
        assert!(std::fs::read_dir(&installed)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_str().unwrap().starts_with('.')));

        // An invalid pick leaves nothing behind either.
        let junk = root.join("junk");
        std::fs::create_dir_all(&junk).unwrap();
        std::fs::write(junk.join("manifest.json"), "not json").unwrap();
        assert!(stage_package_from_path(&junk, &installed).is_err());
        let not_a_package = root.join("plain");
        std::fs::create_dir_all(&not_a_package).unwrap();
        let error = stage_package_from_path(&not_a_package, &installed).unwrap_err();
        assert!(error.contains("manifest.json"), "{error}");
        assert_eq!(
            std::fs::read_dir(&installed).unwrap().flatten().count(),
            1,
            "only the installed package remains"
        );

        assert_eq!(
            uninstall_package(&installed, "alec/drums").unwrap(),
            installed.join("alec.drums")
        );
        assert!(!installed.join("alec.drums").exists());
        assert!(uninstall_package(&installed, "alec/drums").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staging_from_an_archive_unwraps_the_package_folder() {
        let root = std::env::temp_dir().join(format!(
            "eseq-package-stage-zip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let source = root.join("picked/alec.drums");
        let installed = root.join("packages");
        write_content_pack(&source, "alec/drums");

        // `zip -r pack.eseqpack alec.drums` from the parent: the archive wraps
        // the package folder, exactly how Finder's Compress does it too.
        let archive = root.join("picked/alec.drums-2.1.eseqpack");
        let status = Command::new("zip")
            .args(["-q", "-r"])
            .arg(&archive)
            .arg("alec.drums")
            .current_dir(root.join("picked"))
            .status();
        let Ok(status) = status else {
            eprintln!("zip is not available; skipping archive test");
            return;
        };
        assert!(status.success());
        assert!(is_package_archive(&archive));
        assert!(!is_package_archive(&source));

        let staged = stage_package_from_path(&archive, &installed).unwrap();
        assert_eq!(staged.identity(), "alec/drums");
        assert_eq!(staged.summary.instruments, 2);
        assert!(staged.staging_path().join("manifest.json").is_file(), "the wrapper folder was unwrapped");
        let result = publish_staged_package(staged, false).unwrap();
        assert!(result.path.join("effects/comp/ui.lisp").is_file());
        assert!(!result.path.join("alec.drums").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn write_wav(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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

    #[test]
    fn git_install_validates_then_atomically_publishes_package() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "eseq-package-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = root.join("repo");
        let installed = root.join("installed");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/main.lisp"), "(module test.publisher.main)").unwrap();
        std::fs::write(
            repo.join("manifest.json"),
            r#"{"name":"test/publisher","version":"1","entry":"test.publisher.main"}"#,
        )
        .unwrap();
        initialize_git_repository(&repo);

        let result =
            install_git_package(repo.to_str().unwrap(), "test/publisher", &installed).unwrap();
        assert_eq!(result.path, installed.join("test.publisher"));
        assert!(result.path.join("manifest.json").is_file());
        assert!(
            !std::fs::read_dir(&installed)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with(".install-"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_package_loads_lisp_and_ingests_samples_under_derived_origin() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "eseq-package-install-content-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = root.join("repo");
        let installed = root.join("installed");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(
            repo.join("src/main.lisp"),
            "(module test.publisher.main)\n(export answer)\n(def answer () 42)",
        )
        .unwrap();
        std::fs::write(
            repo.join("manifest.json"),
            r#"{"name":"test/publisher","version":"1","entry":"test.publisher.main"}"#,
        )
        .unwrap();
        write_wav(&repo.join("samples/drums/kick.wav"));
        index_package(&repo).unwrap();
        initialize_git_repository(&repo);

        let result =
            install_git_package(repo.to_str().unwrap(), "test/publisher", &installed).unwrap();
        let package = InstalledPackage::load(&result.path).unwrap();
        let mut runtime = eseqlisp::Runtime::new();
        runtime.set_scoped_module_load_path(vec![eseqlisp::ModuleLoadRoot {
            path: package.source_root.expect("package ships src"),
            module_prefix: Some(package.module_prefix),
        }]);
        let value = runtime
            .eval_str("(import test.publisher.main :as publisher)\n(publisher/answer)")
            .unwrap();
        assert_eq!(value, Some(eseqlisp::vm::Value::Number(42.0)));

        let store = root.join("store/samples");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        let mut db = SampleDb::open(&root.join("store/samples.db")).unwrap();
        let report =
            reconcile_installed_package_samples(&installed, &store, &mut db).unwrap();
        assert_eq!(report.ingested_origins, vec!["pkg:test.publisher"]);
        assert!(report.errors.is_empty());
        let rows = db
            .query(&[], &[], None, false, &["pkg:test.publisher"])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(store.join(format!("{}.wav", rows[0].hash)).is_file());

        std::fs::remove_dir_all(root).unwrap();
    }
}
