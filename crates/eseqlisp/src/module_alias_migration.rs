//! Detection and explicit migration of pre-module ESeqLisp spellings.
//!
//! The migration dictionary is embedded so this support survives removal of
//! `module-compat-alias` and works from packaged binaries without a source
//! checkout or Python runtime.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const ALIAS_TABLE: &str = include_str!("../../../tools/module-compat-aliases.tsv");
const WARNING_HIT_LIMIT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceKind {
    Code,
    Comment,
    String,
    Quoted,
}

impl OccurrenceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Comment => "comment",
            Self::String => "string",
            Self::Quoted => "quoted data",
        }
    }

    fn is_manual(self) -> bool {
        matches!(self, Self::String | Self::Quoted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasOccurrence {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub old: &'static str,
    pub new: &'static str,
    pub kind: OccurrenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMigration {
    pub rewritten: String,
    pub replacements: usize,
    pub manual: Vec<AliasOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMigration {
    pub path: PathBuf,
    pub original: String,
    pub rewritten: String,
    pub replacements: usize,
    pub manual: Vec<AliasOccurrence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationResult {
    pub files: Vec<FileMigration>,
}

impl MigrationResult {
    pub fn changed_files(&self) -> impl Iterator<Item = &FileMigration> {
        self.files.iter().filter(|file| file.replacements != 0)
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
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
    quoted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Symbol,
    String,
    Comment,
    Open,
    Close,
    Quote,
    Quasiquote,
    Unquote,
    Delimiter,
}

fn aliases() -> &'static BTreeMap<&'static str, &'static str> {
    static ALIASES: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    ALIASES.get_or_init(|| {
        let mut aliases = BTreeMap::new();
        for (index, line) in ALIAS_TABLE.lines().enumerate() {
            if index == 0 {
                assert_eq!(
                    line, "old\tnew\tsource",
                    "invalid embedded alias-table header"
                );
                continue;
            }
            let mut fields = line.split('\t');
            let old = fields.next().expect("alias old spelling");
            let new = fields.next().expect("alias new spelling");
            let _source = fields.next().expect("alias source");
            assert!(
                fields.next().is_none(),
                "invalid embedded alias row {}",
                index + 1
            );
            assert!(aliases.insert(old, new).is_none(), "duplicate alias {old}");
        }
        assert!(
            !aliases.is_empty(),
            "embedded alias table must not be empty"
        );
        aliases
    })
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b'\'' | b'`' | b',' | b';'
    )
}

fn raw_tokens(source: &str) -> Result<Vec<Token>, String> {
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
            b'(' => {
                index += 1;
                TokenKind::Open
            }
            b')' => {
                index += 1;
                TokenKind::Close
            }
            b'\'' => {
                index += 1;
                TokenKind::Quote
            }
            b'`' => {
                index += 1;
                TokenKind::Quasiquote
            }
            b',' => {
                index += 1;
                TokenKind::Unquote
            }
            b'[' | b']' | b'{' | b'}' => {
                index += 1;
                TokenKind::Delimiter
            }
            _ => {
                index += 1;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !is_delimiter(bytes[index])
                {
                    index += 1;
                }
                TokenKind::Symbol
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: index,
            quoted: false,
        });
    }
    Ok(tokens)
}

fn mark_quoted(tokens: &mut [Token]) {
    let significant = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| (token.kind != TokenKind::Comment).then_some(index))
        .collect::<Vec<_>>();

    fn expression(
        order: usize,
        quote_depth: usize,
        significant: &[usize],
        tokens: &mut [Token],
    ) -> usize {
        if order >= significant.len() {
            return order;
        }
        let token_index = significant[order];
        match tokens[token_index].kind {
            TokenKind::Quote | TokenKind::Quasiquote => {
                expression(order + 1, quote_depth + 1, significant, tokens)
            }
            TokenKind::Unquote => expression(
                order + 1,
                quote_depth.saturating_sub(1),
                significant,
                tokens,
            ),
            TokenKind::Open => {
                let mut next = order + 1;
                while next < significant.len() && tokens[significant[next]].kind != TokenKind::Close
                {
                    next = expression(next, quote_depth, significant, tokens);
                }
                next + 1
            }
            TokenKind::Symbol | TokenKind::String => {
                tokens[token_index].quoted = quote_depth != 0;
                order + 1
            }
            _ => order + 1,
        }
    }

    let mut order = 0;
    while order < significant.len() {
        order = expression(order, 0, &significant, tokens);
    }
}

fn line_and_column(source: &str, offset: usize) -> (usize, usize) {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rfind('\n')
        .map_or(offset + 1, |newline| offset - newline);
    (line, column)
}

fn push_occurrence(
    found: &mut Vec<AliasOccurrence>,
    source: &str,
    start: usize,
    end: usize,
    old: &'static str,
    new: &'static str,
    kind: OccurrenceKind,
) {
    let (line, column) = line_and_column(source, start);
    found.push(AliasOccurrence {
        start,
        end,
        line,
        column,
        old,
        new,
        kind,
    });
}

pub fn occurrences(source: &str) -> Result<Vec<AliasOccurrence>, String> {
    let mut tokens = raw_tokens(source)?;
    mark_quoted(&mut tokens);
    let aliases = aliases();
    let mut found = Vec::new();
    for token in tokens {
        match token.kind {
            TokenKind::Symbol => {
                let text = &source[token.start..token.end];
                if let Some((&old, &new)) = aliases.get_key_value(text) {
                    let kind = if token.quoted {
                        OccurrenceKind::Quoted
                    } else {
                        OccurrenceKind::Code
                    };
                    push_occurrence(&mut found, source, token.start, token.end, old, new, kind);
                }
            }
            TokenKind::String => {
                let start = token.start + 1;
                let end = token.end - 1;
                let text = &source[start..end];
                if let Some((&old, &new)) = aliases.get_key_value(text) {
                    push_occurrence(
                        &mut found,
                        source,
                        start,
                        end,
                        old,
                        new,
                        OccurrenceKind::String,
                    );
                }
            }
            TokenKind::Comment => {
                let mut index = token.start;
                while index < token.end {
                    let byte = source.as_bytes()[index];
                    if byte.is_ascii_whitespace() || is_delimiter(byte) {
                        index += 1;
                        continue;
                    }
                    let start = index;
                    index += 1;
                    while index < token.end
                        && !source.as_bytes()[index].is_ascii_whitespace()
                        && !is_delimiter(source.as_bytes()[index])
                    {
                        index += 1;
                    }
                    let text = &source[start..index];
                    if let Some((&old, &new)) = aliases.get_key_value(text) {
                        push_occurrence(
                            &mut found,
                            source,
                            start,
                            index,
                            old,
                            new,
                            OccurrenceKind::Comment,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(found)
}

pub fn migrate_source(source: &str) -> Result<SourceMigration, String> {
    let found = occurrences(source)?;
    let mut replacements = found
        .iter()
        .filter(|occurrence| !occurrence.kind.is_manual())
        .collect::<Vec<_>>();
    replacements.sort_by_key(|occurrence| occurrence.start);
    let mut rewritten = source.to_string();
    for occurrence in replacements.iter().rev() {
        rewritten.replace_range(occurrence.start..occurrence.end, occurrence.new);
    }
    Ok(SourceMigration {
        rewritten,
        replacements: replacements.len(),
        manual: found
            .into_iter()
            .filter(|occurrence| occurrence.kind.is_manual())
            .collect(),
    })
}

fn warning_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn warned_paths() -> &'static Mutex<HashSet<PathBuf>> {
    static WARNED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Returns and emits an actionable warning the first time `path` contains an
/// old spelling in this process. Clean scans are not remembered, so a later
/// edit that introduces an old spelling is still detected.
pub fn warn_on_old_module_aliases(path: &Path, source: &str) -> Option<String> {
    let found = match occurrences(source) {
        Ok(found) => found,
        Err(error) => {
            // The Lisp parser owns malformed-source diagnostics. Alias
            // detection must never prevent the normal load path.
            eprintln!(
                "module-alias preflight skipped for {}: {error}",
                path.display()
            );
            return None;
        }
    };
    if found.is_empty() {
        return None;
    }
    let path = warning_path(path);
    let mut warned = warned_paths()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !warned.insert(path.clone()) {
        return None;
    }
    drop(warned);

    let mut warning = format!(
        "warning: {} contains {} pre-module spelling{} that will stop working when compatibility aliases are removed\n",
        path.display(),
        found.len(),
        if found.len() == 1 { "" } else { "s" },
    );
    for occurrence in found.iter().take(WARNING_HIT_LIMIT) {
        warning.push_str(&format!(
            "  {}:{}:{}: {} `{}` -> `{}`\n",
            path.display(),
            occurrence.line,
            occurrence.column,
            occurrence.kind.label(),
            occurrence.old,
            occurrence.new,
        ));
    }
    if found.len() > WARNING_HIT_LIMIT {
        warning.push_str(&format!(
            "  ... and {} more\n",
            found.len() - WARNING_HIT_LIMIT
        ));
    }
    warning.push_str(&format!(
        "  Preview the explicit migration: eseqlisp_migrate_module_aliases --dry-run -- {}\n  Apply it only after review: eseqlisp_migrate_module_aliases --write -- {}",
        shell_quote(&path), shell_quote(&path),
    ));
    eprintln!("{warning}");
    Some(warning)
}

fn collect_path(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
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
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a .lisp file: {}", path.display()),
            ));
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        collect_path(&entry.path(), files)?;
    }
    Ok(())
}

pub fn plan_migration(paths: &[PathBuf]) -> io::Result<MigrationResult> {
    let mut files = Vec::new();
    for path in paths {
        collect_path(path, &mut files)?;
    }
    files.sort();
    files.dedup();
    let mut migrations = Vec::with_capacity(files.len());
    for path in files {
        let original = fs::read_to_string(&path)?;
        let migration = migrate_source(&original).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        })?;
        migrations.push(FileMigration {
            path,
            original,
            rewritten: migration.rewritten,
            replacements: migration.replacements,
            manual: migration.manual,
        });
    }
    Ok(MigrationResult { files: migrations })
}

fn adjacent_path(path: &Path, role: &str, ordinal: usize) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lisp");
    for attempt in 0..1000 {
        let candidate = parent.join(format!(
            ".{name}.eseq-module-migration-{role}-{}-{ordinal}-{attempt}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "cannot allocate migration staging path beside {}",
            path.display()
        ),
    ))
}

struct StagedFile {
    path: PathBuf,
    temporary: PathBuf,
    backup: PathBuf,
}

fn cleanup_staged(staged: &[StagedFile]) {
    for file in staged {
        let _ = fs::remove_file(&file.temporary);
    }
}

fn rollback(committed: &[StagedFile]) -> io::Result<()> {
    let mut errors = Vec::new();
    for file in committed.iter().rev() {
        if file.path.exists() {
            if let Err(error) = fs::remove_file(&file.path) {
                errors.push(format!("remove {}: {error}", file.path.display()));
                continue;
            }
        }
        if let Err(error) = fs::rename(&file.backup, &file.path) {
            errors.push(format!("restore {}: {error}", file.path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "migration rollback failed: {}",
            errors.join("; ")
        )))
    }
}

fn apply_migration_inner(
    result: &MigrationResult,
    fail_before_replace: Option<usize>,
) -> io::Result<()> {
    let changed = result.changed_files().collect::<Vec<_>>();
    let mut staged = Vec::with_capacity(changed.len());
    for (ordinal, migration) in changed.iter().enumerate() {
        let temporary = adjacent_path(&migration.path, "new", ordinal)?;
        let backup = adjacent_path(&migration.path, "backup", ordinal)?;
        let stage_result = (|| {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            output.write_all(migration.rewritten.as_bytes())?;
            let permissions = fs::metadata(&migration.path)?.permissions();
            fs::set_permissions(&temporary, permissions)?;
            output.sync_all()?;
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = stage_result {
            let _ = fs::remove_file(&temporary);
            cleanup_staged(&staged);
            return Err(error);
        }
        staged.push(StagedFile {
            path: migration.path.clone(),
            temporary,
            backup,
        });
    }

    let mut committed_count = 0;
    for (ordinal, file) in staged.iter().enumerate() {
        // The adjacent hard link preserves the exact original inode for
        // rollback while leaving the destination in place. Renaming the
        // staged file over it is therefore an atomic per-file replacement.
        if let Err(error) = fs::hard_link(&file.path, &file.backup) {
            cleanup_staged(&staged);
            let rollback_error = rollback(&staged[..committed_count]).err();
            return Err(io::Error::other(format!(
                "failed to back up {}: {error}{}",
                file.path.display(),
                rollback_error.map_or(String::new(), |rollback| format!("; {rollback}")),
            )));
        }
        committed_count += 1;
        let replace_result = if fail_before_replace == Some(ordinal) {
            Err(io::Error::other("injected migration write failure"))
        } else {
            fs::rename(&file.temporary, &file.path)
        };
        if let Err(error) = replace_result {
            cleanup_staged(&staged);
            let rollback_error = rollback(&staged[..committed_count]).err();
            return Err(io::Error::other(format!(
                "failed to replace {}: {error}{}",
                file.path.display(),
                rollback_error.map_or(String::new(), |rollback| format!("; {rollback}")),
            )));
        }
    }

    for file in &staged {
        fs::remove_file(&file.backup)?;
    }
    Ok(())
}

pub fn apply_migration(result: &MigrationResult) -> io::Result<()> {
    apply_migration_inner(result, None)
}

fn push_diff_line(diff: &mut String, prefix: char, line: &str) {
    diff.push(prefix);
    diff.push_str(line.trim_end_matches('\n'));
    diff.push('\n');
    if !line.ends_with('\n') {
        diff.push_str("\\ No newline at end of file\n");
    }
}

/// Produces a line-aligned unified diff with three context lines. Alias
/// replacement never inserts or removes a newline, so no general-purpose diff
/// dependency is needed and each changed line has an unambiguous counterpart.
pub fn unified_diff(file: &FileMigration) -> String {
    if file.replacements == 0 {
        return String::new();
    }
    let old_lines = file.original.split_inclusive('\n').collect::<Vec<_>>();
    let new_lines = file.rewritten.split_inclusive('\n').collect::<Vec<_>>();
    assert_eq!(
        old_lines.len(),
        new_lines.len(),
        "alias rewrite changed line count"
    );
    let changed = old_lines
        .iter()
        .zip(&new_lines)
        .enumerate()
        .filter_map(|(index, (old, new))| (old != new).then_some(index))
        .collect::<Vec<_>>();
    let mut hunks = Vec::new();
    for changed_line in changed {
        let start = changed_line.saturating_sub(3);
        let end = (changed_line + 4).min(old_lines.len());
        if let Some((_, previous_end)) = hunks.last_mut()
            && start <= *previous_end
        {
            *previous_end = (*previous_end).max(end);
        } else {
            hunks.push((start, end));
        }
    }

    let mut diff = format!("--- {}\n+++ {}\n", file.path.display(), file.path.display());
    for (start, end) in hunks {
        let count = end - start;
        diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            start + 1,
            count,
            start + 1,
            count
        ));
        for index in start..end {
            if old_lines[index] == new_lines[index] {
                push_diff_line(&mut diff, ' ', old_lines[index]);
            } else {
                push_diff_line(&mut diff, '-', old_lines[index]);
                push_diff_line(&mut diff, '+', new_lines[index]);
            }
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/module-alias-migration")
            .join(name)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-module-alias-migration-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn embedded_dictionary_and_token_semantics_cover_migration_hazards() {
        assert_eq!(aliases().len(), 720);
        let source = fs::read_to_string(fixture("hazards.lisp")).unwrap();
        let migration = migrate_source(&source).unwrap();
        let expected = fs::read_to_string(fixture("hazards.expected.lisp")).unwrap();
        assert_eq!(migration.rewritten, expected);
        assert_eq!(migration.replacements, 5);
        assert_eq!(
            migration
                .manual
                .iter()
                .map(|hit| hit.kind)
                .collect::<Vec<_>>(),
            vec![OccurrenceKind::String, OccurrenceKind::Quoted]
        );
        assert!(
            migration.rewritten.contains("seq-apply-fx-layout-extra"),
            "hyphen-prefixed longer symbol must not collide"
        );
        assert!(
            migration
                .rewritten
                .contains("eseq.effects.state/effect-mods-open"),
            "identity alias must still qualify"
        );
        assert!(
            migration
                .rewritten
                .contains("(set! eseq.seq-core-state/current-step 9)")
        );
    }

    #[test]
    fn dry_run_plan_is_idempotent_and_does_not_touch_files() {
        let dir = temp_dir("dry-run");
        let path = dir.join("content.lisp");
        let original = fs::read_to_string(fixture("hazards.lisp")).unwrap();
        fs::write(&path, &original).unwrap();
        let plan = plan_migration(std::slice::from_ref(&path)).unwrap();
        assert_eq!(plan.changed_file_count(), 1);
        assert!(unified_diff(&plan.files[0]).contains("-  (set! current-step 9)"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        apply_migration(&plan).unwrap();
        let second = plan_migration(std::slice::from_ref(&path)).unwrap();
        assert_eq!(second.changed_file_count(), 0);
        assert_eq!(second.replacement_count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn batch_write_rolls_back_every_file_after_mid_batch_failure() {
        let dir = temp_dir("rollback");
        let first = dir.join("a.lisp");
        let second = dir.join("b.lisp");
        fs::write(&first, "(seq-apply-fx-layout)\n").unwrap();
        fs::write(&second, "(set! current-step 3)\n").unwrap();
        let plan = plan_migration(&[dir.clone()]).unwrap();
        let error = apply_migration_inner(&plan, Some(1)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("injected migration write failure")
        );
        assert_eq!(
            fs::read_to_string(&first).unwrap(),
            "(seq-apply-fx-layout)\n"
        );
        assert_eq!(
            fs::read_to_string(&second).unwrap(),
            "(set! current-step 3)\n"
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            2,
            "staging and backup files must be cleaned up"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn path_backed_runtime_load_uses_the_preflight_chokepoint() {
        let dir = temp_dir("runtime-load");
        let path = dir.join("external.lisp");
        let source = "(def seq-apply-fx-layout 1)\n";
        fs::write(&path, source).unwrap();
        let mut runtime = crate::Runtime::new();
        runtime.eval_source_at_path(path.clone(), source).unwrap();
        assert!(
            warn_on_old_module_aliases(&path, source).is_none(),
            "the VM load must have consumed this file's one session warning"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_warning_is_actionable_and_fires_once_per_file() {
        let dir = temp_dir("warning");
        let path = dir.join("external content's ui.lisp");
        let source = "(do (seq-apply-fx-layout) (set! current-step 2))\n";
        fs::write(&path, source).unwrap();
        let warning = warn_on_old_module_aliases(&path, source).expect("first warning");
        assert!(warning.contains(&path.display().to_string()));
        assert!(warning.contains("contains 2 pre-module spellings"));
        assert!(
            warning
                .contains(":1:6: code `seq-apply-fx-layout` -> `eseq.seq-layout/apply-fx-layout`")
        );
        assert!(warning.contains("`current-step` -> `eseq.seq-core-state/current-step`"));
        assert!(warning.contains("eseqlisp_migrate_module_aliases --dry-run --"));
        assert!(warning.contains("external content'\\''s ui.lisp"));
        assert!(warn_on_old_module_aliases(&path, source).is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
