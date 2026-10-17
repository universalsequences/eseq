//! Lisp source as top-level forms, for the agent tools: an outline of a
//! module, one form's exact text, and replacing one form by name. The agent
//! reads and edits by form, never by line: an outline of a 3k-line module is
//! a few hundred lines, and a whole-form replacement cannot leave a bracket
//! half-edited.
//!
//! Spans come from eseqlisp's own tokenizer, so strings and comments are
//! read exactly as the language reads them.

use std::path::{Path, PathBuf};

use eseqlisp::lang::def_header::first_doc_line;
use eseqlisp::parser::{Expr, ExprKind, Parser, SpannedASTParser};

/// One top-level form of a source file.
#[derive(Debug, Clone, PartialEq)]
pub struct TopForm {
    /// The head symbol: `def`, `def-kind`, `module`, `bind-key`, …
    pub head: String,
    /// What the form defines: the symbol (or string, for
    /// `(def-sequencer "walker" …)`) after a defining head (see
    /// [`is_defining`]); None for any other form.
    pub name: Option<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-based lines of the form's first and last characters.
    pub line: usize,
    pub end_line: usize,
    /// The first line of the form's documentation: a `:doc "…"` slot, a
    /// docstring right after a `def`'s argument list, or else the `;`
    /// comment block directly above it.
    pub doc: Option<String>,
}

/// Whether a form with this head defines the name after it: `def`,
/// `defstate`, `def-kind`, `defwidget`, `define-mode`, `override`, … Any other
/// form (`set!`, `bind-key`, `import`) only acts, so it never stands in for a
/// definition.
pub fn is_defining(head: &str) -> bool {
    head.starts_with("def") || head == "override"
}

/// Definitions that share one namespace, so one may replace another: a
/// `(defstate x …)` takes the place of a `(def x …)`. Every other head is
/// its own family (`(def pulse …)` and `(def-kind pulse …)` are different
/// things that happen to share a name).
fn family(head: &str) -> &str {
    match head {
        "def" | "defstate" | "defscene" | "defcustom" | "defmacro" => "def",
        other => other,
    }
}

/// Every top-level form, in source order. A source that does not parse is
/// an error naming the line of the first unbalanced bracket when there is
/// one.
pub fn top_level_forms(source: &str) -> Result<Vec<TopForm>, String> {
    let exprs = parse(source)?;
    let line_starts = line_starts(source);
    Ok(exprs
        .iter()
        .map(|expr| {
            let span = &expr.origin.primary_span;
            let items = match &expr.kind {
                ExprKind::List(items) => items.as_slice(),
                _ => &[],
            };
            let head = items.first().and_then(symbol).unwrap_or_default().to_string();
            let name = is_defining(&head).then(|| items.get(1).and_then(name_of)).flatten();
            TopForm {
                doc: form_doc(items).or_else(|| comment_doc(source, span.start_byte)),
                head,
                name,
                start_byte: span.start_byte,
                end_byte: span.end_byte,
                line: line_of(&line_starts, span.start_byte),
                end_line: line_of(&line_starts, span.end_byte.saturating_sub(1)),
            }
        })
        .collect())
}

/// What [`define_form`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Defined {
    /// Replaced the existing form that defines the same name, at this line.
    Replaced { line: usize },
    /// No form defined the name (or the form defines nothing): appended.
    Appended { line: usize },
}

