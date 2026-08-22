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
    /// Every public macro emitted by this package. `name` remains the primary
    /// patcher macro for existing libraries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<String>,
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
    pub macro_exprs: Vec<Expression>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateMacroUnquoteKind {
    Unquote,
    UnquoteSplicing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateMacroCompatibilityIssue {
    pub macro_name: String,
    pub kind: TemplateMacroUnquoteKind,
    pub expression: String,
}

impl fmt::Display for TemplateMacroCompatibilityIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operator = match self.kind {
            TemplateMacroUnquoteKind::Unquote => ",",
            TemplateMacroUnquoteKind::UnquoteSplicing => ",@",
        };
        write!(
            f,
            "defmacro `{}` uses {operator}{} instead of unquoting a parameter directly",
            self.macro_name, self.expression
        )
    }
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
        let (library, errors) = Self::load_available(root)?;
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        Ok(library)
    }

    pub fn load_available(
        root: impl AsRef<Path>,
    ) -> Result<(Self, Vec<DefmacroLibraryError>), DefmacroLibraryError> {
        let root = root.as_ref().to_path_buf();
        let mut packages = BTreeMap::new();
        let mut errors = Vec::new();
        if !root.exists() {
            return Ok((Self { root, packages }, errors));
        }
        let entries = fs::read_dir(&root).map_err(|error| DefmacroLibraryError::Io {
            path: root.clone(),
            message: error.to_string(),
        })?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(DefmacroLibraryError::Io {
                        path: root.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
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
            let package = match DefmacroPackage::load(&path) {
                Ok(package) => package,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            if packages.contains_key(&package.name) {
                errors.push(DefmacroLibraryError::InvalidPackage {
                    path,
                    message: format!("duplicate public macro name `{}`", package.name),
                });
                continue;
            }
            packages.insert(package.name.clone(), package);
        }
        Ok((Self { root, packages }, errors))
    }

    pub fn load_for_source(
        root: impl AsRef<Path>,
        source: &str,
    ) -> Result<Self, DefmacroLibraryError> {
        let root = root.as_ref().to_path_buf();
        let exprs = parse_exprs(source, None)?;
        let local_macros = local_macro_names(&exprs);
        let direct_imports = top_level_imports(&exprs)?;
        Self::load_import_closure(root, &direct_imports, &local_macros)
    }

    fn load_import_closure(
        root: PathBuf,
        direct_imports: &[String],
        local_macros: &HashSet<String>,
    ) -> Result<Self, DefmacroLibraryError> {
        let mut packages = BTreeMap::new();
        let mut visiting = Vec::new();
        let mut visited = HashSet::new();
        for import in direct_imports {
            load_package_dependency(
                &root,
                import,
                local_macros,
                &mut visiting,
                &mut visited,
                &mut packages,
            )?;
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

    pub fn with_package_source(
        &self,
        name: &str,
        source: &str,
    ) -> Result<Self, DefmacroLibraryError> {
        let existing =
            self.packages
                .get(name)
                .ok_or_else(|| DefmacroLibraryError::MissingMacro {
                    name: name.to_string(),
                })?;
        let package = DefmacroPackage::from_source(&existing.package_dir, name, source)?;
        let mut packages = self.packages.clone();
        packages.insert(name.to_string(), package);
        Ok(Self {
            root: self.root.clone(),
            packages,
        })
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
        let mut resolved = self.resolve_imports(&direct_imports, &local_macros)?;

        let imported_defs = resolved
            .packages
            .iter()
            .flat_map(|package| package.macro_exprs.iter())
            .filter_map(|expression| {
                let parsed = parse_defmacro(expression)?;
                if local_macros.contains(&parsed.name) {
                    resolved.shadowed.insert(parsed.name);
                    None
                } else {
                    Some(format_expression(expression))
                }
            })
            .collect::<Vec<_>>();
        let user_source = remove_top_level_use_defmacro_forms(source);
        let source = if imported_defs.is_empty() {
            user_source
        } else if user_source.trim().is_empty() {
            imported_defs.join("\n")
        } else {
            format!("{}\n{}", imported_defs.join("\n"), user_source)
        };

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
        if macros.is_empty() {
            return Err(DefmacroLibraryError::InvalidPackage {
                path: package_dir.to_path_buf(),
                message: "expected at least one public defmacro".to_string(),
            });
        }
        let Some(parsed) = macros.iter().find(|macro_def| macro_def.name == package_name).cloned() else {
            return Err(DefmacroLibraryError::InvalidPackage {
                path: package_dir.to_path_buf(),
                message: format!("package must define its primary macro `{package_name}`"),
            });
        };
        let mut exports = macros.iter().map(|macro_def| macro_def.name.clone()).collect::<Vec<_>>();
        exports.sort();
        exports.dedup();
        if exports.len() != macros.len() {
            return Err(DefmacroLibraryError::InvalidPackage {
                path: package_dir.to_path_buf(),
                message: "duplicate public defmacro name".to_string(),
            });
        }
        let imports = top_level_imports(&exprs)?;
        let manifest = DefmacroManifest {
            version: 1,
            name: parsed.name.clone(),
            params: parsed.params.clone(),
            outputs: infer_macro_outputs(&parsed.body),
            exports,
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
            macro_exprs: macros.into_iter().map(|macro_def| macro_def.expr).collect(),
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
            exports: self.macro_exprs.iter().filter_map(parse_defmacro).map(|macro_def| macro_def.name).collect(),
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

static DEFAULT_LIBRARY_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Install the application's factory defmacro root. The embedding application
/// owns filesystem layout; the eseqlisp crate retains a source-tree fallback
/// only for standalone tools and tests.
pub fn set_default_library_root(root: PathBuf) {
    let _ = DEFAULT_LIBRARY_ROOT.set(root);
}

pub fn default_library_root() -> Option<PathBuf> {
    if let Some(root) = DEFAULT_LIBRARY_ROOT.get() {
        return root.is_dir().then(|| root.clone());
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.join("../sequencer/defmacros"),
        manifest_dir.join("../../content/defmacros"),
    ];
    candidates.into_iter().find(|path| path.is_dir())
}

pub fn materialize_with_default_library(source: &str) -> Result<String, DefmacroLibraryError> {
    let exprs = parse_exprs(source, None)?;
    let direct_imports = top_level_imports(&exprs)?;
    if direct_imports.is_empty() {
        return Ok(source.to_string());
    }
    let Some(root) = default_library_root() else {
        return DefmacroLibrary::empty("defmacros")
            .materialize_source(source)
            .map(|materialized| materialized.source);
    };
    let library = DefmacroLibrary::load_for_source(root, source)?;
    library
        .materialize_source(source)
        .map(|materialized| materialized.source)
}

fn load_package_dependency(
    root: &Path,
    name: &str,
    local_macros: &HashSet<String>,
    visiting: &mut Vec<String>,
    visited: &mut HashSet<String>,
    packages: &mut BTreeMap<String, DefmacroPackage>,
) -> Result<(), DefmacroLibraryError> {
    if local_macros.contains(name) || visited.contains(name) {
        return Ok(());
    }
    if let Some(position) = visiting.iter().position(|active| active == name) {
        let mut chain = visiting[position..].to_vec();
        chain.push(name.to_string());
        return Err(DefmacroLibraryError::ImportCycle { chain });
    }
    let package_dir = root.join(name);
    if !package_dir.exists() {
        return Err(DefmacroLibraryError::MissingMacro {
            name: name.to_string(),
        });
    }
    let package = DefmacroPackage::load(&package_dir)?;
    visiting.push(name.to_string());
    for import in &package.imports {
        load_package_dependency(root, import, local_macros, visiting, visited, packages)?;
    }
    visiting.pop();
    visited.insert(name.to_string());
    packages.insert(package.name.clone(), package);
    Ok(())
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

/// Finds template macros whose expansion changes when defmacro bodies become
/// evaluated code. The legacy expander substitutes only a directly unquoted
/// parameter: it copies other unquotes into the expansion and rejects other
/// unquote-splices. Procedural expansion evaluates both kinds instead.
pub fn lint_template_macro_compatibility(
    source: &str,
) -> Result<Vec<TemplateMacroCompatibilityIssue>, DefmacroLibraryError> {
    let exprs = parse_exprs(source, None)?;
    let mut issues = Vec::new();
    for macro_def in exprs.iter().filter_map(parse_defmacro) {
        let params = macro_def
            .params
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        for body in &macro_def.body {
            lint_template_expression(body, &macro_def.name, &params, &mut issues);
        }
    }
    Ok(issues)
}

fn lint_template_expression(
    expression: &Expression,
    macro_name: &str,
    params: &HashSet<&str>,
    issues: &mut Vec<TemplateMacroCompatibilityIssue>,
) {
    let mut pending = vec![(expression, 0usize)];
    while let Some((expression, quasiquote_depth)) = pending.pop() {
        match expression {
            Expression::Quasiquote(inner) => pending.push((inner, quasiquote_depth + 1)),
            Expression::Unquote(inner) | Expression::UnquoteSplicing(inner) => {
                if quasiquote_depth == 1 {
                    let directly_unquotes_parameter = matches!(
                        inner.as_ref(),
                        Expression::Symbol(name) if params.contains(name.as_str())
                    );
                    if !directly_unquotes_parameter {
                        issues.push(TemplateMacroCompatibilityIssue {
                            macro_name: macro_name.to_string(),
                            kind: if matches!(expression, Expression::Unquote(_)) {
                                TemplateMacroUnquoteKind::Unquote
                            } else {
                                TemplateMacroUnquoteKind::UnquoteSplicing
                            },
                            expression: format_expression(inner),
                        });
                    }
                    // The unquoted expression executes outside this quasiquote;
                    // inspect any quasiquotes that its computation contains.
                    pending.push((inner, 0));
                } else if quasiquote_depth > 1 {
                    pending.push((inner, quasiquote_depth - 1));
                } else {
                    pending.push((inner, 0));
                }
            }
            Expression::List(items) => {
                pending.extend(items.iter().rev().map(|item| (item, quasiquote_depth)));
            }
            Expression::QuoteList(items) if quasiquote_depth > 0 => {
                // The legacy template expander descends into quote lists while
                // processing a surrounding quasiquote, so preserve that model.
                pending.extend(items.iter().rev().map(|item| (item, quasiquote_depth)));
            }
            Expression::Symbol(_)
            | Expression::Keyword(_)
            | Expression::String(_)
            | Expression::QuoteSymbol(_)
            | Expression::Number(_)
            | Expression::QuoteList(_) => {}
        }
    }
}

/// True when a macro body contains a quasiquote, i.e. it is a substitution
/// template rather than a plain DGenLisp body. Only these can depend on the
/// legacy unquote fallback that `lint_template_macro_compatibility` reports.
fn body_uses_quasiquote(expression: &Expression) -> bool {
    let mut pending = vec![expression];
    while let Some(expression) = pending.pop() {
        match expression {
            Expression::Quasiquote(_) => return true,
            Expression::Unquote(inner) | Expression::UnquoteSplicing(inner) => pending.push(inner),
            Expression::List(items) | Expression::QuoteList(items) => pending.extend(items),
            _ => {}
        }
    }
    false
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

fn remove_top_level_use_defmacro_forms(source: &str) -> String {
    let spans = top_level_use_defmacro_spans(source);
    if spans.is_empty() {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end) in spans {
        out.push_str(&source[cursor..start]);
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    trim_repeated_blank_lines(&out)
}

fn top_level_use_defmacro_spans(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut idx = 0;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
            idx += 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }
        match byte {
            b';' => {
                in_comment = true;
                idx += 1;
            }
            b'"' => {
                in_string = true;
                idx += 1;
            }
            b'(' if depth == 0 => {
                let start = idx;
                if let Some(end) = matching_top_level_form_end(source, start) {
                    if form_is_use_defmacro(&source[start..end]) {
                        spans.push(expand_removed_form_span(source, start, end));
                    }
                    idx = end;
                } else {
                    idx += 1;
                }
            }
            b'(' => {
                depth += 1;
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    spans
}

fn matching_top_level_form_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut idx = start;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
            idx += 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }
        match byte {
            b';' => in_comment = true,
            b'"' => in_string = true,
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx + 1);
                }
            }
            _ => {}
        }
        idx += 1;
    }
    None
}

fn form_is_use_defmacro(source: &str) -> bool {
    parse_exprs(source, None)
        .ok()
        .and_then(|exprs| exprs.into_iter().next())
        .and_then(|expr| parse_use_defmacro(&expr).ok().flatten())
        .is_some()
}

fn expand_removed_form_span(source: &str, start: usize, end: usize) -> (usize, usize) {
    let bytes = source.as_bytes();
    let mut remove_end = end;
    while remove_end < bytes.len() && matches!(bytes[remove_end], b' ' | b'\t' | b'\r') {
        remove_end += 1;
    }
    if remove_end < bytes.len() && bytes[remove_end] == b'\n' {
        remove_end += 1;
    }
    let mut remove_start = start;
    while remove_start > 0 && matches!(bytes[remove_start - 1], b' ' | b'\t') {
        remove_start -= 1;
    }
    (remove_start, remove_end)
}

fn trim_repeated_blank_lines(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut blank_count = 0usize;
    for line in source.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                out.push('\n');
            }
        } else {
            blank_count = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    if !source.ends_with('\n') {
        out.pop();
    }
    out
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
    fn one_defmacro_package_can_export_multiple_symbols() {
        let root = tmp_root("multi-symbol");
        write_package(
            &root,
            "acid",
            "(defmacro acid-helper (x) (* x 2))\n(defmacro acid (x) (acid-helper x))",
        );
        let library = DefmacroLibrary::load(&root).unwrap();
        let package = library.package("acid").unwrap();
        assert_eq!(package.manifest.exports, vec!["acid", "acid-helper"]);
        let materialized = library.materialize_source("(use-defmacro acid)\n(def y (acid x))").unwrap();
        assert!(materialized.source.contains("(defmacro acid-helper"));
        assert!(materialized.source.contains("(defmacro acid"));
    }

    #[test]
    fn materialization_preserves_user_source_number_text() {
        let root = tmp_root("preserve-number-text");
        write_package(&root, "gain2", "(defmacro gain2 (x) (* x 2))");
        let library = DefmacroLibrary::load(&root).unwrap();
        let source =
            "(use-defmacro gain2)\n(def mod1 (in 6 @name mod1 @modulator 1))\n(def y (gain2 mod1))";
        let materialized = library.materialize_source(source).unwrap();

        assert!(materialized.source.contains("@modulator 1)"));
        assert!(!materialized.source.contains("@modulator 1.0)"));
        assert!(!materialized.source.contains("use-defmacro"));
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
    fn load_for_source_ignores_invalid_unrelated_packages() {
        let root = tmp_root("load-for-source-unrelated-invalid");
        write_package(&root, "gain2", "(defmacro gain2 (x) (* x 2))");
        write_package(&root, "broken", "(use-defmacro gain2)");

        let strict_error = DefmacroLibrary::load(&root).unwrap_err();
        assert!(matches!(
            strict_error,
            DefmacroLibraryError::InvalidPackage { .. }
        ));

        let library =
            DefmacroLibrary::load_for_source(&root, "(use-defmacro gain2)\n(def y (gain2 x))")
                .unwrap();
        let materialized = library
            .materialize_source("(use-defmacro gain2)\n(def y (gain2 x))")
            .unwrap();
        assert_eq!(
            materialized.source,
            "(defmacro gain2 (x) (* x 2.0))\n(def y (gain2 x))"
        );
    }

    #[test]
    fn load_available_keeps_valid_packages_and_reports_invalid_packages() {
        let root = tmp_root("load-available");
        write_package(&root, "gain2", "(defmacro gain2 (x) (* x 2))");
        write_package(&root, "broken", "(use-defmacro gain2)");

        let (library, errors) = DefmacroLibrary::load_available(&root).unwrap();
        assert!(library.package("gain2").is_some());
        assert!(library.package("broken").is_none());
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            errors.first(),
            Some(DefmacroLibraryError::InvalidPackage { .. })
        ));
    }

    #[test]
    fn load_for_source_with_no_imports_does_not_scan_invalid_packages() {
        let root = tmp_root("load-for-source-no-imports");
        write_package(&root, "broken", "(use-defmacro nope)");

        let library = DefmacroLibrary::load_for_source(&root, "(def y (* x 2))").unwrap();
        assert!(library.packages().is_empty());
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

    #[test]
    fn template_macro_lint_reports_unquoted_non_parameters() {
        let issues = lint_template_macro_compatibility(
            "(defmacro unsafe (x &rest xs) `(list ,x ,@xs ,helper ,(first xs)))",
        )
        .unwrap();

        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|issue| {
            issue.macro_name == "unsafe"
                && issue.kind == TemplateMacroUnquoteKind::Unquote
                && issue.expression == "helper"
        }));
        assert!(issues.iter().any(|issue| {
            issue.macro_name == "unsafe"
                && issue.kind == TemplateMacroUnquoteKind::Unquote
                && issue.expression == "(first xs)"
        }));
    }

    #[test]
    fn template_macro_lint_reports_computed_unquote_splices() {
        // The legacy expander refuses a splice of anything but a bound
        // parameter, so the whole macro call is left unexpanded today;
        // procedural expansion would evaluate and splice it instead.
        let issues = lint_template_macro_compatibility(
            "(defmacro spliced (x) `(list ,@(rest x) ,@globals))",
        )
        .unwrap();

        assert_eq!(issues.len(), 2);
        assert!(
            issues
                .iter()
                .all(|issue| issue.kind == TemplateMacroUnquoteKind::UnquoteSplicing)
        );
        assert!(issues.iter().any(|issue| issue.expression == "(rest x)"));
        assert!(issues.iter().any(|issue| issue.expression == "globals"));
    }

    #[test]
    fn template_macro_lint_accepts_direct_parameter_unquotes() {
        let issues = lint_template_macro_compatibility(
            "(defmacro safe (head &rest tail) `(list ,head ,@tail))",
        )
        .unwrap();

        assert!(issues.is_empty());
    }

    #[test]
    fn checked_in_template_macros_are_procedural_expansion_compatible() {
        fn lisp_files_below(root: &Path) -> Vec<PathBuf> {
            let mut pending = vec![root.to_path_buf()];
            let mut files = Vec::new();
            while let Some(path) = pending.pop() {
                for entry in fs::read_dir(&path).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        pending.push(path);
                    } else if path.extension().and_then(|ext| ext.to_str()) == Some("lisp") {
                        files.push(path);
                    }
                }
            }
            files.sort();
            files
        }

        // Every checked-in Lisp tree: `content/` covers packages, UI, scripts and
        // the patcher's `content/defmacros` library (the only on-disk root
        // `defmacro_library_root` resolves to), while `crates/` covers the
        // examples, musicplayer and shipped effect sources that live next to
        // the Rust.
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut audited_macros = 0usize;
        let mut template_macros = 0usize;
        let mut failures = Vec::new();
        // `content` and `crates` must always carry macros; `docs` is linted too
        // but legitimately holds only example programs.
        for (root, expect_macros) in [("content", true), ("crates", true), ("docs", false)] {
            let mut macros_in_root = 0usize;
            for path in lisp_files_below(&repo_root.join(root)) {
                let source = fs::read_to_string(&path).unwrap();
                let exprs = parse_exprs(&source, Some(path.clone())).unwrap();
                for macro_def in exprs.iter().filter_map(parse_defmacro) {
                    macros_in_root += 1;
                    if macro_def.body.iter().any(body_uses_quasiquote) {
                        template_macros += 1;
                    }
                }
                for issue in lint_template_macro_compatibility(&source).unwrap() {
                    failures.push(format!("{}: {issue}", path.display()));
                }
            }
            assert!(
                macros_in_root > 0 || !expect_macros,
                "audit found no defmacros under {root}/ — has the tree moved?"
            );
            audited_macros += macros_in_root;
        }

        assert!(
            audited_macros >= 400,
            "audit unexpectedly found only {audited_macros} macros"
        );
        // Only quasiquote bodies can rely on the substitution fallback, so this
        // is the population the lint actually clears; the rest are DGenLisp DSP
        // macros with plain bodies. Guard it separately so a walk that silently
        // stops reaching `content/ui` or `content/packages` still fails.
        assert!(
            template_macros >= 40,
            "audit found only {template_macros} quasiquote-bodied macros"
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
