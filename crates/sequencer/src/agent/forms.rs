//! Lisp source as top-level forms, for the agent tools: an outline of a
//! module, one form's exact text, and replacing one form by name. The agent
//! reads and edits by form, never by line: an outline of a 3k-line module is
//! a few hundred lines, and a whole-form replacement cannot leave a bracket
//! half-edited.
//!
//! Spans come from eseqlisp's own tokenizer, so strings and comments are
//! read exactly as the language reads them.

use std::path::{Path, PathBuf};

use eseqlisp::lang::def_header::{comment_block_above, first_doc_line, form_doc_text};
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
                doc: form_doc_text(items)
                    .or_else(|| comment_block_above(source, span.start_byte))
                    .and_then(|doc| first_doc_line(&doc).map(str::to_string)),
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

/// `form` (one top-level list) with its item at `index` replaced by `text`,
/// or None when it has no such item: `(defcustom gain 0.5 …)` with item 2
/// replaced by `0.8`.
pub fn with_item_replaced(form: &str, index: usize, text: &str) -> Option<String> {
    let exprs = parse(form).ok()?;
    let [expr] = exprs.as_slice() else { return None };
    let ExprKind::List(items) = &expr.kind else { return None };
    let span = &items.get(index)?.origin.primary_span;
    Some(format!("{}{text}{}", &form[..span.start_byte], &form[span.end_byte..]))
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

/// The heads that name what a DGenLisp form defines: `(def x …)`,
/// `(param x …)`, `(defmacro x …)`.
const DGEN_DEFINING: &[&str] = &["def", "param", "defmacro"];

/// One top-level DGenLisp form: its head, the name it defines (for `def`,
/// `param` and `defmacro`), its byte span and 1-based first line.
struct DgenForm {
    head: String,
    name: Option<String>,
    start_byte: usize,
    end_byte: usize,
    line: usize,
}

fn dgen_forms(source: &str) -> Result<Vec<DgenForm>, String> {
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
            let name = DGEN_DEFINING.contains(&head.as_str()).then(|| items.get(1).and_then(symbol).map(str::to_string)).flatten();
            DgenForm { head, name, start_byte: span.start_byte, end_byte: span.end_byte, line: line_of(&line_starts, span.start_byte) }
        })
        .collect())
}

