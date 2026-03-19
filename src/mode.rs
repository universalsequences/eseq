use std::collections::{HashMap, HashSet};

use crate::buffer::Buffer;
use crate::runtime::SymbolMetadata;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BufferMode {
    ESeqLisp,
    DGenLisp,
    Named(String),
}

impl BufferMode {
    pub fn name(&self) -> &str {
        match self {
            BufferMode::ESeqLisp => "eseqlisp-mode",
            BufferMode::DGenLisp => "dgenlisp-mode",
            BufferMode::Named(name) => name,
        }
    }
}

impl Default for BufferMode {
    fn default() -> Self {
        Self::ESeqLisp
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    Comment,
    String,
    Number,
    Keyword,
    Builtin,
    Special,
    Delimiter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpan {
    pub start: usize,
    pub end: usize,
    pub class: TokenClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionMatch {
    pub start_col: usize,
    pub prefix: String,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub signature: Option<String>,
    pub docs: Option<String>,
}

const ESEQLISP_SPECIALS: &[(&str, &str, &str)] = &[
    ("def", "(def name value)", "Bind a global name."),
    ("fn", "(fn args...)", "Reference a function value."),
    (
        "lambda",
        "(lambda (args...) body...)",
        "Create an anonymous function.",
    ),
    ("if", "(if cond then else)", "Conditional expression."),
    (
        "let",
        "(let ((name value) ...) body...)",
        "Lexical bindings for body expressions.",
    ),
    (
        "do",
        "(do expr...)",
        "Evaluate expressions in sequence and return the last.",
    ),
    (
        "eval",
        "(eval source)",
        "Compile and run a Lisp source string.",
    ),
];

const ESEQLISP_BUILTINS: &[(&str, &str, &str)] = &[
    ("append", "(append list ...)", "Concatenate lists."),
    (
        "clear-hooks",
        "(clear-hooks)",
        "Remove all registered sequencer hook callbacks.",
    ),
    ("cons", "(cons value list)", "Prepend a value to a list."),
    (
        "compile-current",
        "(compile-current)",
        "Ask the host to compile the current buffer.",
    ),
    (
        "dict",
        "(dict :key value ...)",
        "Create a map from keyword/value pairs.",
    ),
    ("empty?", "(empty? xs)", "Return true when a list is empty."),
    (
        "eval-buffer-command",
        "(eval-buffer-command)",
        "Evaluate the entire current buffer.",
    ),
    (
        "eval-sexp",
        "(eval-sexp)",
        "Evaluate the s-expression at the cursor.",
    ),
    (
        "every",
        "(every unit interval form)",
        "Register a repeating hook that runs a quoted form on the host schedule.",
    ),
    (
        "filter",
        "(filter fn xs)",
        "Return a list of items where fn returns truthy.",
    ),
    ("first", "(first list)", "Return the first item in a list."),
    (
        "for-each",
        "(for-each fn xs)",
        "Call fn for each item in a list, for side effects.",
    ),
    ("get", "(get map :key)", "Lookup a keyword in a map."),
    ("keys", "(keys map)", "Return map keys as keywords."),
    ("len", "(len value)", "Length of a list or string."),
    ("list", "(list item ...)", "Create a list."),
    (
        "map",
        "(map fn xs)",
        "Return a list produced by applying fn to each item.",
    ),
    (
        "max",
        "(max a b ...)",
        "Return the largest numeric argument.",
    ),
    (
        "merge",
        "(merge map :key value ...)",
        "Return a new map with overrides.",
    ),
    (
        "min",
        "(min a b ...)",
        "Return the smallest numeric argument.",
    ),
    ("not", "(not value)", "Logical negation."),
    (
        "nth",
        "(nth list idx)",
        "Return the 0-based item from a list.",
    ),
    (
        "rand-int",
        "(rand-int end) or (rand-int start end)",
        "Pseudo-random integer.",
    ),
    (
        "range",
        "(range end) or (range start end)",
        "Build a numeric range list.",
    ),
    (
        "reduce",
        "(reduce fn acc xs)",
        "Fold a list left-to-right, carrying an accumulator.",
    ),
    (
        "rest",
        "(rest list)",
        "Return a list without its first item.",
    ),
    ("reverse", "(reverse list)", "Reverse a list."),
    (
        "save-current-buffer",
        "(save-current-buffer)",
        "Save the current buffer through the editor.",
    ),
    (
        "source",
        "(source value ...)",
        "Render evaluable Lisp source.",
    ),
    ("str", "(str value ...)", "Render values to a string."),
];

const DGENLISP_SPECIALS: &[(&str, &str, &str)] = &[
    ("def", "(def name expr)", "Bind a DSP symbol."),
    (
        "defmacro",
        "(defmacro name (args...) body...)",
        "Define a reusable macro.",
    ),
    (
        "param",
        "(param name @default v @min v @max v ...)",
        "Declare a host-controllable parameter.",
    ),
    (
        "in",
        "(in channel @name label ...)",
        "Read an input channel.",
    ),
    (
        "out",
        "(out expr channel @name label)",
        "Write an output channel.",
    ),
    (
        "make-history",
        "(make-history name)",
        "Create a feedback cell.",
    ),
    (
        "read-history",
        "(read-history name)",
        "Read a feedback cell from the previous frame.",
    ),
    (
        "write-history",
        "(write-history name expr)",
        "Write a feedback cell for the current frame.",
    ),
];

const DGENLISP_BUILTINS: &[(&str, &str, &str)] = &[
    ("abs", "(abs x)", "Absolute value."),
    (
        "accum",
        "(accum inc [reset min max])",
        "Stateful accumulator.",
    ),
    (
        "biquad",
        "(biquad signal cutoff q gain mode)",
        "IIR filter.",
    ),
    ("ceil", "(ceil x)", "Round upward."),
    ("click", "(click)", "Impulse generator."),
    ("clip", "(clip sig min max)", "Clamp to range."),
    (
        "compressor",
        "(compressor signal ratio threshold knee attack release)",
        "Dynamics processor.",
    ),
    ("cos", "(cos x)", "Cosine."),
    ("delay", "(delay signal time-in-samples)", "Delay line."),
    ("eq", "(eq a b)", "Equality comparison."),
    ("exp", "(exp x)", "Exponential."),
    ("floor", "(floor x)", "Round downward."),
    (
        "gswitch",
        "(gswitch cond a b)",
        "Conditional signal switch.",
    ),
    ("gte", "(gte a b)", "Greater-than-or-equal comparison."),
    ("gt", "(gt a b)", "Greater-than comparison."),
    ("latch", "(latch value trigger)", "Sample and hold."),
    ("log", "(log x)", "Natural logarithm."),
    ("lte", "(lte a b)", "Less-than-or-equal comparison."),
    ("lt", "(lt a b)", "Less-than comparison."),
    ("max", "(max a b ...)", "Maximum value."),
    ("min", "(min a b ...)", "Minimum value."),
    ("mix", "(mix a b t)", "Linear interpolation."),
    (
        "mod",
        "(mod param-name)",
        "Read the modulated value for a modulatable param.",
    ),
    ("mse", "(mse prediction target)", "Mean squared error."),
    ("noise", "(noise)", "White noise source."),
    ("phasor", "(phasor freq [reset])", "Ramp oscillator."),
    ("pow", "(pow base exponent)", "Exponentiation."),
    ("relu", "(relu x)", "Rectified linear unit."),
    ("round", "(round x)", "Round to nearest integer."),
    (
        "scale",
        "(scale sig in-min in-max out-min out-max)",
        "Linear remap.",
    ),
    (
        "selector",
        "(selector mode option1 option2 ...)",
        "1-based selector.",
    ),
    ("sigmoid", "(sigmoid x)", "Sigmoid curve."),
    ("sign", "(sign x)", "Sign function."),
    ("sin", "(sin x)", "Sine."),
    ("sqrt", "(sqrt x)", "Square root."),
    (
        "stateful-phasor",
        "(stateful-phasor freq)",
        "Forced-state phasor.",
    ),
    ("tan", "(tan x)", "Tangent."),
    ("tanh", "(tanh x)", "Hyperbolic tangent."),
    (
        "triangle",
        "(triangle phase)",
        "Triangle waveform from phasor phase.",
    ),
    ("wrap", "(wrap sig min max)", "Wrap into range."),
];

pub fn completion_match(
    mode: &BufferMode,
    buffer: &Buffer,
    runtime_symbols: &[String],
    runtime_metadata: &HashMap<String, SymbolMetadata>,
) -> Option<CompletionMatch> {
    let line = buffer.lines.get(buffer.cursor.0)?;
    let cursor_col = buffer.cursor.1.min(line.len());
    let (start_col, prefix) = symbol_prefix(line, cursor_col)?;
    let prefix_lower = prefix.to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut items = completion_candidates(mode, runtime_symbols, buffer)
        .into_iter()
        .filter(|item| {
            let label_lower = item.label.to_ascii_lowercase();
            label_lower.starts_with(&prefix_lower) && label_lower != prefix_lower
        })
        .filter(|item| seen.insert(item.label.clone()))
        .collect::<Vec<_>>();
    for item in &mut items {
        if item.signature.is_none() || item.docs.is_none() {
            if let Some(meta) = runtime_metadata.get(&item.label) {
                item.signature.get_or_insert_with(|| meta.signature.clone());
                item.docs.get_or_insert_with(|| meta.docs.clone());
            }
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    if items.is_empty() {
        return None;
    }
    Some(CompletionMatch {
        start_col,
        prefix,
        items,
    })
}

pub fn highlight_line(
    mode: &BufferMode,
    line: &str,
    runtime_symbols: &[String],
    buffer: &Buffer,
) -> Vec<TokenSpan> {
    let mut spans = Vec::new();
    let known = completion_candidates(mode, runtime_symbols, buffer)
        .into_iter()
        .map(|item| item.label)
        .collect::<HashSet<_>>();
    let bytes = line.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        match ch {
            ';' | '#' => {
                spans.push(TokenSpan {
                    start: idx,
                    end: bytes.len(),
                    class: TokenClass::Comment,
                });
                break;
            }
            '"' => {
                let start = idx;
                idx += 1;
                while idx < bytes.len() {
                    if bytes[idx] == b'"' {
                        idx += 1;
                        break;
                    }
                    idx += 1;
                }
                spans.push(TokenSpan {
                    start,
                    end: idx,
                    class: TokenClass::String,
                });
            }
            '(' | ')' | '[' | ']' | '{' | '}' => {
                spans.push(TokenSpan {
                    start: idx,
                    end: idx + 1,
                    class: TokenClass::Delimiter,
                });
                idx += 1;
            }
            _ if ch.is_whitespace() => {
                idx += 1;
            }
            _ => {
                let start = idx;
                idx += 1;
                while idx < bytes.len() && is_symbol_byte(bytes[idx]) {
                    idx += 1;
                }
                let token = &line[start..idx];
                let class = classify_token(mode, token, &known);
                if let Some(class) = class {
                    spans.push(TokenSpan {
                        start,
                        end: idx,
                        class,
                    });
                }
            }
        }
    }

    spans
}

fn classify_token(mode: &BufferMode, token: &str, known: &HashSet<String>) -> Option<TokenClass> {
    if token.is_empty() {
        return None;
    }
    if token.starts_with(':') || token.starts_with('@') {
        return Some(TokenClass::Keyword);
    }
    if token.parse::<f64>().is_ok() {
        return Some(TokenClass::Number);
    }
    if special_forms(mode)
        .iter()
        .any(|(label, _, _)| *label == token)
    {
        return Some(TokenClass::Special);
    }
    if known.contains(token) {
        return Some(TokenClass::Builtin);
    }
    None
}

fn completion_candidates(
    mode: &BufferMode,
    runtime_symbols: &[String],
    buffer: &Buffer,
) -> Vec<CompletionItem> {
    let mut items = static_items(special_forms(mode));
    items.extend(buffer_defined_symbols(buffer));
    match mode {
        BufferMode::ESeqLisp | BufferMode::Named(_) => {
            items.extend(static_items(ESEQLISP_BUILTINS));
            items.extend(runtime_symbols.iter().cloned().map(|label| CompletionItem {
                label,
                signature: None,
                docs: None,
            }));
        }
        BufferMode::DGenLisp => items.extend(static_items(DGENLISP_BUILTINS)),
    }
    items
}

fn buffer_defined_symbols(buffer: &Buffer) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for line in &buffer.lines {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("(def ") && !trimmed.starts_with("(defmacro ") {
            continue;
        }
        let rest = if let Some(rest) = trimmed.strip_prefix("(defmacro ") {
            rest
        } else {
            trimmed.strip_prefix("(def ").unwrap_or("")
        };
        if let Some(name) = rest.split_whitespace().next() {
            let name = name.trim_matches(|ch: char| ch == '(' || ch == ')');
            if !name.is_empty() {
                items.push(CompletionItem {
                    label: name.to_string(),
                    signature: Some(format!("({name} ...)")),
                    docs: Some("User-defined symbol from the current buffer.".to_string()),
                });
            }
        }
    }
    items
}

fn special_forms(mode: &BufferMode) -> &'static [(&'static str, &'static str, &'static str)] {
    match mode {
        BufferMode::ESeqLisp | BufferMode::Named(_) => ESEQLISP_SPECIALS,
        BufferMode::DGenLisp => DGENLISP_SPECIALS,
    }
}

fn static_items(entries: &[(&str, &str, &str)]) -> Vec<CompletionItem> {
    entries
        .iter()
        .map(|(label, signature, docs)| CompletionItem {
            label: (*label).to_string(),
            signature: Some((*signature).to_string()),
            docs: Some((*docs).to_string()),
        })
        .collect()
}

fn symbol_prefix(line: &str, cursor_col: usize) -> Option<(usize, String)> {
    if cursor_col == 0 {
        return None;
    }
    let bytes = line.as_bytes();
    let mut start = cursor_col.min(bytes.len());
    while start > 0 && is_symbol_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start == cursor_col || start >= bytes.len() && cursor_col == 0 {
        return None;
    }
    let prefix = line[start..cursor_col.min(bytes.len())].to_ascii_lowercase();
    if prefix.is_empty() {
        return None;
    }
    Some((start, prefix))
}

fn is_symbol_byte(byte: u8) -> bool {
    let ch = byte as char;
    !ch.is_whitespace()
        && !matches!(
            ch,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | ';' | '#'
        )
}

#[cfg(test)]
mod tests {
    use super::{BufferMode, completion_match, highlight_line};
    use crate::buffer::Buffer;
    use crate::runtime::SymbolMetadata;
    use std::collections::HashMap;

