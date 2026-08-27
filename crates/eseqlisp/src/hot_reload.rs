use crate::parser::{ASTParser, Expression, Parser};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const CWD_RELATIVE_LOAD_PREFIX: &str = "@/";

/// Process-wide fallback roots for relative `(load …)` paths that do not
/// resolve against the load stack or cwd. The host application installs its
/// content roots here (the sequencer's factory `content/` dir and its
/// parent), so user source that loads factory scripts by a repo-relative
/// path (`scripts/…`, `content/scripts/…`) keeps working regardless of the
/// process cwd. Empty (no fallback) unless the host installs roots.
static LOAD_FALLBACK_ROOTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();

pub fn set_global_load_fallback_roots(roots: Vec<PathBuf>) {
    let _ = LOAD_FALLBACK_ROOTS.set(roots);
}

/// Resolve a content-relative *asset* path the same way `resolve_load_path`
/// resolves a relative `(load …)`: cwd first, then the installed content
/// roots. Widgets that name a factory asset by its content-relative path
/// (`instruments/core/wavetable/waves/bank.json`) resolved against the
/// crate-dir cwd before the factory `content/` split and need this fallback
/// now. Absolute paths and paths that resolve against the cwd are returned
/// unchanged, so a host that never installs roots behaves exactly as before.
pub fn resolve_content_relative_asset(path: &str) -> PathBuf {
    let raw = PathBuf::from(path);
    if raw.is_absolute() || raw.exists() {
        return raw;
    }
    for root in load_fallback_roots() {
        let candidate = root.join(&raw);
        if candidate.exists() {
            return candidate;
        }
    }
    raw
}

/// When the host never installs roots, fall back to the dev-workspace layout
/// derived from this crate's manifest dir (same precedent as
/// `defmacro_library::default_library_root`), so tests and helper tools
/// resolve factory content without explicit setup. Missing dirs → no
/// fallback.
fn load_fallback_roots() -> &'static [PathBuf] {
    LOAD_FALLBACK_ROOTS
        .get_or_init(|| {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let content = workspace.join("content");
            if content.is_dir() {
                vec![
                    content.canonicalize().unwrap_or(content),
                    workspace.canonicalize().unwrap_or(workspace),
                ]
            } else {
                Vec::new()
            }
        })
        .as_slice()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub PathBuf);

