use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::parser::{ASTParser, Expression, Parser, format_expression};

const MACRO_SOURCE_FILE: &str = "macro.lisp";
const MACRO_LAYOUT_FILE: &str = "macro.layout.json";
const MACRO_MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefmacroManifest {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DefmacroPackage {
    pub name: String,
    pub package_dir: PathBuf,
    pub source_path: PathBuf,
    pub layout_path: PathBuf,
    pub manifest_path: PathBuf,
    pub source: String,
    pub macro_expr: Expression,
    pub params: Vec<String>,
    pub outputs: Vec<String>,
    pub imports: Vec<String>,
    pub manifest: DefmacroManifest,
}

#[derive(Debug, Clone)]
pub struct DefmacroLibrary {
    root: PathBuf,
    packages: BTreeMap<String, DefmacroPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedSource {
    pub source: String,
    pub imported_macros: Vec<String>,
    pub shadowed_imports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefmacroLibraryError {
    Io {
        path: PathBuf,
        message: String,
    },
    Parse {
        path: Option<PathBuf>,
        message: String,
    },
    InvalidPackage {
        path: PathBuf,
        message: String,
    },
    MissingMacro {
        name: String,
    },
    ImportCycle {
        chain: Vec<String>,
    },
}

impl fmt::Display for DefmacroLibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefmacroLibraryError::Io { path, message } => {
                write!(f, "failed to read '{}': {message}", path.display())
            }
            DefmacroLibraryError::Parse { path, message } => {
                if let Some(path) = path {
                    write!(f, "failed to parse '{}': {message}", path.display())
                } else {
                    write!(f, "failed to parse source: {message}")
                }
            }
            DefmacroLibraryError::InvalidPackage { path, message } => {
                write!(
                    f,
                    "invalid defmacro package '{}': {message}",
                    path.display()
                )
            }
            DefmacroLibraryError::MissingMacro { name } => {
                write!(f, "unknown library defmacro `{name}`")
            }
            DefmacroLibraryError::ImportCycle { chain } => {
                write!(f, "defmacro import cycle: {}", chain.join(" -> "))
            }
        }
    }
}

impl std::error::Error for DefmacroLibraryError {}

impl DefmacroLibrary {
    pub fn load(root: impl AsRef<Path>) -> Result<Self, DefmacroLibraryError> {
        let root = root.as_ref().to_path_buf();
        let mut packages = BTreeMap::new();
        if !root.exists() {
            return Ok(Self { root, packages });
        }
        let entries = fs::read_dir(&root).map_err(|error| DefmacroLibraryError::Io {
            path: root.clone(),
            message: error.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| DefmacroLibraryError::Io {
                path: root.clone(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            let package = DefmacroPackage::load(&path)?;
            if packages.insert(package.name.clone(), package).is_some() {
                return Err(DefmacroLibraryError::InvalidPackage {
                    path,
                    message: "duplicate public macro name".to_string(),
                });
            }
        }
        Ok(Self { root, packages })
    }

    pub fn empty(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            packages: BTreeMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn packages(&self) -> &BTreeMap<String, DefmacroPackage> {
        &self.packages
    }

    pub fn package(&self, name: &str) -> Option<&DefmacroPackage> {
        self.packages.get(name)
    }

    pub fn materialize_source(
        &self,
        source: &str,
    ) -> Result<MaterializedSource, DefmacroLibraryError> {
        let exprs = parse_exprs(source, None)?;
        let local_macros = local_macro_names(&exprs);
        let direct_imports = top_level_imports(&exprs)?;
        if direct_imports.is_empty() {
            return Ok(MaterializedSource {
                source: source.to_string(),
                imported_macros: Vec::new(),
                shadowed_imports: Vec::new(),
            });
        }
        let resolved = self.resolve_imports(&direct_imports, &local_macros)?;

        let imported_defs = resolved
            .packages
            .iter()
            .map(|package| format_expression(&package.macro_expr))
            .collect::<Vec<_>>();
        let user_forms = exprs
            .iter()
            .filter(|expr| !matches!(parse_use_defmacro(expr), Ok(Some(_))))
            .map(format_expression)
            .collect::<Vec<_>>();
        let source = imported_defs
            .into_iter()
            .chain(user_forms)
            .collect::<Vec<_>>()
            .join("\n");

        Ok(MaterializedSource {
            source,
            imported_macros: resolved
                .packages
                .iter()
                .map(|package| package.name.clone())
                .collect(),
            shadowed_imports: resolved.shadowed.into_iter().collect(),
        })
    }

    pub fn resolve_for_source(
        &self,
        source: &str,
    ) -> Result<ResolvedDefmacros<'_>, DefmacroLibraryError> {
        let exprs = parse_exprs(source, None)?;
        let local_macros = local_macro_names(&exprs);
        let direct_imports = top_level_imports(&exprs)?;
        self.resolve_imports(&direct_imports, &local_macros)
    }

    pub fn rebuild_manifest(&self, name: &str) -> Result<DefmacroManifest, DefmacroLibraryError> {
        let package =
            self.packages
                .get(name)
                .ok_or_else(|| DefmacroLibraryError::MissingMacro {
                    name: name.to_string(),
                })?;
        let manifest = package.rebuilt_manifest();
        let json = serde_json::to_string_pretty(&manifest).map_err(|error| {
            DefmacroLibraryError::InvalidPackage {
                path: package.package_dir.clone(),
                message: format!("failed to serialize manifest: {error}"),
            }
        })?;
        atomic_write(&package.manifest_path, &format!("{json}\n")).map_err(|error| {
            DefmacroLibraryError::Io {
                path: package.manifest_path.clone(),
                message: error.to_string(),
            }
        })?;
        Ok(manifest)
    }

    fn resolve_imports<'a>(
        &'a self,
        direct_imports: &[String],
        local_macros: &HashSet<String>,
    ) -> Result<ResolvedDefmacros<'a>, DefmacroLibraryError> {
        let mut state = ResolveState {
            library: self,
            local_macros,
            visiting: Vec::new(),
            visited: HashSet::new(),
            packages: Vec::new(),
            shadowed: BTreeSet::new(),
        };
        for import in direct_imports {
            state.resolve(import)?;
        }
        Ok(ResolvedDefmacros {
            packages: state.packages,
            shadowed: state.shadowed,
        })
    }
}

impl DefmacroPackage {
    pub fn load(package_dir: &Path) -> Result<Self, DefmacroLibraryError> {
        let package_name = package_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DefmacroLibraryError::InvalidPackage {
                path: package_dir.to_path_buf(),
                message: "package directory has no valid name".to_string(),
            })?
            .to_string();
        let source_path = package_dir.join(MACRO_SOURCE_FILE);
        let source =
            fs::read_to_string(&source_path).map_err(|error| DefmacroLibraryError::Io {
                path: source_path.clone(),
                message: error.to_string(),
            })?;
        Self::from_source(package_dir, &package_name, &source)
    }

