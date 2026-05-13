use std::io;
use std::path::{Path, PathBuf};

pub fn enter_sequencer_dir() -> io::Result<()> {
    std::env::set_current_dir(sequencer_dir()?)
}

pub fn sequencer_dir() -> io::Result<PathBuf> {
    find_package_dir("sequencer", "SEQUENCER_ROOT", env!("CARGO_MANIFEST_DIR")).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "sequencer crate directory not found",
        )
    })
}

pub fn eseqlisp_init_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(root) = std::env::var("ESEQLISP_ROOT") {
        if !root.trim().is_empty() {
            paths.push(PathBuf::from(root).join("init.lisp"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("../eseqlisp/init.lisp"));
        paths.push(cwd.join("crates/eseqlisp/init.lisp"));
    }

    for root in repo_roots_from_current_exe() {
        paths.push(root.join("crates/eseqlisp/init.lisp"));
    }

    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../eseqlisp/init.lisp"));
    paths.push(PathBuf::from("../eseqlisp/init.lisp"));
    paths.push(PathBuf::from("init.lisp"));
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