    #[test]
    fn eseqlisp_completion_uses_runtime_symbols() {
        let mut buffer = Buffer::from_text(0, "*test*", "(seq-");
        buffer.cursor = (0, 5);
        let result = completion_match(
            &BufferMode::ESeqLisp,
            &buffer,
            &[String::from("seq-step"), String::from("seq-track-steps")],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(result.start_col, 1);
        assert!(result.items.iter().any(|item| item.label == "seq-step"));
    }

    #[test]
    fn dgenlisp_highlights_param_keywords() {
        let buffer = Buffer::from_text(0, "*test*", "(param freq @default 440)");
        let spans = highlight_line(&BufferMode::DGenLisp, &buffer.lines[0], &[], &buffer);
        assert!(
            spans
                .iter()
                .any(|span| span.class == super::TokenClass::Keyword)
        );
        assert!(
            spans
                .iter()
                .any(|span| span.class == super::TokenClass::Special)
        );
    }

    #[test]
    fn runtime_metadata_is_attached_to_completion_items() {
        let mut buffer = Buffer::from_text(0, "*test*", "(seq-");
        buffer.cursor = (0, 5);
        let mut metadata = HashMap::new();
        metadata.insert(
            "seq-step".to_string(),
            SymbolMetadata {
                signature: "(seq-step step)".to_string(),
                docs: "Return a step snapshot.".to_string(),
            },
        );
        let result = completion_match(
            &BufferMode::ESeqLisp,
            &buffer,
            &[String::from("seq-step")],
            &metadata,
        )
        .unwrap();
        let item = result
            .items
            .iter()
            .find(|item| item.label == "seq-step")
            .unwrap();
        assert_eq!(item.signature.as_deref(), Some("(seq-step step)"));
        assert_eq!(item.docs.as_deref(), Some("Return a step snapshot."));
    }

    #[test]
    fn completion_matches_runtime_symbols_case_insensitively() {
        let mut buffer = Buffer::from_text(0, "*test*", "(MODU");
        buffer.cursor = (0, 5);
        let result = completion_match(
            &BufferMode::ESeqLisp,
            &buffer,
            &[String::from("MODUM_DELAY")],
            &HashMap::new(),
        )
        .unwrap();
        assert!(result.items.iter().any(|item| item.label == "MODUM_DELAY"));
    }
}