    pub fn from_source(
        package_dir: &Path,
        package_name: &str,
        source: &str,
    ) -> Result<Self, DefmacroLibraryError> {
        let source_path = package_dir.join(MACRO_SOURCE_FILE);
        let layout_path = package_dir.join(MACRO_LAYOUT_FILE);
        let manifest_path = package_dir.join(MACRO_MANIFEST_FILE);
        let exprs = parse_exprs(source, Some(source_path.clone()))?;
        let macros = exprs.iter().filter_map(parse_defmacro).collect::<Vec<_>>();
        if macros.len() != 1 {
            return Err(DefmacroLibraryError::InvalidPackage {
                path: package_dir.to_path_buf(),
                message: format!(
                    "expected exactly one public defmacro, found {}",
                    macros.len()
                ),
            });
        }
        let parsed = macros[0].clone();
        if parsed.name != package_name {
            return Err(DefmacroLibraryError::InvalidPackage {
                path: package_dir.to_path_buf(),
                message: format!(
                    "public macro `{}` does not match package name `{package_name}`",
                    parsed.name
                ),
            });
        }
        let imports = top_level_imports(&exprs)?;
        let manifest = DefmacroManifest {
            version: 1,
            name: parsed.name.clone(),
            params: parsed.params.clone(),
            outputs: infer_macro_outputs(&parsed.body),
            summary: None,
            tags: Vec::new(),
        };
        Ok(Self {
            name: parsed.name,
            package_dir: package_dir.to_path_buf(),
            source_path,
            layout_path,
            manifest_path,
            source: source.to_string(),
            macro_expr: parsed.expr,
            params: parsed.params,
            outputs: manifest.outputs.clone(),
            imports,
            manifest,
        })
    }

