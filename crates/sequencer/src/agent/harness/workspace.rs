//! What the agent built: the Local packages a conversation wrote, each with
//! the files it had before the agent's first write, so the pane can show
//! what changed (lines added and removed) and the user can rename a new
//! package or reject the work (its files go back to how they were).
//!
//! A package is a folder under the Local packages root: module `tidal.view`
//! is `<root>/tidal/view.lisp`, in package `tidal`. Only its module files
//! (`.lisp`, not hidden, not links) are tracked: they are all the write
//! tools write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::agent::forms;

/// The tools that write module files, each naming its module in `module`.
pub const WRITE_TOOLS: &[&str] = &["create_module", "define_form", "write_module"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub path: PathBuf,
    /// The path inside the package, `view.lisp`.
    pub name: String,
    pub change: Change,
    /// Lines now (0 once deleted).
    pub lines: usize,
    pub added: usize,
    pub removed: usize,
}

/// A kind a package declares (`def-kind`), which the pane offers to add.
#[derive(Debug, Clone, PartialEq)]
pub struct KindDecl {
    pub module: String,
    pub kind: String,
}

/// One package as the pane shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Artifact {
    pub package: String,
    /// The agent made the package (it did not exist before), so it can be
    /// renamed and rejecting it deletes it.
    pub created: bool,
    /// Changed files only (for a new package, every file).
    pub files: Vec<FileChange>,
    /// The package's modules no other module of it imports: what loading it
    /// into the project attaches.
    pub entries: Vec<String>,
    pub kinds: Vec<KindDecl>,
}

impl Artifact {
    /// Lines in the package's changed files now.
    pub fn lines(&self) -> usize {
        self.files.iter().map(|file| file.lines).sum()
    }

    pub fn added(&self) -> usize {
        self.files.iter().map(|file| file.added).sum()
    }

    pub fn removed(&self) -> usize {
        self.files.iter().map(|file| file.removed).sum()
    }
}

struct Tracked {
    package: String,
    /// The package's files before the first write, by path inside it;
    /// empty for a package the agent made.
    baseline: BTreeMap<PathBuf, Vec<u8>>,
    created: bool,
}

