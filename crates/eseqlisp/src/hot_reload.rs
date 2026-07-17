use crate::parser::{ASTParser, Expression, Parser};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const CWD_RELATIVE_LOAD_PREFIX: &str = "@/";

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
pub struct ModuleStackEntry {
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
        }
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
            return self.canonicalize_path(Path::new(cwd_relative));
        }
        let base = self
            .load_stack
            .last()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.cwd.clone());
        self.canonicalize_path(&base.join(raw))
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

    pub fn enter_module(&mut self, path: PathBuf, revision: u64) {
        self.pending_children.entry(path.clone()).or_default();
        self.load_stack.push(path);
        self.revision_stack.push(revision);
    }

    pub fn leave_module(&mut self) {
        let _ = self.load_stack.pop();
        let _ = self.revision_stack.pop();
    }

    pub fn current_module(&self) -> Option<PathBuf> {
        self.load_stack.last().cloned()
    }

    pub fn current_revision(&self) -> Option<u64> {
        self.revision_stack.last().copied()
    }

    pub fn module_stack_snapshot(&self) -> Vec<ModuleStackEntry> {
        self.load_stack
            .iter()
            .cloned()
            .zip(self.revision_stack.iter().copied())
            .map(|(path, revision)| ModuleStackEntry { path, revision })
            .collect()
    }

    pub fn restore_module_stack(&mut self, stack: Vec<ModuleStackEntry>) {
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
        if let Some(module) = self.current_module() {
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
        [Expression::Symbol(form), Expression::Symbol(name), ..] if form == "def" => {
            out.insert(name.clone());
        }
        [Expression::Symbol(form), Expression::Symbol(name), ..] if form == "defstate" => {
            out.insert(name.clone());
        }
        [Expression::Symbol(form), Expression::Symbol(name), ..] if form == "defmacro" => {
            out.insert(name.clone());
        }
        [Expression::Symbol(form), Expression::Symbol(name), ..] if form == "defwidget" => {
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
    fn explicit_cwd_relative_load_ignores_the_active_module_directory() {
        let cwd = std::env::temp_dir().join("eseqlisp-source-root");
        let mut manager = SourceManager::new();
        manager.cwd = cwd.clone();
        manager.enter_module(cwd.join("ui/main.lisp"), 1);

        assert_eq!(
            manager.resolve_load_path("@/ui/themes.lisp"),
            cwd.join("ui/themes.lisp")
        );
        assert_eq!(
            manager.resolve_load_path("themes.lisp"),
            cwd.join("ui/themes.lisp")
        );
    }
}