    pub fn rebuilt_manifest(&self) -> DefmacroManifest {
        DefmacroManifest {
            version: 1,
            name: self.name.clone(),
            params: self.params.clone(),
            outputs: self.outputs.clone(),
            summary: self.manifest.summary.clone(),
            tags: self.manifest.tags.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedDefmacros<'a> {
    pub packages: Vec<&'a DefmacroPackage>,
    pub shadowed: BTreeSet<String>,
}

struct ResolveState<'a, 'b> {
    library: &'a DefmacroLibrary,
    local_macros: &'b HashSet<String>,
    visiting: Vec<String>,
    visited: HashSet<String>,
    packages: Vec<&'a DefmacroPackage>,
    shadowed: BTreeSet<String>,
}

impl<'a, 'b> ResolveState<'a, 'b> {
    fn resolve(&mut self, name: &str) -> Result<(), DefmacroLibraryError> {
        if self.local_macros.contains(name) {
            self.shadowed.insert(name.to_string());
            return Ok(());
        }
        if self.visited.contains(name) {
            return Ok(());
        }
        if let Some(position) = self.visiting.iter().position(|active| active == name) {
            let mut chain = self.visiting[position..].to_vec();
            chain.push(name.to_string());
            return Err(DefmacroLibraryError::ImportCycle { chain });
        }
        let package =
            self.library
                .package(name)
                .ok_or_else(|| DefmacroLibraryError::MissingMacro {
                    name: name.to_string(),
                })?;
        self.visiting.push(name.to_string());
        for import in &package.imports {
            self.resolve(import)?;
        }
        self.visiting.pop();
        self.visited.insert(name.to_string());
        self.packages.push(package);
        Ok(())
    }
}

#[derive(Clone)]
struct ParsedDefmacro {
    name: String,
    params: Vec<String>,
    body: Vec<Expression>,
    expr: Expression,
}

pub fn parse_use_defmacro(expr: &Expression) -> Result<Option<String>, DefmacroLibraryError> {
    let Expression::List(items) = expr else {
        return Ok(None);
    };
    if symbol_at(items, 0) != Some("use-defmacro") {
        return Ok(None);
    }
    if items.len() != 2 {
        return Err(DefmacroLibraryError::Parse {
            path: None,
            message: "`use-defmacro` must have exactly one symbol argument".to_string(),
        });
    }
    let Some(name) = symbol_at(items, 1) else {
        return Err(DefmacroLibraryError::Parse {
            path: None,
            message: "`use-defmacro` argument must be a symbol".to_string(),
        });
    };
    Ok(Some(name.to_string()))
}

pub fn top_level_imports(exprs: &[Expression]) -> Result<Vec<String>, DefmacroLibraryError> {
    let mut imports = Vec::new();
    for expr in exprs {
        if let Some(name) = parse_use_defmacro(expr)? {
            imports.push(name);
        }
    }
    imports.sort();
    imports.dedup();
    Ok(imports)
}

pub fn default_library_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut candidates = Vec::new();
    candidates.push(cwd.join("defmacros"));
    candidates.push(cwd.join("crates/sequencer/defmacros"));
    for ancestor in cwd.ancestors() {
        candidates.push(ancestor.join("crates/sequencer/defmacros"));
        candidates.push(ancestor.join("defmacros"));
    }
    candidates.into_iter().find(|path| path.is_dir())
}

pub fn materialize_with_default_library(source: &str) -> Result<String, DefmacroLibraryError> {
    let Some(root) = default_library_root() else {
        if source.contains("use-defmacro") {
            return DefmacroLibrary::empty("defmacros")
                .materialize_source(source)
                .map(|materialized| materialized.source);
        }
        return Ok(source.to_string());
    };
    let library = DefmacroLibrary::load(root)?;
    library
        .materialize_source(source)
        .map(|materialized| materialized.source)
}

fn parse_exprs(
    source: &str,
    path: Option<PathBuf>,
) -> Result<Vec<Expression>, DefmacroLibraryError> {
    let tokens =
        Parser::new(source.to_string())
            .parse()
            .map_err(|error| DefmacroLibraryError::Parse {
                path: path.clone(),
                message: format!("{error:?}"),
            })?;
    ASTParser::new(tokens)
        .parse()
        .map_err(|error| DefmacroLibraryError::Parse {
            path,
            message: format!("{error:?}"),
        })
}

fn parse_defmacro(expr: &Expression) -> Option<ParsedDefmacro> {
    let Expression::List(items) = expr else {
        return None;
    };
    if symbol_at(items, 0) != Some("defmacro") {
        return None;
    }
    let name = symbol_at(items, 1)?.to_string();
    let params = match items.get(2) {
        Some(Expression::List(params)) => params
            .iter()
            .map(|expr| match expr {
                Expression::Symbol(name) => Some(name.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    Some(ParsedDefmacro {
        name,
        params,
        body: items.iter().skip(3).cloned().collect(),
        expr: expr.clone(),
    })
}

fn local_macro_names(exprs: &[Expression]) -> HashSet<String> {
    exprs
        .iter()
        .filter_map(parse_defmacro)
        .map(|macro_def| macro_def.name)
        .collect()
}

fn infer_macro_outputs(body: &[Expression]) -> Vec<String> {
    let Some(return_expr) = body.last() else {
        return Vec::new();
    };
    let count = tuple_return_items(return_expr)
        .map(|items| items.len())
        .unwrap_or(1);
    (0..count)
        .map(|idx| {
            if idx == 0 {
                "out".to_string()
            } else {
                format!("out{}", idx + 1)
            }
        })
        .collect()
}

fn tuple_return_items(expr: &Expression) -> Option<Vec<&Expression>> {
    let Expression::List(items) = expr else {
        return None;
    };
    (symbol_at(items, 0) == Some("tuple")).then(|| items.iter().skip(1).collect())
}

fn symbol_at(items: &[Expression], idx: usize) -> Option<&str> {
    match items.get(idx) {
        Some(Expression::Symbol(symbol)) => Some(symbol),
        _ => None,
    }
}

fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_file_name(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("defmacro.tmp")
    ));
    fs::write(&tmp_path, contents)?;
    fs::rename(&tmp_path, path).or_else(|error| {
        let _ = fs::remove_file(&tmp_path);
        Err(error)
    })
}

pub fn library_macro_layout_file_name() -> &'static str {
    MACRO_LAYOUT_FILE
}

pub fn library_macro_source_file_name() -> &'static str {
    MACRO_SOURCE_FILE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eseq-defmacro-library-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_package(root: &Path, name: &str, source: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MACRO_SOURCE_FILE), source).unwrap();
    }

    #[test]
    fn materializes_direct_import() {
        let root = tmp_root("direct");
        write_package(&root, "gain2", "(defmacro gain2 (x) (* x 2))");
        let library = DefmacroLibrary::load(&root).unwrap();
        let materialized = library
            .materialize_source("(use-defmacro gain2)\n(def y (gain2 x))")
            .unwrap();
        assert_eq!(
            materialized.source,
            "(defmacro gain2 (x) (* x 2.0))\n(def y (gain2 x))"
        );
    }

    #[test]
    fn materializes_transitive_import_before_dependent_macro() {
        let root = tmp_root("transitive");
        write_package(&root, "pitch2freq", "(defmacro pitch2freq (p) (* p 2))");
        write_package(
            &root,
            "pluck",
            "(use-defmacro pitch2freq)\n(defmacro pluck (p) (pitch2freq p))",
        );
        let library = DefmacroLibrary::load(&root).unwrap();
        let materialized = library
            .materialize_source("(use-defmacro pluck)\n(def y (pluck x))")
            .unwrap();
        assert!(
            materialized
                .source
                .starts_with("(defmacro pitch2freq (p) (* p 2.0))\n(defmacro pluck")
        );
    }

    #[test]
    fn local_macro_shadows_library_import() {
        let root = tmp_root("shadow");
        write_package(&root, "gain2", "(defmacro gain2 (x) (* x 2))");
        let library = DefmacroLibrary::load(&root).unwrap();
        let materialized = library
            .materialize_source("(use-defmacro gain2)\n(defmacro gain2 (x) x)\n(def y (gain2 x))")
            .unwrap();
        assert_eq!(
            materialized.source,
            "(defmacro gain2 (x) x)\n(def y (gain2 x))"
        );
        assert_eq!(materialized.shadowed_imports, vec!["gain2".to_string()]);
    }

    #[test]
    fn detects_import_cycle() {
        let root = tmp_root("cycle");
        write_package(&root, "a", "(use-defmacro b)\n(defmacro a (x) (b x))");
        write_package(&root, "b", "(use-defmacro a)\n(defmacro b (x) (a x))");
        let library = DefmacroLibrary::load(&root).unwrap();
        let error = library.materialize_source("(use-defmacro a)").unwrap_err();
        assert!(matches!(error, DefmacroLibraryError::ImportCycle { .. }));
    }

    #[test]
    fn rejects_missing_import() {
        let root = tmp_root("missing");
        let library = DefmacroLibrary::load(&root).unwrap();
        let error = library
            .materialize_source("(use-defmacro nope)")
            .unwrap_err();
        assert_eq!(
            error,
            DefmacroLibraryError::MissingMacro {
                name: "nope".to_string()
            }
        );
    }

    #[test]
    fn rejects_package_name_mismatch() {
        let root = tmp_root("mismatch");
        write_package(&root, "expected", "(defmacro other (x) x)");
        let error = DefmacroLibrary::load(&root).unwrap_err();
        assert!(matches!(error, DefmacroLibraryError::InvalidPackage { .. }));
    }
}