pub struct Workspace {
    root: PathBuf,
    tracked: Vec<Tracked>,
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        Self { root, tracked: Vec::new() }
    }

    /// Forget what was tracked (the files stay): a new conversation.
    pub fn clear(&mut self) {
        self.tracked.clear();
    }

    /// Before `tool` writes `module`: remember its package as it is now,
    /// once. `create_module` writes under the root; the other write tools
    /// edit wherever the module is, so only a module already here counts
    /// (an installed package's is not tracked).
    pub fn before_write(&mut self, tool: &str, module: &str) {
        let Some(package) = module.split('.').next() else { return };
        if !eseqlisp::modules::is_valid_module_name(module) || self.find(package).is_some() {
            return;
        }
        let file = self.root.join(format!("{}.lisp", module.replace('.', "/")));
        if tool != "create_module" && !file.is_file() {
            return;
        }
        // A folder that exists is the user's, whatever is in it: rejecting
        // restores its modules, never deletes it.
        let dir = self.root.join(package);
        let created = !dir.exists();
        let baseline = read_files(&dir);
        self.tracked.push(Tracked { package: package.to_string(), baseline, created });
    }

    /// Every tracked package that changed, as it stands on disk now.
    pub fn artifacts(&self) -> Vec<Artifact> {
        self.tracked.iter().filter_map(|tracked| self.artifact(tracked)).collect()
    }

    fn artifact(&self, tracked: &Tracked) -> Option<Artifact> {
        let dir = self.root.join(&tracked.package);
        let current = read_files(&dir);
        let mut files = Vec::new();
        for (path, now) in &current {
            let change = match tracked.baseline.get(path) {
                None => Change::Added,
                Some(before) if before != now => Change::Modified,
                Some(_) => continue,
            };
            let now = String::from_utf8_lossy(now);
            let lines = now.lines().count();
            let (added, removed) = match tracked.baseline.get(path) {
                Some(before) => line_changes(&String::from_utf8_lossy(before), &now),
                None => (lines, 0),
            };
            files.push(FileChange { path: dir.join(path), name: path.display().to_string(), change, lines, added, removed });
        }
        for (path, before) in &tracked.baseline {
            if !current.contains_key(path) {
                let removed = String::from_utf8_lossy(before).lines().count();
                files.push(FileChange {
                    path: dir.join(path),
                    name: path.display().to_string(),
                    change: Change::Deleted,
                    lines: 0,
                    added: 0,
                    removed,
                });
            }
        }
        if files.is_empty() {
            return None;
        }
        let (entries, kinds) = package_modules(&current);
        Some(Artifact { package: tracked.package.clone(), created: tracked.created, files, entries, kinds })
    }

    /// Put the package's files back as they were before the agent wrote:
    /// a package it made is deleted.
    pub fn reject(&mut self, package: &str) -> Result<String, String> {
        let index = self.find(package).ok_or_else(|| format!("No changes to `{package}` to reject."))?;
        let tracked = &self.tracked[index];
        let dir = self.root.join(package);
        if tracked.created {
            match std::fs::remove_dir_all(&dir) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("cannot delete {}: {error}", dir.display()));
                }
                _ => {}
            }
        } else {
            let current = read_files(&dir);
            let mut problems = Vec::new();
            for path in current.keys().filter(|path| !tracked.baseline.contains_key(*path)) {
                if let Err(error) = std::fs::remove_file(dir.join(path)) {
                    problems.push(format!("cannot delete {}: {error}", path.display()));
                }
            }
            for (path, before) in &tracked.baseline {
                if current.get(path) == Some(before) {
                    continue;
                }
                let file = dir.join(path);
                let restored = file
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&file, before));
                if let Err(error) = restored {
                    problems.push(format!("cannot restore {}: {error}", path.display()));
                }
            }
            if !problems.is_empty() {
                return Err(format!("`{package}` was only partly restored: {}", problems.join("; ")));
            }
        }
        let created = tracked.created;
        self.tracked.remove(index);
        Ok(if created {
            format!("Deleted the package `{package}`.")
        } else {
            format!("Put `{package}` back as it was before the agent's changes.")
        })
    }

    /// Rename a package the agent made: its folder, and its module names in
    /// every file of it (`old.view` becomes `new.view`).
    pub fn rename(&mut self, package: &str, new_name: &str) -> Result<String, String> {
        let new_name = new_name.trim();
        let index = self.find(package).ok_or_else(|| format!("`{package}` is not a package the agent made."))?;
        if !self.tracked[index].created {
            return Err(format!("`{package}` existed before; only a package the agent made can be renamed here."));
        }
        if !valid_name(new_name) {
            return Err("A package name is lowercase letters, digits and dashes, starting with a letter.".to_string());
        }
        if new_name == "eseq" {
            return Err("`eseq` is the factory's name.".to_string());
        }
        let (from, to) = (self.root.join(package), self.root.join(new_name));
        if to.exists() || self.find(new_name).is_some() {
            return Err(format!("There is already a Local package called `{new_name}`."));
        }
        std::fs::rename(&from, &to).map_err(|error| format!("cannot rename {}: {error}", from.display()))?;
        // The folder is renamed: track it under its new name even if a
        // file below cannot be rewritten.
        self.tracked[index].package = new_name.to_string();
        for (path, bytes) in read_files(&to) {
            let source = String::from_utf8_lossy(&bytes);
            let renamed = rename_modules(&source, package, new_name);
            if renamed != source {
                let file = to.join(path);
                std::fs::write(&file, renamed).map_err(|error| format!("cannot write {}: {error}", file.display()))?;
            }
        }
        Ok(format!("Renamed `{package}` to `{new_name}`."))
    }

    fn find(&self, package: &str) -> Option<usize> {
        self.tracked.iter().position(|tracked| tracked.package == package)
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_lisp(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "lisp")
}

