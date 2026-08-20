//! Source-preserving migration from `%` privacy to explicit module exports.
//!
//! The migration is intentionally module-scoped: bare private names are only
//! rewritten in their defining module, while consumers are changed only at
//! qualified references (including imported aliases). Comments are untouched
//! and possible name references in strings are reported for manual review.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::parser::{Expr, ExprKind, Parser, SpannedASTParser};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualOccurrence {
    pub line: usize,
    pub column: usize,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMigration {
    pub path: PathBuf,
    pub original: String,
    pub rewritten: String,
    pub replacements: usize,
    pub manual: Vec<ManualOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationResult {
    pub files: Vec<FileMigration>,
    pub modules: Vec<String>,
}

impl MigrationResult {
    pub fn changed_files(&self) -> impl Iterator<Item = &FileMigration> {
        self.files
            .iter()
            .filter(|file| file.original != file.rewritten)
    }

    pub fn changed_file_count(&self) -> usize {
        self.changed_files().count()
    }

    pub fn replacement_count(&self) -> usize {
        self.files.iter().map(|file| file.replacements).sum()
    }

    pub fn manual_count(&self) -> usize {
        self.files.iter().map(|file| file.manual.len()).sum()
    }
}

#[derive(Debug, Clone)]
struct Definition {
    name: String,
    string_span: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct SourceAnalysis {
    module: Option<String>,
    definitions: Vec<Definition>,
    aliases: HashMap<String, String>,
    insertion_anchor: Option<usize>,
    has_export: bool,
}

fn parse_source(source: &str) -> Result<Vec<Expr>, String> {
    let tokens = Parser::new(source.to_string())
        .parse_spanned()
        .map_err(|error| format!("parse error: {error:?}"))?;
    SpannedASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("AST parse error: {error:?}"))
}

fn symbol(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Symbol(value) => Some(value),
        _ => None,
    }
}

fn analyze(source: &str) -> Result<SourceAnalysis, String> {
    let expressions = parse_source(source)?;
    let mut module = None;
    let mut definitions = Vec::new();
    let mut aliases = HashMap::new();
    let mut insertion_anchor = None;
    let mut has_export = false;

    for expression in &expressions {
        let ExprKind::List(items) = &expression.kind else {
            continue;
        };
        let Some(form) = items.first().and_then(symbol) else {
            continue;
        };
        match form {
            "module" => {
                if let Some(name) = items.get(1).and_then(symbol) {
                    module = Some(name.to_string());
                    insertion_anchor = Some(expression.origin.primary_span.end_byte);
                }
            }
            "import" => {
                insertion_anchor = Some(expression.origin.primary_span.end_byte);
                let Some(imported) = items.get(1).and_then(symbol) else {
                    continue;
                };
                let mut index = 2;
                while index + 1 < items.len() {
                    match (&items[index].kind, &items[index + 1].kind) {
                        (ExprKind::Keyword(keyword), ExprKind::Symbol(alias))
                            if keyword == "as" =>
                        {
                            aliases.insert(alias.clone(), imported.to_string());
                            index += 2;
                        }
                        _ => index += 1,
                    }
                }
            }
            "export" => has_export = true,
            "def" | "defn" | "defstate" | "defmacro" => {
                let Some(name_expr) = items.get(1) else {
                    continue;
                };
                let name = match &name_expr.kind {
                    ExprKind::Symbol(name) => Some(name.clone()),
                    ExprKind::List(signature) if form == "def" => {
                        signature.first().and_then(symbol).map(str::to_string)
                    }
                    _ => None,
                };
                if let Some(name) = name {
                    definitions.push(Definition {
                        name,
                        string_span: None,
                    });
                }
            }
            "defhook" => {
                if let Some(name_expr) = items.get(1)
                    && let ExprKind::String(name) = &name_expr.kind
                {
                    // The parser span includes the quotes. This one string is
                    // structural definition syntax and is therefore safe to
                    // migrate; all other strings remain manual-review items.
                    let span = &name_expr.origin.primary_span;
                    definitions.push(Definition {
                        name: name.clone(),
                        string_span: Some((span.start_byte + 1, span.end_byte - 1)),
                    });
                }
            }
            _ => {}
        }
    }

    Ok(SourceAnalysis {
        module,
        definitions,
        aliases,
        insertion_anchor,
        has_export,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Symbol,
    String,
    Comment,
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

fn is_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b'\'' | b'`' | b',' | b';'
        )
}

fn scan_tokens(source: &str) -> Result<Vec<Token>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        let kind = match bytes[index] {
            b';' => {
                index = source[index..]
                    .find('\n')
                    .map_or(bytes.len(), |offset| index + offset);
                TokenKind::Comment
            }
            b'"' => {
                index += 1;
                let mut escaped = false;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                }
                if bytes.get(index.saturating_sub(1)) != Some(&b'"') {
                    return Err("unterminated string".to_string());
                }
                TokenKind::String
            }
            byte if is_delimiter(byte) => {
                index += 1;
                TokenKind::Other
            }
            _ => {
                index += 1;
                while index < bytes.len() && !is_delimiter(bytes[index]) {
                    index += 1;
                }
                TokenKind::Symbol
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: index,
        });
    }
    Ok(tokens)
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rfind('\n')
        .map_or(offset + 1, |newline| offset - newline);
    (line, column)
}