/// `source` with `form` (exactly one top-level form) in place of the
/// definition of the same name and family, or appended at the end when none
/// does (or `form` defines nothing). A name defined by more than one form is
/// an error rather than a guess.
pub fn define_form(source: &str, form: &str) -> Result<(String, Defined), String> {
    let new_forms = top_level_forms(form).map_err(|error| format!("the new form, line {error}"))?;
    let new = match new_forms.as_slice() {
        [one] => one,
        [] => return Err("the source holds no form".to_string()),
        many => {
            return Err(format!(
                "pass exactly one top-level form, got {}; define them one at a time",
                many.len()
            ))
        }
    };
    let text = form[new.start_byte..new.end_byte].to_string();
    let existing = top_level_forms(source).map_err(|error| format!("the file, line {error}"))?;
    let matches: Vec<&TopForm> = match &new.name {
        Some(name) => existing
            .iter()
            .filter(|old| old.name.as_ref() == Some(name) && family(&old.head) == family(&new.head))
            .collect(),
        None => Vec::new(),
    };
    match matches.as_slice() {
        [] => {
            let mut out = source.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            let line = out.lines().count() + 1;
            out.push_str(&text);
            out.push('\n');
            Ok((out, Defined::Appended { line }))
        }
        [old] => {
            let out = format!("{}{}{}", &source[..old.start_byte], text, &source[old.end_byte..]);
            Ok((out, Defined::Replaced { line: old.line }))
        }
        many => Err(format!(
            "{} forms define `{}` (lines {}); edit the file so one does",
            many.len(),
            new.name.as_deref().unwrap_or_default(),
            many.iter().map(|old| old.line.to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The exact text of the forms that define `name`, each preceded by its
/// line number.
pub fn form_source(source: &str, name: &str) -> Result<Vec<(usize, String)>, String> {
    Ok(top_level_forms(source)?
        .into_iter()
        .filter(|form| form.name.as_deref() == Some(name))
        .map(|form| (form.line, source[form.start_byte..form.end_byte].to_string()))
        .collect())
}

/// The file a module loads from: the first load root (Local, installed
/// packages, factory) holding one of its candidate paths. A package root
/// only resolves modules in its own namespace, prefix stripped.
pub fn resolve_module_file(roots: &[eseqlisp::ModuleLoadRoot], module: &str) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let relative = match &root.module_prefix {
            Some(prefix) => module.strip_prefix(prefix.as_str())?.strip_prefix('.')?,
            None => module,
        };
        eseqlisp::modules::module_relative_file_candidates(relative)
            .into_iter()
            .map(|candidate| root.path.join(candidate))
            .find(|candidate| candidate.is_file())
    })
}

/// `LINE: reason` for the first unbalanced bracket, or None.
pub fn bracket_problem(source: &str) -> Option<String> {
    let mut open: Vec<(char, usize)> = Vec::new();
    for (index, line) in code_only(source).lines().enumerate() {
        let line_no = index + 1;
        for ch in line.chars() {
            match ch {
                '(' | '[' => open.push((ch, line_no)),
                ')' | ']' => {
                    let want = if ch == ')' { '(' } else { '[' };
                    match open.pop() {
                        Some((got, _)) if got == want => {}
                        Some((got, at)) => {
                            return Some(format!(
                                "{line_no}: `{ch}` closes the `{got}` opened on line {at}"
                            ))
                        }
                        None => return Some(format!("{line_no}: `{ch}` has nothing to close")),
                    }
                }
                _ => {}
            }
        }
    }
    open.pop().map(|(ch, at)| format!("{at}: this `{ch}` is never closed"))
}

/// Source with `;` comments dropped and string contents blanked, so neither
/// can look like a form (a `\` escape inside a string is skipped). String state
/// carries across lines (strings may span them); newlines are always kept so
/// reported line numbers stay correct.
pub fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string = false;
    let mut in_comment = false;
    let mut escaped = false;
    for ch in source.chars() {
        match ch {
            '\n' => {
                in_comment = false;
                escaped = false;
                out.push('\n');
            }
            _ if in_comment => {}
            _ if escaped => {
                escaped = false;
                out.push(' ');
            }
            '\\' if in_string => {
                escaped = true;
                out.push(' ');
            }
            '"' => {
                in_string = !in_string;
                out.push('"');
            }
            ';' if !in_string => in_comment = true,
            _ if in_string => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// The user-writable module tiers: Local modules and installed packages.
pub fn user_module_roots(paths: &crate::app_paths::AppPaths) -> [PathBuf; 2] {
    [paths.local_modules_dir(), paths.packages_dir()]
}

/// Whether `path` lies under one of `roots`.
pub fn is_under_any(path: &Path, roots: &[PathBuf]) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|root| {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        path.starts_with(root)
    })
}

fn parse(source: &str) -> Result<Vec<Expr>, String> {
    let failure = |error: String| match bracket_problem(source) {
        Some(problem) => problem,
        None => error,
    };
    let tokens = Parser::new(source.to_string())
        .parse_spanned()
        .map_err(|error| failure(format!("cannot read the source: {error:?}")))?;
    SpannedASTParser::new(tokens)
        .parse()
        .map_err(|error| failure(format!("cannot parse the source: {error:?}")))
}

fn symbol(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Symbol(name) => Some(name),
        _ => None,
    }
}

fn name_of(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Symbol(name) | ExprKind::String(name) => Some(name.clone()),
        _ => None,
    }
}

/// `:doc "…"` anywhere at the top of the form, or a docstring as the first
/// body form of `(def name (args) "doc" body…)`.
fn form_doc(items: &[Expr]) -> Option<String> {
    let keyword_doc = items.windows(2).find_map(|pair| match (&pair[0].kind, &pair[1].kind) {
        (ExprKind::Keyword(key), ExprKind::String(doc)) if key.trim_start_matches(':') == "doc" => {
            Some(doc.clone())
        }
        _ => None,
    });
    let docstring = match items {
        [head, _, args, doc, _, ..] if symbol(head) == Some("def") => match (&args.kind, &doc.kind) {
            (ExprKind::List(_), ExprKind::String(doc)) => Some(doc.clone()),
            _ => None,
        },
        _ => None,
    };
    keyword_doc.or(docstring).and_then(|doc| first_doc_line(&doc).map(str::to_string))
}

