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
struct ReferEntry {
    module: String,
    name: String,
    span: (usize, usize),
}

#[derive(Debug, Clone)]
struct SourceAnalysis {
    module: Option<String>,
    definitions: Vec<Definition>,
    aliases: HashMap<String, String>,
    refers: Vec<ReferEntry>,
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
    let mut refers = Vec::new();
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
                        (ExprKind::Keyword(keyword), ExprKind::List(names))
                            if keyword == "refer" =>
                        {
                            for name in names {
                                if let ExprKind::Symbol(symbol) = &name.kind {
                                    let span = &name.origin.primary_span;
                                    refers.push(ReferEntry {
                                        module: imported.to_string(),
                                        name: symbol.clone(),
                                        span: (span.start_byte, span.end_byte),
                                    });
                                }
                            }
                            index += 2;
                        }
                        _ => index += 1,
                    }
                }
            }
            "export" => has_export = true,
            "def" | "defn" | "defstate" | "defscene" | "defmacro" => {
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
                if let Some(name) = name
                    // An explicitly qualified definition belongs to the named
                    // namespace, not to the module containing this source.
                    // In particular, compatibility pins into eseq.vanilla
                    // must not become invalid qualified export entries.
                    && !name.contains('/')
                {
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
        refers,
        insertion_anchor,
        has_export,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SourceKind {
    Lisp,
    Rust,
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

fn rust_string_ranges(source: &str) -> Result<Vec<(usize, usize)>, String> {
    use rustc_lexer::{LiteralKind, TokenKind};

    let mut strings = Vec::new();
    let mut offset = 0;
    for token in rustc_lexer::tokenize(source) {
        match token.kind {
            TokenKind::BlockComment { terminated: false } => {
                return Err(format!("unterminated block comment at byte {offset}"));
            }
            TokenKind::Literal { kind, suffix_start } => {
                let (prefix, suffix) = match kind {
                    LiteralKind::Str { terminated: true } => (1, 1),
                    LiteralKind::ByteStr { terminated: true } => (2, 1),
                    LiteralKind::RawStr {
                        n_hashes,
                        started: true,
                        terminated: true,
                    } => (n_hashes + 2, n_hashes + 1),
                    LiteralKind::RawByteStr {
                        n_hashes,
                        started: true,
                        terminated: true,
                    } => (n_hashes + 3, n_hashes + 1),
                    LiteralKind::Str { terminated: false }
                    | LiteralKind::ByteStr { terminated: false }
                    | LiteralKind::RawStr {
                        terminated: false, ..
                    }
                    | LiteralKind::RawByteStr {
                        terminated: false, ..
                    } => {
                        return Err(format!("unterminated string at byte {offset}"));
                    }
                    _ => {
                        offset += token.len;
                        continue;
                    }
                };
                strings.push((offset + prefix, offset + suffix_start - suffix));
            }
            _ => {}
        }
        offset += token.len;
    }
    Ok(strings)
}

fn symbol_occurrences<'a>(text: &'a str, symbol: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(symbol).filter_map(move |(offset, _)| {
        let before = (offset == 0).then_some(true).unwrap_or_else(|| {
            text.as_bytes()
                .get(offset - 1)
                .is_some_and(|byte| is_delimiter(*byte))
        });
        let end = offset + symbol.len();
        let after = end == text.len()
            || text
                .as_bytes()
                .get(end)
                .is_some_and(|byte| is_delimiter(*byte));
        (before && after).then_some(offset)
    })
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

fn source_kind(path: &Path) -> Option<SourceKind> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("lisp") => Some(SourceKind::Lisp),
        Some("rs") => Some(SourceKind::Rust),
        _ => None,
    }
}

fn collect_path(
    path: &Path,
    files: &mut Vec<(PathBuf, SourceKind)>,
    explicit: bool,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to replace symbolic link: {}", path.display()),
        ));
    }
    if metadata.is_file() {
        if let Some(kind) = source_kind(path) {
            files.push((path.to_path_buf(), kind));
        } else if explicit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a .lisp or .rs file: {}", path.display()),
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
    for (path, kind) in paths_to_scan {
        let source = fs::read_to_string(&path)?;
        let analysis = if kind == SourceKind::Lisp {
            Some(analyze(&source).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", path.display()),
                )
            })?)
        } else {
            None
        };
        sources.push((path, source, kind, analysis));
    }

    let mut module_plans = HashMap::new();
    for (path, _, _, analysis) in &sources {
        let Some(analysis) = analysis else {
            continue;
        };
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
    for (path, original, kind, analysis) in sources {
        let mut replacements = Vec::<(usize, usize, String)>::new();
        let mut manual = Vec::new();
        let own = analysis
            .as_ref()
            .and_then(|analysis| analysis.module.as_ref())
            .and_then(|module| module_plans.get(module));

        let mut qualified = BTreeMap::new();
        for (module, plan) in &module_plans {
            for (old, new) in &plan.private {
                qualified.insert(format!("{module}/{old}"), format!("{module}/{new}"));
                if let Some(analysis) = &analysis {
                    for (alias, imported) in &analysis.aliases {
                        if imported == module {
                            qualified.insert(format!("{alias}/{old}"), format!("{alias}/{new}"));
                        }
                    }
                }
            }
        }

        // Older module conversion output used fully-qualified spellings in
        // :refer lists. The export contract requires bare names there: the
        // import already names the owner, and visibility validation operates
        // on that owner's base-name export set. Normalize only this syntax
        // position; ordinary qualified call sites must stay qualified.
        let mut refer_spans = HashSet::new();
        if let Some(analysis) = &analysis {
            for refer in &analysis.refers {
                let Some(plan) = module_plans.get(&refer.module) else {
                    continue;
                };
                let base = match crate::modules::split_qualified(&refer.name) {
                    Some((namespace, base)) if namespace == refer.module => base,
                    None => refer.name.as_str(),
                    Some(_) => continue,
                };
                if plan.private.contains_key(base) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "{}: cannot preserve private :refer entry '{}'; {} does not export it",
                            path.display(),
                            refer.name,
                            refer.module,
                        ),
                    ));
                }
                let replacement = base.to_string();
                if replacement != refer.name {
                    replacements.push((refer.span.0, refer.span.1, replacement));
                    refer_spans.insert(refer.span);
                }
            }
        }

        let tokens = if kind == SourceKind::Lisp {
            scan_tokens(&original)
        } else {
            rust_string_ranges(&original).map(|ranges| {
                ranges
                    .into_iter()
                    .map(|(start, end)| Token {
                        kind: TokenKind::String,
                        start: start - 1,
                        end: end + 1,
                    })
                    .collect()
            })
        };
        for token in tokens.map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        })? {
            match token.kind {
                TokenKind::Symbol => {
                    if refer_spans.contains(&(token.start, token.end)) {
                        continue;
                    }
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
                        for relative in symbol_occurrences(text, &old) {
                            let offset = inner.0 + relative;
                            let (line, column) = line_column(&original, offset);
                            manual.push(ManualOccurrence {
                                line,
                                column,
                                old: old.clone(),
                                new: new.clone(),
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
            "(module test.consumer)\n(import test.owner :as own)\n(import test.owner :refer (test.owner/public))\n(def use () (list (own/%hidden 1) test.owner/%cell foreign/%hidden))\n(def names () \"test.owner/%hidden\")\n",
        ).unwrap();
        let rust_consumer = dir.join("consumer.rs");
        fs::write(
            &rust_consumer,
            "// \"test.owner/%hidden\" is only a comment\nconst QUOTE: char = '\"';\nconst ONE: &str = \"test.owner/%hidden test.owner/%hidden\";\nconst TWO: &str = r#\"test.owner/%cell\"#;\n",
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
        assert!(consumer
            .rewritten
            .contains("(import test.owner :refer (public))"));
        assert!(consumer.rewritten.contains("own/hidden"));
        assert!(consumer.rewritten.contains("test.owner/cell"));
        assert!(consumer.rewritten.contains("foreign/%hidden"));
        assert_eq!(consumer.manual.len(), 1);
        let rust_consumer = result
            .files
            .iter()
            .find(|file| file.path == rust_consumer)
            .unwrap();
        assert_eq!(rust_consumer.manual.len(), 3);
        assert_eq!(rust_consumer.rewritten, rust_consumer.original);
        assert!(unified_diff(owner).contains("+        published-hook)"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn private_refer_refuses_the_plan_instead_of_creating_an_invalid_import() {
        let dir = temp_dir("private-refer");
        fs::write(
            dir.join("owner.lisp"),
            "(module test.owner)\n(def %hidden 1)\n",
        )
        .unwrap();
        fs::write(
            dir.join("consumer.lisp"),
            "(module test.consumer)\n(import test.owner :refer (test.owner/%hidden))\n",
        )
        .unwrap();

        let error = plan_migration("test.owner", &[dir.clone()]).unwrap_err();
        assert!(error.to_string().contains("cannot preserve private :refer"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn qualified_definitions_are_not_exported_or_rewritten_as_module_members() {
        let dir = temp_dir("qualified-definition");
        let path = dir.join("owner.lisp");
        fs::write(
            &path,
            "(module test.owner)\n(def public 1)\n(def eseq.vanilla/pinned 2)\n(def eseq.vanilla/%legacy 3)\n",
        )
        .unwrap();

        let result = plan_migration("test.owner", &[dir.clone()]).unwrap();
        let file = result.files.iter().find(|file| file.path == path).unwrap();
        assert!(file.rewritten.contains("(export public)"));
        assert!(file.rewritten.contains("(def eseq.vanilla/pinned 2)"));
        assert!(file.rewritten.contains("(def eseq.vanilla/%legacy 3)"));
        assert!(!file.rewritten.contains("export eseq.vanilla/"));
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