/// `source` (DGenLisp) with `form`, one `def`, `param` or `defmacro`, in
/// place of the form defining the same name. A new name goes after the form
/// defining `after` when given, else before the first `(out …)` (the DSP
/// reads top to bottom, and outputs come last), else at the end.
pub fn define_dgen_form(source: &str, form: &str, after: Option<&str>) -> Result<(String, Defined), String> {
    let new_forms = dgen_forms(form).map_err(|error| format!("the new form: {error}"))?;
    let new = match new_forms.as_slice() {
        [one] => one,
        [] => return Err("the source holds no form".to_string()),
        many => return Err(format!("pass exactly one top-level form, got {}; change them one at a time", many.len())),
    };
    let Some(name) = &new.name else {
        return Err(format!(
            "`({} …)` defines no name; pass a def, param or defmacro (to change an `out`, change the def it outputs)",
            new.head
        ));
    };
    let text = &form[new.start_byte..new.end_byte];
    let existing = dgen_forms(source).map_err(|error| format!("the code: {error}"))?;
    let matches: Vec<&DgenForm> = existing.iter().filter(|old| old.name.as_ref() == Some(name)).collect();
    match matches.as_slice() {
        [old] => {
            let out = format!("{}{}{}", &source[..old.start_byte], text, &source[old.end_byte..]);
            Ok((out, Defined::Replaced { line: old.line }))
        }
        [] => {
            let at = match after {
                Some(after) => {
                    let Some(anchor) = existing.iter().find(|old| old.name.as_deref() == Some(after)) else {
                        return Err(format!("no form defines `{after}` to put `{name}` after"));
                    };
                    Some(anchor.end_byte)
                }
                None => existing.iter().find(|old| old.head == "out").map(|out| out.start_byte),
            };
            Ok(match at {
                Some(at) => {
                    // After an anchor the form starts a new line; before an
                    // `out` it takes the out's place and the out moves down.
                    let (before, rest) = source.split_at(at);
                    let (inserted, line) = if after.is_some() {
                        (format!("{before}\n{text}{rest}"), before.lines().count() + 1)
                    } else {
                        (format!("{before}{text}\n{rest}"), before.lines().count() + 1)
                    };
                    (inserted, Defined::Appended { line })
                }
                None => {
                    let mut out = source.trim_end().to_string();
                    out.push_str("\n");
                    let line = out.lines().count() + 1;
                    out.push_str(text);
                    out.push('\n');
                    (out, Defined::Appended { line })
                }
            })
        }
        many => Err(format!(
            "{} forms define `{name}` (lines {}); remove the extra ones first",
            many.len(),
            many.iter().map(|old| old.line.to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The name in the source's first form when that form is `(module NAME)`.
pub fn declared_module(source: &str) -> Option<String> {
    let exprs = parse(source).ok()?;
    let ExprKind::List(items) = &exprs.first()?.kind else { return None };
    match items.as_slice() {
        [head, name, ..] if symbol(head) == Some("module") => symbol(name).map(str::to_string),
        _ => None,
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

/// A source line `search` matched: its number, the top-level form around
/// it (`(def track-strip …)` as head and name) and the line itself.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceHit {
    pub line: usize,
    pub form: Option<(String, Option<String>)>,
    pub text: String,
}

/// What `search` found in one source.
#[derive(Debug, Default, PartialEq)]
pub struct SourceMatches {
    /// Every matching line, shown or not.
    pub count: usize,
    /// The first matches, up to the budget.
    pub hits: Vec<SourceHit>,
    /// False when the source does not parse: its hits carry no form.
    pub parsed: bool,
}

/// The lines of `source` that `pattern` matches: all of them counted, the
/// first `budget` returned with the top-level form each sits in. The source
/// is only parsed when there is something to return.
pub fn search(source: &str, pattern: &regex::Regex, budget: usize) -> SourceMatches {
    let lines: Vec<(usize, &str)> =
        source.lines().enumerate().filter(|(_, line)| pattern.is_match(line)).map(|(index, line)| (index + 1, line)).collect();
    if lines.is_empty() || budget == 0 {
        return SourceMatches { count: lines.len(), hits: Vec::new(), parsed: true };
    }
    let forms = top_level_forms(source);
    let parsed = forms.is_ok();
    let forms = forms.unwrap_or_default();
    let hits = lines
        .iter()
        .take(budget)
        .map(|&(line, text)| {
            // Forms are in source order: the candidate is the last one starting at or before `line`.
            let before = forms.partition_point(|form| form.line <= line);
            let form = before.checked_sub(1).map(|index| &forms[index]).filter(|form| line <= form.end_line);
            SourceHit {
                line,
                form: form.map(|form| (form.head.clone(), form.name.clone())),
                text: text.trim().to_string(),
            }
        })
        .collect();
    SourceMatches { count: lines.len(), hits, parsed }
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

    const DSP: &str = "(def gate (in 1 @name gate))
(param radius @default 0.1 @min 0.001 @max 0.3 @unit m)
(defmacro onset-of (trigger) (make-history previous) trigger)
(def young 2.08e11)
(def air-loss (* young 1e-7))
(out (* air-loss gate) 1)
";

    #[test]
    fn dgen_forms_replace_by_name_and_new_defs_land_before_the_outputs() {
        let (out, defined) = define_dgen_form(DSP, "(param radius @default 0.2 @min 0.001 @max 0.5 @unit m)", None).unwrap();
        assert_eq!(defined, Defined::Replaced { line: 2 });
        assert!(out.contains("(param radius @default 0.2 @min 0.001 @max 0.5 @unit m)\n(defmacro"), "{out}");

        let (out, defined) = define_dgen_form(DSP, "(def wet (* air-loss 0.5))", None).unwrap();
        assert_eq!(defined, Defined::Appended { line: 6 });
        assert!(out.contains("(def air-loss (* young 1e-7))\n(def wet (* air-loss 0.5))\n(out "), "{out}");

        let (out, _) = define_dgen_form(DSP, "(def dry 1)", Some("gate")).unwrap();
        assert!(out.starts_with("(def gate (in 1 @name gate))\n(def dry 1)\n(param"), "{out}");

        assert!(define_dgen_form(DSP, "(out 0 1)", None).unwrap_err().contains("defines no name"));
        assert!(define_dgen_form(DSP, "(def a 1) (def b 2)", None).unwrap_err().contains("exactly one"));
        assert!(define_dgen_form(DSP, "(def a 1)", Some("nope")).unwrap_err().contains("no form defines `nope`"));
    }

    #[test]
    fn a_search_names_the_form_around_each_matching_line() {
        let source = "(module my.mix)\n\n(def strip (t)\n  (v-stack :height 20\n    (label t.name)))\n\n;; height of the panel\n(def panel-height () 20)\n";
        let found = search(source, &regex::Regex::new("height").unwrap(), 10);
        assert_eq!((found.count, found.parsed), (3, true));
        assert_eq!(
            found.hits,
            vec![
                SourceHit { line: 4, form: Some(("def".into(), Some("strip".into()))), text: "(v-stack :height 20".into() },
                SourceHit { line: 7, form: None, text: ";; height of the panel".into() },
                SourceHit { line: 8, form: Some(("def".into(), Some("panel-height".into()))), text: "(def panel-height () 20)".into() },
            ]
        );
        assert_eq!(search(source, &regex::Regex::new("absent").unwrap(), 10), SourceMatches { parsed: true, ..Default::default() });
        let counted = search(source, &regex::Regex::new("height").unwrap(), 1);
        assert_eq!((counted.count, counted.hits.len()), (3, 1), "past the budget lines are only counted");
        let broken = search("(def a (\n  height", &regex::Regex::new("height").unwrap(), 10);
        assert_eq!((broken.count, broken.parsed, broken.hits[0].form.clone()), (1, false, None));
    }

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
    fn the_declared_module_is_the_first_forms_name() {
        assert_eq!(declared_module(SOURCE).as_deref(), Some("my.pulse"));
        assert_eq!(declared_module(";; (module fake)\n(module real.one) ; c\n").as_deref(), Some("real.one"));
        assert_eq!(declared_module("(def x 1)\n(module late)"), None);
        assert_eq!(declared_module("(module \"quoted\")"), None);
    }

    #[test]
    fn form_source_returns_exact_text() {
        let found = form_source(SOURCE, "pulse").unwrap();
        assert_eq!(found, vec![(10, "(def-kind pulse\n  :doc \"A pulse sequencer.\"\n  :fields ((rate 4)))".to_string())]);
        assert!(form_source(SOURCE, "missing").unwrap().is_empty());
    }
}