fn format_export(names: &[String]) -> String {
    if names.is_empty() {
        return "\n\n(export)".to_string();
    }
    let mut block = format!("\n\n(export {}", names[0]);
    for name in &names[1..] {
        block.push_str("\n        ");
        block.push_str(name);
    }
    block.push(')');
    block
}

fn selected(selector: &str, module: &str) -> bool {
    module == selector
        || module
            .strip_prefix(selector)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn collect_path(path: &Path, files: &mut Vec<PathBuf>, explicit: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to replace symbolic link: {}", path.display()),
        ));
    }
    if metadata.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == "lisp")
        {
            files.push(path.to_path_buf());
        } else if explicit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a .lisp file: {}", path.display()),
            ));
        }
    } else if metadata.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            collect_path(&entry.path(), files, false)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ModulePlan {
    public: Vec<String>,
    private: BTreeMap<String, String>,
    hook_spans: HashSet<(usize, usize)>,
    insertion_anchor: Option<usize>,
    has_export: bool,
}

pub fn plan_migration(selector: &str, paths: &[PathBuf]) -> io::Result<MigrationResult> {
    if selector.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "module selector is empty",
        ));
    }
    let mut paths_to_scan = Vec::new();
    for path in paths {
        collect_path(path, &mut paths_to_scan, true)?;
    }
    paths_to_scan.sort();
    paths_to_scan.dedup();

    let mut sources = Vec::with_capacity(paths_to_scan.len());
    for path in paths_to_scan {
        let source = fs::read_to_string(&path)?;
        let analysis = analyze(&source).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        })?;
        sources.push((path, source, analysis));
    }

    let mut module_plans = HashMap::new();
    for (path, _, analysis) in &sources {
        let Some(module) = analysis
            .module
            .as_deref()
            .filter(|module| selected(selector, module))
        else {
            continue;
        };
        if module_plans.contains_key(module) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("module {module} is declared by more than one input file"),
            ));
        }
        let mut public = Vec::new();
        let mut seen_public = HashSet::new();
        let mut private = BTreeMap::new();
        let mut bare = BTreeSet::new();
        let mut hook_spans = HashSet::new();
        for definition in &analysis.definitions {
            if let Some(stripped) = definition.name.strip_prefix('%') {
                if stripped.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "{}: cannot strip the empty private name `%`",
                            path.display()
                        ),
                    ));
                }
                private.insert(definition.name.clone(), stripped.to_string());
                if let Some(span) = definition.string_span {
                    hook_spans.insert(span);
                }
            } else {
                bare.insert(definition.name.clone());
                if seen_public.insert(definition.name.clone()) {
                    public.push(definition.name.clone());
                }
            }
        }
        let collisions = private
            .iter()
            .filter(|(_, stripped)| bare.contains(*stripped))
            .map(|(private, bare)| format!("  {private} -> {bare}"))
            .collect::<Vec<_>>();
        if !collisions.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing to convert module {module}: stripping private names collides with existing definitions:\n{}",
                    collisions.join("\n")
                ),
            ));
        }
        module_plans.insert(
            module.to_string(),
            ModulePlan {
                public,
                private,
                hook_spans,
                insertion_anchor: analysis.insertion_anchor,
                has_export: analysis.has_export,
            },
        );
    }
    if module_plans.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no module matching {selector} was found in the input paths"),
        ));
    }

    let mut files = Vec::with_capacity(sources.len());
    for (path, original, analysis) in sources {
        let mut replacements = Vec::<(usize, usize, String)>::new();
        let mut manual = Vec::new();
        let own = analysis
            .module
            .as_ref()
            .and_then(|module| module_plans.get(module));

        let mut qualified = BTreeMap::new();
        for (module, plan) in &module_plans {
            for (old, new) in &plan.private {
                qualified.insert(format!("{module}/{old}"), format!("{module}/{new}"));
                for (alias, imported) in &analysis.aliases {
                    if imported == module {
                        qualified.insert(format!("{alias}/{old}"), format!("{alias}/{new}"));
                    }
                }
            }
        }

        for token in scan_tokens(&original).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        })? {
            match token.kind {
                TokenKind::Symbol => {
                    let text = &original[token.start..token.end];
                    let replacement = own
                        .and_then(|plan| plan.private.get(text))
                        .cloned()
                        .or_else(|| qualified.get(text).cloned());
                    if let Some(replacement) = replacement {
                        replacements.push((token.start, token.end, replacement));
                    }
                }
                TokenKind::String => {
                    let inner = (token.start + 1, token.end - 1);
                    if let Some(plan) = own
                        && plan.hook_spans.contains(&inner)
                    {
                        let text = &original[inner.0..inner.1];
                        if let Some(replacement) = plan.private.get(text) {
                            replacements.push((inner.0, inner.1, replacement.clone()));
                            continue;
                        }
                    }
                    let text = &original[inner.0..inner.1];
                    let mut candidates = qualified.clone();
                    if let Some(plan) = own {
                        candidates.extend(plan.private.clone());
                    }
                    for (old, new) in candidates {
                        if let Some(relative) = text.find(&old) {
                            let offset = inner.0 + relative;
                            let (line, column) = line_column(&original, offset);
                            manual.push(ManualOccurrence {
                                line,
                                column,
                                old,
                                new,
                            });
                        }
                    }
                }
                TokenKind::Comment | TokenKind::Other => {}
            }
        }

        if let Some(plan) = own
            && !plan.has_export
        {
            let anchor = plan.insertion_anchor.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{}: selected module has no module declaration",
                        path.display()
                    ),
                )
            })?;
            replacements.push((anchor, anchor, format_export(&plan.public)));
        }

        replacements.sort_by_key(|replacement| (replacement.0, replacement.1));
        for pair in replacements.windows(2) {
            if pair[0].1 > pair[1].0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: overlapping migration edits", path.display()),
                ));
            }
        }
        let replacement_count = replacements.len();
        let mut rewritten = original.clone();
        for (start, end, replacement) in replacements.into_iter().rev() {
            rewritten.replace_range(start..end, &replacement);
        }
        manual.sort_by_key(|hit| (hit.line, hit.column, hit.old.clone()));
        manual.dedup();
        files.push(FileMigration {
            path,
            original,
            rewritten,
            replacements: replacement_count,
            manual,
        });
    }

    let mut modules = module_plans.keys().cloned().collect::<Vec<_>>();
    modules.sort();
    Ok(MigrationResult { files, modules })
}