/// The module files under `dir`, by path inside it: `.lisp` files, not
/// hidden ones, and never through a link.
fn read_files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, base: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(items) = std::fs::read_dir(dir) else { return };
        for item in items.flatten() {
            let (path, Ok(kind)) = (item.path(), item.file_type()) else { continue };
            if item.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if kind.is_dir() {
                walk(&path, base, files);
            } else if kind.is_file() && is_lisp(&path) {
                if let Ok(bytes) = std::fs::read(&path) {
                    files.insert(path.strip_prefix(base).unwrap_or(&path).to_path_buf(), bytes);
                }
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

fn line_changes(before: &str, after: &str) -> (usize, usize) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(before, after);
    diff.iter_all_changes().fold((0, 0), |(added, removed), change| match change.tag() {
        ChangeTag::Insert => (added + 1, removed),
        ChangeTag::Delete => (added, removed + 1),
        ChangeTag::Equal => (added, removed),
    })
}

/// The package's entry modules (imported by none of its other modules) and
/// its kinds.
fn package_modules(files: &BTreeMap<PathBuf, Vec<u8>>) -> (Vec<String>, Vec<KindDecl>) {
    let mut modules = Vec::new();
    let mut imported = std::collections::HashSet::new();
    let mut kinds = Vec::new();
    for bytes in files.values() {
        let source = String::from_utf8_lossy(bytes);
        let Some(module) = forms::declared_module(&source) else { continue };
        imported.extend(crate::app::import_modules(&source));
        for form in forms::top_level_forms(&source).unwrap_or_default() {
            if let (Some(kind), "def-kind") = (form.name, form.head.as_str()) {
                kinds.push(KindDecl { module: module.clone(), kind });
            }
        }
        modules.push(module);
    }
    let entries = modules.into_iter().filter(|module| !imported.contains(module)).collect();
    (entries, kinds)
}

/// `source` with every module name in package `from` moved to `to`:
/// `from.x` becomes `to.x` wherever it starts a symbol.
fn rename_modules(source: &str, from: &str, to: &str) -> String {
    let prefix = format!("{from}.");
    let symbol_char = |c: char| c.is_alphanumeric() || "-_./:*+!?<>=$%&~^".contains(c);
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(&prefix) {
        let starts_symbol = !rest[..at].chars().next_back().is_some_and(symbol_char)
            && (at > 0 || out.chars().next_back().is_none_or(|c| !symbol_char(c)));
        let continues = rest[at + prefix.len()..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
        out.push_str(&rest[..at]);
        out.push_str(if starts_symbol && continues { to } else { from });
        out.push('.');
        rest = &rest[at + prefix.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_new_package_shows_its_files_kinds_and_entry_and_rejecting_deletes_it() {
        let root = tempfile::tempdir().unwrap();
        let mut workspace = Workspace::new(root.path().to_path_buf());
        workspace.before_write("create_module", "ring.core");
        write(&root.path().join("ring/core.lisp"), "(module ring.core)\n(def a 1)\n");
        workspace.before_write("create_module", "ring.view");
        write(&root.path().join("ring/view.lisp"), "(module ring.view)\n(import ring.core)\n(def-kind ring :doc \"x\")\n");
        let artifacts = workspace.artifacts();
        assert_eq!(artifacts.len(), 1);
        let ring = &artifacts[0];
        assert!(ring.created);
        assert_eq!(ring.entries, vec!["ring.view".to_string()]);
        assert_eq!(ring.kinds, vec![KindDecl { module: "ring.view".into(), kind: "ring".into() }]);
        assert_eq!(
            ring.files.iter().map(|f| (f.name.as_str(), f.change, f.lines, f.added)).collect::<Vec<_>>(),
            vec![("core.lisp", Change::Added, 2, 2), ("view.lisp", Change::Added, 3, 3)]
        );
        workspace.reject("ring").unwrap();
        assert!(!root.path().join("ring").exists());
        assert!(workspace.artifacts().is_empty());
    }

    #[test]
    fn an_edited_package_counts_changed_lines_and_rejecting_restores_it() {
        let root = tempfile::tempdir().unwrap();
        let core = root.path().join("tidal/core.lisp");
        write(&core, "(module tidal.core)\n(def a 1)\n(def b 2)\n");
        write(&root.path().join("tidal/notes.txt"), "mine");
        write(&root.path().join("tidal/.DS_Store"), "x");
        let mut workspace = Workspace::new(root.path().to_path_buf());
        workspace.before_write("define_form", "installed.pkg");
        workspace.before_write("define_form", "tidal.core");
        assert!(workspace.artifacts().is_empty(), "nothing changed yet");
        write(&core, "(module tidal.core)\n(def a 10)\n(def b 2)\n(def c 3)\n");
        write(&root.path().join("tidal/extra.lisp"), "(module tidal.extra)\n");
        write(&root.path().join("tidal/.DS_Store"), "changed");
        let artifact = &workspace.artifacts()[0];
        assert!(!artifact.created);
        assert_eq!(
            artifact.files.iter().map(|f| (f.name.as_str(), f.change, f.added, f.removed)).collect::<Vec<_>>(),
            vec![("core.lisp", Change::Modified, 2, 1), ("extra.lisp", Change::Added, 1, 0)]
        );
        assert!(workspace.rename("tidal", "cycles").is_err(), "an existing package keeps its name");
        workspace.reject("tidal").unwrap();
        assert_eq!(std::fs::read_to_string(&core).unwrap(), "(module tidal.core)\n(def a 1)\n(def b 2)\n");
        assert!(!root.path().join("tidal/extra.lisp").exists());
        assert!(root.path().join("tidal/notes.txt").exists());
        assert!(root.path().join("tidal/.DS_Store").exists());
    }

    #[test]
    fn renaming_a_new_package_moves_its_folder_and_module_names() {
        let root = tempfile::tempdir().unwrap();
        let mut workspace = Workspace::new(root.path().to_path_buf());
        workspace.before_write("create_module", "ring.view");
        write(
            &root.path().join("ring/view.lisp"),
            "(module ring.view)\n(import ring.core :as c)\n(def x (ring.core/f \"ring.core\" my-ring.core string.x))\n",
        );
        assert!(workspace.rename("ring", "Bad Name").is_err());
        workspace.rename("ring", "halo").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("halo/view.lisp")).unwrap(),
            "(module halo.view)\n(import halo.core :as c)\n(def x (halo.core/f \"halo.core\" my-ring.core string.x))\n"
        );
        assert_eq!(workspace.artifacts()[0].package, "halo");
        workspace.before_write("create_module", "ring.core");
        assert!(workspace.rename("halo", "ring").is_err(), "a tracked name is taken");
        assert!(!root.path().join("ring").exists());
    }
}