#[derive(Debug, Clone)]
pub struct SourceOverlay {
    pub path: PathBuf,
    pub text: String,
    pub dirty: bool,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct SourceSnapshot {
    pub overlays: Vec<SourceOverlay>,
}

#[derive(Debug, Clone)]
pub struct LoadedSource {
    pub path: PathBuf,
    pub text: String,
    pub revision: u64,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStackEntry {
    pub path: PathBuf,
    pub revision: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleRecord {
    pub path: PathBuf,
    pub source_hash: u64,
    pub source_revision: u64,
    pub defined_symbols: HashSet<String>,
    pub children: HashSet<PathBuf>,
    pub parents: HashSet<PathBuf>,
    pub render_roots: HashSet<u32>,
    pub last_successful_diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleGraph {
    modules: HashMap<PathBuf, ModuleRecord>,
    revision: u64,
}

impl ModuleGraph {
    pub fn record_module(
        &mut self,
        path: PathBuf,
        source_hash: u64,
        source_revision: u64,
        defined_symbols: HashSet<String>,
        children: HashSet<PathBuf>,
        diagnostics: Vec<String>,
    ) -> HashSet<String> {
        let old = self
            .modules
            .get(&path)
            .cloned()
            .unwrap_or_else(|| ModuleRecord {
                path: path.clone(),
                ..ModuleRecord::default()
            });
        let mut changed = HashSet::new();
        if old.source_hash != source_hash {
            changed.extend(old.defined_symbols.iter().cloned());
            changed.extend(defined_symbols.iter().cloned());
        }

        for removed_child in old.children.difference(&children) {
            if let Some(child_record) = self.modules.get_mut(removed_child) {
                child_record.parents.remove(&path);
            }
        }
        for child in &children {
            self.modules
                .entry(child.clone())
                .or_insert_with(|| ModuleRecord {
                    path: child.clone(),
                    ..ModuleRecord::default()
                })
                .parents
                .insert(path.clone());
        }

        let record = self
            .modules
            .entry(path.clone())
            .or_insert_with(|| ModuleRecord {
                path,
                ..ModuleRecord::default()
            });
        record.source_hash = source_hash;
        record.source_revision = source_revision;
        record.defined_symbols = defined_symbols;
        record.children = children;
        record.last_successful_diagnostics = diagnostics;
        self.revision = self.revision.wrapping_add(1);
        changed
    }

    pub fn record_render_root(&mut self, module: &Path, node_id: u32) {
        self.modules
            .entry(module.to_path_buf())
            .or_insert_with(|| ModuleRecord {
                path: module.to_path_buf(),
                ..ModuleRecord::default()
            })
            .render_roots
            .insert(node_id);
    }

    pub fn owner_root_for(&self, path: &Path) -> Option<PathBuf> {
        let start = self.modules.get(path)?;
        if start.parents.is_empty() {
            return Some(path.to_path_buf());
        }

        let mut best = None;
        let mut queue = VecDeque::new();
        let mut seen = HashSet::new();
        queue.extend(start.parents.iter().cloned());
        while let Some(candidate) = queue.pop_front() {
            if !seen.insert(candidate.clone()) {
                continue;
            }
            let Some(record) = self.modules.get(&candidate) else {
                best.get_or_insert(candidate);
                continue;
            };
            if record.parents.is_empty() {
                best = Some(candidate);
                continue;
            }
            queue.extend(record.parents.iter().cloned());
            best.get_or_insert(candidate);
        }
        best
    }

    pub fn known_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.modules.keys().cloned().collect::<Vec<_>>();
        paths.sort();
        paths
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn debug_lines(&self) -> Vec<String> {
        let mut records = self.modules.values().collect::<Vec<_>>();
        records.sort_by(|a, b| a.path.cmp(&b.path));
        records
            .into_iter()
            .map(|record| {
                let mut children = record.children.iter().collect::<Vec<_>>();
                children.sort();
                format!(
                    "{} -> [{}]",
                    record.path.display(),
                    children
                        .into_iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct OverlayRecord {
    text: String,
    dirty: bool,
    revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleLoadRoot {
    pub path: PathBuf,
    /// When present, this root may resolve only this package's owned module
    /// namespace. The prefix is stripped before mapping the module to `src/`.
    pub module_prefix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourceManager {
    cwd: PathBuf,
    overlays: HashMap<PathBuf, OverlayRecord>,
    load_stack: Vec<PathBuf>,
    revision_stack: Vec<u64>,
    module_graph: ModuleGraph,
    pending_children: HashMap<PathBuf, HashSet<PathBuf>>,
    changed_symbols: HashSet<String>,
    diagnostics: Vec<String>,
    evaluated_sources: HashMap<(PathBuf, u64), String>,
    module_alias_scan_exclusions: Vec<PathBuf>,
    module_load_roots: Vec<ModuleLoadRoot>,
}

impl Default for SourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceManager {
    pub fn new() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            overlays: HashMap::new(),
            load_stack: Vec::new(),
            revision_stack: Vec::new(),
            module_graph: ModuleGraph::default(),
            pending_children: HashMap::new(),
            changed_symbols: HashSet::new(),
            diagnostics: Vec::new(),
            evaluated_sources: HashMap::new(),
            module_alias_scan_exclusions: Vec::new(),
            module_load_roots: Vec::new(),
        }
    }

    /// Override the `@/`-prefix root (defaults to the process cwd at
    /// construction). Tests use this to pin module→file resolution against
    /// a synthetic layout without touching the process-wide cwd.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    /// Set ordered roots used exclusively for module imports. Ordinary
    /// `(load …)` remains relative to the importing file. An empty list keeps
    /// the legacy candidate behavior for standalone embedders and tests.
    pub fn set_module_load_roots(&mut self, roots: Vec<PathBuf>) {
        self.set_scoped_module_load_roots(
            roots.into_iter().map(|path| ModuleLoadRoot { path, module_prefix: None }).collect()
        );
    }

    pub fn set_scoped_module_load_roots(&mut self, roots: Vec<ModuleLoadRoot>) {
        self.module_load_roots = roots
            .into_iter()
            .map(|root| ModuleLoadRoot {
                path: self.canonicalize_path(&root.path),
                module_prefix: root.module_prefix,
            })
            .collect();
    }

    /// Resolve a module against the configured tiered load path. Scoped
    /// package roots strip their owned prefix (`alec.tools.ui` → `ui.lisp`)
    /// and are never considered for another package's namespace.
    pub fn load_module_source(
        &mut self,
        module: &str,
        candidates: &[PathBuf],
    ) -> Option<Result<LoadedSource, Vec<String>>> {
        if self.module_load_roots.is_empty() {
            return None;
        }
        let roots = self.module_load_roots.clone();
        let mut errors = Vec::new();
        for root in roots {
            let package_candidates;
            let candidates = if let Some(prefix) = &root.module_prefix {
                let Some(suffix) = module.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('.')) else {
                    continue;
                };
                package_candidates = crate::modules::module_relative_file_candidates(suffix);
                package_candidates.as_slice()
            } else {
                candidates
            };
            for relative in candidates {
                let resolved = self.canonicalize_path(&root.path.join(relative));
                match self.source_for_canonical_path(resolved.clone()) {
                    Ok(source) => {
                        if let Some(parent) = self.load_stack.last().cloned() {
                            self.pending_children
                                .entry(parent)
                                .or_default()
                                .insert(resolved);
                        }
                        return Some(Ok(source));
                    }
                    Err(error) => errors.push(error),
                }
            }
        }
        Some(Err(errors))
    }

    /// Excludes a known factory-content root from legacy-alias preflight.
    /// Authored roots must not be registered here: future user roots should
    /// inherit detection automatically through normal path evaluation.
    pub fn exclude_module_alias_scan_root(&mut self, root: PathBuf) {
        let root = self.canonicalize_path(&root);
        if !self.module_alias_scan_exclusions.contains(&root) {
            self.module_alias_scan_exclusions.push(root);
        }
    }

    pub fn should_scan_module_aliases(&self, path: &Path) -> bool {
        !self
            .module_alias_scan_exclusions
            .iter()
            .any(|root| path.starts_with(root))
    }

    pub fn set_overlays(&mut self, overlays: Vec<SourceOverlay>) {
        self.overlays.clear();
        for overlay in overlays {
            let path = self.canonicalize_path(&overlay.path);
            self.overlays.insert(
                path,
                OverlayRecord {
                    text: overlay.text,
                    dirty: overlay.dirty,
                    revision: overlay.revision,
                },
            );
        }
    }

    pub fn begin_transaction(&mut self) {
        self.changed_symbols.clear();
        self.diagnostics.clear();
        self.pending_children.clear();
    }

    pub fn changed_symbols(&self) -> HashSet<String> {
        self.changed_symbols.clone()
    }

    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics.clone()
    }

    pub fn module_graph(&self) -> &ModuleGraph {
        &self.module_graph
    }

    pub fn canonicalize_path(&self, path: &Path) -> PathBuf {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        std::fs::canonicalize(&absolute).unwrap_or_else(|_| normalize_path(&absolute))
    }

    pub fn resolve_load_path(&self, path: &str) -> PathBuf {
        let raw = Path::new(path);
        if raw.is_absolute() {
            return self.canonicalize_path(raw);
        }
        if let Some(cwd_relative) = path.strip_prefix(CWD_RELATIVE_LOAD_PREFIX) {
            let primary = self.canonicalize_path(Path::new(cwd_relative));
            if primary.exists() || self.overlays.contains_key(&primary) {
                return primary;
            }
            for root in load_fallback_roots() {
                let candidate = root.join(cwd_relative);
                if candidate.exists() {
                    return self.canonicalize_path(&candidate);
                }
            }
            return primary;
        }
        let base = self
            .load_stack
            .last()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.cwd.clone());
        let primary = self.canonicalize_path(&base.join(raw));
        if primary.exists() || self.overlays.contains_key(&primary) {
            return primary;
        }
        for root in load_fallback_roots() {
            let candidate = root.join(raw);
            if candidate.exists() {
                return self.canonicalize_path(&candidate);
            }
        }
        primary
    }

    pub fn load_source(&mut self, path: &str) -> Result<LoadedSource, String> {
        let resolved = self.resolve_load_path(path);
        if let Some(parent) = self.load_stack.last().cloned() {
            self.pending_children
                .entry(parent)
                .or_default()
                .insert(resolved.clone());
        }
        self.source_for_canonical_path(resolved)
    }

    pub fn source_for_path(&self, path: &Path) -> Result<LoadedSource, String> {
        self.source_for_canonical_path(self.canonicalize_path(path))
    }

    fn source_for_canonical_path(&self, path: PathBuf) -> Result<LoadedSource, String> {
        if let Some(overlay) = self.overlays.get(&path) {
            return Ok(LoadedSource {
                path,
                text: overlay.text.clone(),
                revision: overlay.revision,
                dirty: overlay.dirty,
            });
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let revision = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_else(|| hash_source(&text));
        Ok(LoadedSource {
            path,
            text,
            revision,
            dirty: false,
        })
    }

    pub fn enter_file(&mut self, path: PathBuf, revision: u64) {
        self.pending_children.entry(path.clone()).or_default();
        self.load_stack.push(path);
        self.revision_stack.push(revision);
    }

    pub fn leave_file(&mut self) {
        let _ = self.load_stack.pop();
        let _ = self.revision_stack.pop();
    }

    pub fn current_source_file(&self) -> Option<PathBuf> {
        self.load_stack.last().cloned()
    }

    pub fn current_revision(&self) -> Option<u64> {
        self.revision_stack.last().copied()
    }

    pub fn source_stack_snapshot(&self) -> Vec<SourceStackEntry> {
        self.load_stack
            .iter()
            .cloned()
            .zip(self.revision_stack.iter().copied())
            .map(|(path, revision)| SourceStackEntry { path, revision })
            .collect()
    }

    pub fn restore_source_stack(&mut self, stack: Vec<SourceStackEntry>) {
        self.load_stack = stack.iter().map(|entry| entry.path.clone()).collect();
        self.revision_stack = stack.iter().map(|entry| entry.revision).collect();
    }

    pub fn remember_evaluated_source(&mut self, path: PathBuf, revision: u64, source: &str) {
        let path = self.canonicalize_path(&path);
        self.evaluated_sources
            .insert((path, revision), source.to_string());
    }

    pub fn evaluated_source(&self, path: &Path, revision: u64) -> Option<&str> {
        let path = self.canonicalize_path(path);
        self.evaluated_sources
            .get(&(path, revision))
            .map(String::as_str)
    }

    pub fn record_module_success(
        &mut self,
        path: PathBuf,
        source: &str,
        revision: u64,
        defined_symbols: HashSet<String>,
        diagnostics: Vec<String>,
    ) {
        let children = self.pending_children.remove(&path).unwrap_or_default();
        let changed = self.module_graph.record_module(
            path,
            hash_source(source),
            revision,
            defined_symbols,
            children,
            diagnostics,
        );
        self.changed_symbols.extend(changed);
    }

    pub fn discard_module_loads(&mut self, path: &Path) {
        self.pending_children.remove(path);
    }

    pub fn record_render_root(&mut self, node_id: u32) {
        if let Some(module) = self.current_source_file() {
            self.module_graph.record_render_root(&module, node_id);
        }
    }

    pub fn push_diagnostic(&mut self, diagnostic: impl Into<String>) {
        self.diagnostics.push(diagnostic.into());
    }

    pub fn owner_root_for(&self, path: &Path) -> Option<PathBuf> {
        self.module_graph.owner_root_for(path)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReloadReport {
    pub success: bool,
    pub evaluated_path: Option<PathBuf>,
    pub requested_path: Option<PathBuf>,
    pub changed_symbols: Vec<String>,
    pub rerendered_roots: Vec<String>,
    pub diagnostics: Vec<String>,
}

impl ReloadReport {
    pub fn success_message(&self) -> String {
        let changed = if self.changed_symbols.is_empty() {
            "no changed symbols".to_string()
        } else {
            format!("changed: {}", self.changed_symbols.join(", "))
        };
        let roots = if self.rerendered_roots.is_empty() {
            "no roots rerendered".to_string()
        } else {
            format!("rerendered: {}", self.rerendered_roots.join(", "))
        };
        format!("Lisp reload ok ({changed}; {roots})")
    }

    pub fn failure_message(&self) -> String {
        self.diagnostics
            .first()
            .cloned()
            .unwrap_or_else(|| "Lisp reload failed".to_string())
    }
}

pub fn hash_source(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

pub fn extract_defined_symbols_from_source(source: &str) -> Result<HashSet<String>, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("parse error: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("AST parse error: {error:?}"))?;
    let mut symbols = HashSet::new();
    for expr in &exprs {
        collect_defined_symbols(expr, &mut symbols);
    }
    Ok(symbols)
}

fn collect_defined_symbols(expr: &Expression, out: &mut HashSet<String>) {
    let Expression::List(items) = expr else {
        return;
    };
    match items.as_slice() {
        [Expression::Symbol(form), Expression::Symbol(name), ..]
            if matches!(
                form.as_str(),
                "def"
                    | "defn"
                    | "defstate"
                    | "defscene"
                    | "defmacro"
                    | "defwidget"
                    | "def-process"
                    | "def-accumulator"
                    | "def-sequencer"
                    | "defchan"
            ) =>
        {
            out.insert(name.clone());
        }
        [Expression::Symbol(form), Expression::String(name), ..] if form == "defhook" => {
            out.insert(name.clone());
        }
        [Expression::Symbol(form), Expression::Symbol(name), ..]
            if form == "override" || form == "remove-override" =>
        {
            // Overrides change the effective value of the factory symbol even
            // though they deliberately do not mutate its global cell. Mark it
            // changed so transactional init/hot reload rerenders dependents.
            out.insert(name.clone());
        }
        [Expression::Symbol(form), Expression::List(signature), ..] if form == "def" => {
            if let Some(Expression::Symbol(name)) = signature.first() {
                out.insert(name.clone());
            }
        }
        _ => {}
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_targets_are_effective_defined_symbols_for_hot_reload() {
        let symbols = extract_defined_symbols_from_source(
            "(override eseq.factory/view :around (original) (original))\n\
             (remove-override eseq.factory/other)",
        )
        .expect("extract symbols");
        assert!(symbols.contains("eseq.factory/view"));
        assert!(symbols.contains("eseq.factory/other"));
    }

    #[test]
    fn defscene_is_a_defined_symbol_for_hot_reload() {
        let symbols = extract_defined_symbols_from_source("(defscene figures '())")
            .expect("extract symbols");
        assert!(symbols.contains("figures"));
    }

    #[test]
    fn relative_load_falls_back_to_installed_content_roots() {
        let root = std::env::temp_dir().join(format!(
            "eseqlisp-load-fallback-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("scripts/demo-seq.lisp"), "(+ 1 2)\n").unwrap();
        set_global_load_fallback_roots(vec![root.clone()]);

        let mut manager = SourceManager::new();
        manager.cwd = std::env::temp_dir().join(format!(
            "eseqlisp-load-fallback-elsewhere-{}",
            std::process::id()
        ));

        assert_eq!(
            manager.resolve_load_path("scripts/demo-seq.lisp"),
            root.join("scripts/demo-seq.lisp").canonicalize().unwrap()
        );
        // Paths that resolve normally are untouched by the fallback.
        assert_eq!(
            manager.resolve_load_path("scripts/missing.lisp"),
            manager.cwd.join("scripts/missing.lisp")
        );
    }

    #[test]
    fn explicit_cwd_relative_load_ignores_the_active_module_directory() {
        let cwd =
            std::env::temp_dir().join(format!("eseqlisp-source-root-{}", std::process::id()));
        let mut manager = SourceManager::new();
        manager.cwd = cwd.clone();
        manager.enter_file(cwd.join("ui/main.lisp"), 1);

        // Names that exist nowhere (in particular not under the content
        // fallback roots), so the assertions see pure resolution semantics.
        assert_eq!(
            manager.resolve_load_path("@/ui/no-such-themes.lisp"),
            cwd.join("ui/no-such-themes.lisp")
        );
        assert_eq!(
            manager.resolve_load_path("no-such-themes.lisp"),
            cwd.join("ui/no-such-themes.lisp")
        );
    }
}