/// The first line of the `;` comment block that ends on the line above
/// `start_byte`, if the form is not separated from it by a blank line.
fn comment_doc(source: &str, start_byte: usize) -> Option<String> {
    let before = &source[..start_byte];
    let mut block = Vec::new();
    for line in before.lines().rev().skip(usize::from(!before.ends_with('\n'))) {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(';') {
            break;
        }
        block.push(trimmed.trim_start_matches(';').trim());
    }
    block.reverse();
    block.into_iter().find(|line| !line.is_empty()).map(str::to_string)
}

fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

fn line_of(line_starts: &[usize], byte: usize) -> usize {
    line_starts.partition_point(|start| *start <= byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "(module my.pulse)\n\
(import eseq.kinds :refer (tracks))\n\
\n\
;; How many steps a ring has.\n\
;; Second line.\n\
(def ring-steps 16)\n\
\n\
(def hits (n k) \"Euclid: k onsets over n steps.\" (list n k))\n\
\n\
(def-kind pulse\n  :doc \"A pulse sequencer.\"\n  :fields ((rate 4)))\n";

    #[test]
    fn outline_names_heads_lines_and_docs() {
        let forms = top_level_forms(SOURCE).unwrap();
        let summary: Vec<_> = forms
            .iter()
            .map(|form| (form.head.as_str(), form.name.as_deref(), form.line, form.doc.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("module", None, 1, None),
                ("import", None, 2, None),
                ("def", Some("ring-steps"), 6, Some("How many steps a ring has.")),
                ("def", Some("hits"), 8, Some("Euclid: k onsets over n steps.")),
                ("def-kind", Some("pulse"), 10, Some("A pulse sequencer.")),
            ]
        );
        assert_eq!(forms[4].end_line, 12);
    }

    #[test]
    fn define_replaces_the_form_with_the_same_name_and_appends_new_ones() {
        let (replaced, outcome) = define_form(SOURCE, "(def ring-steps 8)").unwrap();
        assert_eq!(outcome, Defined::Replaced { line: 6 });
        assert!(replaced.contains("(def ring-steps 8)") && !replaced.contains("ring-steps 16"));
        assert!(replaced.contains(";; How many steps a ring has."), "comments stay");

        let (appended, outcome) = define_form(SOURCE, "(def rotate (xs n) xs)").unwrap();
        assert!(matches!(outcome, Defined::Appended { .. }));
        assert!(appended.ends_with("(def rotate (xs n) xs)\n"));
        assert_eq!(top_level_forms(&appended).unwrap().len(), 6);
    }

    #[test]
    fn only_definitions_replace_and_only_within_their_family() {
        let (out, outcome) = define_form(SOURCE, "(defstate ring-steps 12)").unwrap();
        assert_eq!(outcome, Defined::Replaced { line: 6 }, "defstate takes def's place");
        assert!(!out.contains("(def ring-steps 16)"));
        let (_, outcome) = define_form(SOURCE, "(def pulse 1)").unwrap();
        assert!(matches!(outcome, Defined::Appended { .. }), "a function is not the kind `pulse`");
        let (_, outcome) = define_form(SOURCE, "(set! ring-steps 3)").unwrap();
        assert!(matches!(outcome, Defined::Appended { .. }), "set! defines nothing");
        let forms = top_level_forms("(bind-key \"C-c a\" \"f\")\n(override eseq.mixer/strip (i) i)").unwrap();
        assert_eq!(forms[0].name, None);
        assert_eq!(forms[1].name.as_deref(), Some("eseq.mixer/strip"));
    }

    #[test]
    fn define_rejects_several_forms_and_ambiguous_names() {
        assert!(define_form(SOURCE, "(def a 1) (def b 2)").unwrap_err().contains("exactly one"));
        assert_eq!(define_form(SOURCE, "(def a (+ 1 2)").unwrap_err(), "the new form, line 1: this `(` is never closed");
        let doubled = format!("{SOURCE}(def hits 3)\n");
        assert!(define_form(&doubled, "(def hits 4)").unwrap_err().contains("2 forms define `hits`"));
    }

    #[test]
    fn unbalanced_source_reports_the_line() {
        let error = top_level_forms("(def a 1)\n(def b (list 1 2)\n").unwrap_err();
        assert_eq!(error, "2: this `(` is never closed");
        assert_eq!(bracket_problem("(def a \"(\")"), None, "brackets in strings do not count");
        assert_eq!(bracket_problem("(def a \"say \\\"(\\\" \")"), None, "an escaped quote stays in the string");
        assert_eq!(bracket_problem("; (\n(def a 1)"), None, "brackets in comments do not count");
    }

    #[test]
    fn form_source_returns_exact_text() {
        let found = form_source(SOURCE, "pulse").unwrap();
        assert_eq!(found, vec![(10, "(def-kind pulse\n  :doc \"A pulse sequencer.\"\n  :fields ((rate 4)))".to_string())]);
        assert!(form_source(SOURCE, "missing").unwrap().is_empty());
    }
}
