//! "My processes" (docs/expr-process-spec.md §8; bead eseq-waa9.17): the
//! user's own process package, where promote writes expr cards as classes.
//!
//! It is an ordinary user-tier package, `user/processes` (module prefix /
//! id prefix `user.processes`, `pkg:user.processes/…`), installed at
//! `<user_lisp_root>/packages/user.processes/` with its `def-process`
//! modules under `src/`. Nothing creates the package until something is
//! promoted. (The *processes* dock's library list that grouped classes by
//! source was removed in eseq-waa9.22; the add-process dropdown lists them.)

use std::path::{Path, PathBuf};


/// The package the user's own processes live in (spec §8, "My processes").
pub const MY_PROCESSES_PACKAGE: &str = "user/processes";

/// Module / id prefix of [`MY_PROCESSES_PACKAGE`].
pub const MY_PROCESSES_MODULE_PREFIX: &str = "user.processes";

thread_local! {
    static MY_PROCESSES_DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Restores the previous [`set_my_processes_package_dir_override`] value
/// when dropped, so a test's override never outlives its temp dir.
#[must_use = "the override lasts only while the guard lives"]
pub struct MyProcessesDirOverrideGuard {
    previous: Option<PathBuf>,
}

impl Drop for MyProcessesDirOverrideGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        MY_PROCESSES_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = previous);
    }
}

/// Point [`my_processes_package_dir`] somewhere else, on the calling thread
/// only, until the returned guard drops (`None` restores the app path). For
/// tests: promote writes files, and a test must never write into the real
/// `~/.eseq.d`. Thread-local, so tests sharing one process (plain `cargo
/// test`) cannot see each other's temp dirs; the promote natives run on the
/// thread that evaluates the Lisp.
pub fn set_my_processes_package_dir_override(dir: Option<PathBuf>) -> MyProcessesDirOverrideGuard {
    let previous = MY_PROCESSES_DIR_OVERRIDE.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), dir));
    MyProcessesDirOverrideGuard { previous }
}

/// The installed directory of [`MY_PROCESSES_PACKAGE`]:
/// `<user_lisp_root>/packages/user.processes/`.
pub fn my_processes_package_dir() -> PathBuf {
    if let Some(dir) = MY_PROCESSES_DIR_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return dir;
    }
    crate::app_paths::app_paths().packages_dir().join("user.processes")
}

/// Whether `source_path` (a def's source file) lies in the My processes
/// package: its classes are library declarations, like builtin.lisp's.
pub fn is_my_processes_source(source_path: &str) -> bool {
    !source_path.is_empty()
        && canonical(Path::new(source_path)).starts_with(canonical(&my_processes_package_dir()))
}

/// The My processes package's module files (`src/**/*.lisp`), sorted.
pub fn my_processes_module_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("lisp") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&my_processes_package_dir().join("src"), &mut files);
    files.sort();
    files
}

/// Source that loads every My processes module into a UI runtime, next to
/// `load_process_library_source` (spec §8: promoted classes exist on every
/// start, and survive a project switch like builtin.lisp's). Empty when the
/// package does not exist. Each module loads on its own, so one broken file
/// does not hide the rest.
pub fn load_my_processes_source() -> String {
    my_processes_module_files()
        .into_iter()
        .filter_map(|path| {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let text = path.to_string_lossy().into_owned();
            // eseqlisp strings have no escapes.
            (!text.contains('"')).then(|| format!("(load \"{text}\")"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `path` with symlinks resolved; a missing file resolves through its
/// parent, so `/var/…` and `/private/var/…` still compare equal.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical(parent).join(name),
        _ => path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn my_processes_is_a_valid_user_package() {
        assert_eq!(
            eseqlisp::package::validate_package_name(MY_PROCESSES_PACKAGE).as_deref(),
            Ok("user.processes")
        );
        assert!(my_processes_package_dir().ends_with("packages/user.processes"));
    }
}
