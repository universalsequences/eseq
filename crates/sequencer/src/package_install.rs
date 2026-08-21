//! Atomic git-backed installation for Lisp/content packages.

use std::path::{Path, PathBuf};
use std::process::Command;

use eseqlisp::package::InstalledPackage;

#[derive(Debug, Clone)]
pub struct InstalledPackageResult {
    pub identity: String,
    pub path: PathBuf,
}

/// Clone and validate a package before making it visible to the package scan.
/// Failed clones and invalid manifests leave no installed directory behind.
pub fn install_git_package(
    repository: &str,
    expected_identity: &str,
    packages_dir: &Path,
) -> Result<InstalledPackageResult, String> {
    let expected_prefix = eseqlisp::package::validate_package_name(expected_identity)?;
    std::fs::create_dir_all(packages_dir)
        .map_err(|error| format!("failed to create {}: {error}", packages_dir.display()))?;
    let destination = packages_dir.join(&expected_prefix);
    if destination.exists() {
        return Err(format!(
            "package `{expected_identity}` is already installed"
        ));
    }
    let staging = packages_dir.join(format!(
        ".install-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let cleanup = |message: String| {
        let _ = std::fs::remove_dir_all(&staging);
        Err(message)
    };

    let output = match Command::new("git")
        .args(["clone", "--quiet", "--", repository])
        .arg(&staging)
        .output()
    {
        Ok(output) => output,
        Err(error) => return cleanup(format!("failed to launch git clone: {error}")),
    };
    if !output.status.success() {
        return cleanup(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let package = match InstalledPackage::load(&staging) {
        Ok(package) => package,
        Err(error) => return cleanup(error.to_string()),
    };
    if package.manifest.name != expected_identity {
        return cleanup(format!(
            "repository declares package `{}`, expected `{expected_identity}`",
            package.manifest.name
        ));
    }
    if package.module_prefix != expected_prefix {
        return cleanup("package identity produced an inconsistent module namespace".into());
    }
    if let Err(error) = std::fs::rename(&staging, &destination) {
        return cleanup(format!(
            "failed to publish package at {}: {error}",
            destination.display()
        ));
    }
    Ok(InstalledPackageResult {
        identity: expected_identity.to_string(),
        path: destination,
    })
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
            path: package.source_root,
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
