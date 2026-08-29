use std::io;
use std::path::{Path, PathBuf};

pub fn enter_sequencer_dir() -> io::Result<()> {
    std::env::set_current_dir(sequencer_dir()?)
}

pub fn sequencer_dir() -> io::Result<PathBuf> {
    find_package_dir("sequencer", "SEQUENCER_ROOT", env!("ESEQ_DEV_MANIFEST_DIR")).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "sequencer crate directory not found",
        )
    })
}

pub fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("ESEQ_DEV_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .unwrap_or(&manifest_dir)
        .to_path_buf()
}

pub fn project_scratch_source_path() -> PathBuf {
    crate::app_paths::app_paths().project_scratch_source_path()
}

/// User Lisp customization entrypoint from the process-wide, captured path
/// layout. The file is optional, but its root is always well-defined.
pub fn user_init_path() -> Option<PathBuf> {
    Some(crate::app_paths::app_paths().user_lisp_root().join("init.lisp"))
}

pub fn eseqlisp_init_candidates() -> Vec<PathBuf> {
    let app_paths = crate::app_paths::app_paths();
    let override_root = (!app_paths.is_release())
        .then(|| std::env::var("ESEQLISP_ROOT").ok())
        .flatten();
    eseqlisp_init_candidates_for(app_paths, override_root.as_deref())
}

fn eseqlisp_init_candidates_for(
    app_paths: &crate::app_paths::AppPaths,
    dev_override: Option<&str>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if !app_paths.is_release() {
        if let Some(root) = dev_override.filter(|root| !root.trim().is_empty()) {
            paths.push(PathBuf::from(root).join("init.lisp"));
        }
    }
    paths.push(app_paths.core_dir().join("init.lisp"));
    paths
}

fn find_package_dir(package: &str, env_var: &str, compile_manifest_dir: &str) -> Option<PathBuf> {
    if let Ok(root) = std::env::var(env_var) {
        if !root.trim().is_empty() {
            let path = PathBuf::from(root);
            if is_package_dir(&path, package) {
                return Some(path);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        if is_package_dir(&cwd, package) {
            return Some(cwd);
        }
        let workspace_member = cwd.join("crates").join(package);
        if is_package_dir(&workspace_member, package) {
            return Some(workspace_member);
        }
    }

    for root in repo_roots_from_current_exe() {
        let workspace_member = root.join("crates").join(package);
        if is_package_dir(&workspace_member, package) {
            return Some(workspace_member);
        }
    }

    let fallback = PathBuf::from(compile_manifest_dir);
    is_package_dir(&fallback, package).then_some(fallback)
}

fn repo_roots_from_current_exe() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    exe.ancestors()
        .filter(|path| path.join("crates").is_dir())
        .map(Path::to_path_buf)
        .collect()
}

fn is_package_dir(path: &Path, package: &str) -> bool {
    path.join("Cargo.toml").is_file() && path.file_name().is_some_and(|name| name == package)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_init_candidates_ignore_eseqlisp_root() {
        let paths = crate::app_paths::AppPaths::release(
            PathBuf::from("/ESeq.app/Contents/MacOS"),
            PathBuf::from("/ESeq.app/Contents/Resources"),
            PathBuf::from("/Support"),
            PathBuf::from("/Caches"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        assert_eq!(
            eseqlisp_init_candidates_for(&paths, Some("/checkout/crates/eseqlisp")),
            vec![PathBuf::from("/ESeq.app/Contents/Resources/core/init.lisp")]
        );
    }

    #[test]
    fn dev_init_candidates_still_honor_eseqlisp_root() {
        let paths = crate::app_paths::AppPaths::dev(
            PathBuf::from("/ws/crates/sequencer"),
            PathBuf::from("/ws"),
            PathBuf::from("/home/test/.eseq.d"),
        );
        assert_eq!(
            eseqlisp_init_candidates_for(&paths, Some("/ws/crates/eseqlisp")),
            vec![
                PathBuf::from("/ws/crates/eseqlisp/init.lisp"),
                PathBuf::from("/ws/content/core/init.lisp"),
            ]
        );
    }
}