fn adjacent_path(path: &Path, role: &str, ordinal: usize) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lisp");
    for attempt in 0..1000 {
        let candidate = parent.join(format!(
            ".{name}.eseq-export-migration-{role}-{}-{ordinal}-{attempt}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot allocate migration staging path",
    ))
}

struct StagedFile {
    path: PathBuf,
    temporary: PathBuf,
    backup: PathBuf,
}

fn rollback(files: &[StagedFile]) -> io::Result<()> {
    let mut errors = Vec::new();
    for file in files.iter().rev() {
        let _ = fs::remove_file(&file.path);
        if let Err(error) = fs::rename(&file.backup, &file.path) {
            errors.push(format!("restore {}: {error}", file.path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}

pub fn apply_migration(result: &MigrationResult) -> io::Result<()> {
    let changed = result.changed_files().collect::<Vec<_>>();
    let mut staged: Vec<StagedFile> = Vec::with_capacity(changed.len());
    for (ordinal, migration) in changed.iter().enumerate() {
        let temporary = adjacent_path(&migration.path, "new", ordinal)?;
        let backup = adjacent_path(&migration.path, "backup", ordinal)?;
        let stage_result = (|| {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            output.write_all(migration.rewritten.as_bytes())?;
            fs::set_permissions(&temporary, fs::metadata(&migration.path)?.permissions())?;
            output.sync_all()
        })();
        if let Err(error) = stage_result {
            let _ = fs::remove_file(&temporary);
            for file in &staged {
                let _ = fs::remove_file(&file.temporary);
            }
            return Err(error);
        }
        staged.push(StagedFile {
            path: migration.path.clone(),
            temporary,
            backup,
        });
    }

    let mut committed = 0;
    for file in &staged {
        if let Err(error) = fs::hard_link(&file.path, &file.backup) {
            for file in &staged {
                let _ = fs::remove_file(&file.temporary);
            }
            let rollback_error = rollback(&staged[..committed]).err();
            return Err(io::Error::other(format!(
                "failed to back up {}: {error}{}",
                file.path.display(),
                rollback_error.map_or(String::new(), |error| format!("; {error}"))
            )));
        }
        committed += 1;
        if let Err(error) = fs::rename(&file.temporary, &file.path) {
            for file in &staged {
                let _ = fs::remove_file(&file.temporary);
            }
            let rollback_error = rollback(&staged[..committed]).err();
            return Err(io::Error::other(format!(
                "failed to replace {}: {error}{}",
                file.path.display(),
                rollback_error.map_or(String::new(), |error| format!("; {error}"))
            )));
        }
    }
    for file in &staged {
        fs::remove_file(&file.backup)?;
    }
    Ok(())
}

pub fn unified_diff(file: &FileMigration) -> String {
    if file.original == file.rewritten {
        return String::new();
    }
    similar::TextDiff::from_lines(&file.original, &file.rewritten)
        .unified_diff()
        .context_radius(3)
        .header(
            &file.path.display().to_string(),
            &file.path.display().to_string(),
        )
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-module-export-migration-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn migration_generates_exports_and_rewrites_only_owned_and_qualified_names() {
        let dir = temp_dir("rewrite");
        let owner = dir.join("owner.lisp");
        let consumer = dir.join("consumer.lisp");
        fs::write(
            &owner,
            ";; %hidden comment\n(module test.owner)\n(import dep.core)\n\n(def public (x) (%hidden x))\n(def %hidden (x) x)\n(defn %private-fn (x) x)\n(defn public-fn (x) x)\n(defstate %cell 1)\n(defmacro visible-macro (x) x)\n(defhook \"published-hook\")\n(defhook \"%changed\")\n(run-hook \"%changed\")\n",
        ).unwrap();
        fs::write(
            &consumer,
            "(module test.consumer)\n(import test.owner :as own)\n(def use () (list (own/%hidden 1) test.owner/%cell foreign/%hidden))\n(def names () \"test.owner/%hidden\")\n",
        ).unwrap();

        let result = plan_migration("test.owner", &[dir.clone()]).unwrap();
        assert_eq!(result.modules, vec!["test.owner"]);
        let owner = result.files.iter().find(|file| file.path == owner).unwrap();
        assert!(owner.rewritten.contains(
            "(export public\n        public-fn\n        visible-macro\n        published-hook)"
        ));
        assert!(owner.rewritten.contains("(def hidden (x) x)"));
        assert!(owner.rewritten.contains("(defn private-fn (x) x)"));
        assert!(owner.rewritten.contains("(defhook \"changed\")"));
        assert!(owner.rewritten.contains("(run-hook \"%changed\")"));
        assert!(owner.rewritten.contains(";; %hidden comment"));
        assert_eq!(owner.manual.len(), 1);

        let consumer = result
            .files
            .iter()
            .find(|file| file.path == consumer)
            .unwrap();
        assert!(consumer.rewritten.contains("own/hidden"));
        assert!(consumer.rewritten.contains("test.owner/cell"));
        assert!(consumer.rewritten.contains("foreign/%hidden"));
        assert_eq!(consumer.manual.len(), 1);
        assert!(unified_diff(owner).contains("+        published-hook)"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn collision_refuses_the_entire_plan_and_names_the_pair() {
        let dir = temp_dir("collision");
        let path = dir.join("owner.lisp");
        fs::write(&path, "(module test.owner)\n(def %same 1)\n(def same 2)\n").unwrap();
        let error = plan_migration("test.owner", &[dir.clone()]).unwrap_err();
        assert!(error.to_string().contains("%same -> same"));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "(module test.owner)\n(def %same 1)\n(def same 2)\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dotted_family_is_idempotent_after_atomic_write() {
        let dir = temp_dir("family");
        fs::write(
            dir.join("a.lisp"),
            "(module test.family.a)\n(def one 1)\n(def %two 2)\n",
        )
        .unwrap();
        fs::write(
            dir.join("b.lisp"),
            "(module test.family.b)\n(def three () test.family.a/%two)\n",
        )
        .unwrap();
        let result = plan_migration("test.family", &[dir.clone()]).unwrap();
        assert_eq!(result.modules, vec!["test.family.a", "test.family.b"]);
        apply_migration(&result).unwrap();
        let second = plan_migration("test.family", &[dir.clone()]).unwrap();
        assert_eq!(second.changed_file_count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
