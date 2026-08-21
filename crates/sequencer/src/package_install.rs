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
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success());
        }

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
}
