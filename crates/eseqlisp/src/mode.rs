use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

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
    pub category: Option<String>,
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
        "eval-current-buffer",
        "(eval-current-buffer)",
        "Evaluate the current buffer through the editor reload pipeline.",
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
    (
        "zip",
        "(zip xs ys ...)",
        "Combine lists positionally, stopping at the shortest input.",
    ),
];

/// Tokens highlighted as special forms in DGenLisp buffers.
///
/// Signatures and docs for these names come from the generated manifest (see
/// [`dgenlisp_manifest_items`]), so this list decides syntax emphasis only and
/// deliberately carries names alone — a second doc catalog is exactly what the
/// manifest exists to prevent.
const DGENLISP_HIGHLIGHT_SPECIALS: &[&str] = &[
    "def",
    "defmacro",
    "param",
    "in",
    "out",
    "make-history",
    "read-history",
    "write-history",
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
    let candidates = if prefix.starts_with(':') {
        contextual_keyword_candidates(buffer, start_col, runtime_metadata)?
    } else {
        completion_candidates(mode, runtime_symbols, buffer)
    };
    let mut items = candidates
        .into_iter()
        .filter(|item| {
            let label_lower = item.label.to_ascii_lowercase();
            label_lower.starts_with(&prefix_lower)
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

pub fn has_completion_prefix(buffer: &Buffer) -> bool {
    buffer
        .lines
        .get(buffer.cursor.0)
        .and_then(|line| symbol_prefix(line, buffer.cursor.1.min(line.len())))
        .is_some()
}

pub fn highlight_line(
    mode: &BufferMode,
    line: &str,
    runtime_symbols: &[String],
    buffer: &Buffer,
) -> Vec<TokenSpan> {
    let known = completion_labels(mode, runtime_symbols, buffer);
    highlight_line_with_known(mode, line, &known)
}

pub fn highlight_lines<'a>(
    mode: &BufferMode,
    lines: impl IntoIterator<Item = &'a String>,
    runtime_symbols: &[String],
    buffer: &Buffer,
) -> Vec<Vec<TokenSpan>> {
    let known = completion_labels(mode, runtime_symbols, buffer);
    lines
        .into_iter()
        .map(|line| highlight_line_with_known(mode, line, &known))
        .collect()
}

fn highlight_line_with_known(
    mode: &BufferMode,
    line: &str,
    known: &HashSet<Cow<'_, str>>,
) -> Vec<TokenSpan> {
    let mut spans = Vec::new();
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

fn classify_token(
    mode: &BufferMode,
    token: &str,
    known: &HashSet<Cow<'_, str>>,
) -> Option<TokenClass> {
    if token.is_empty() {
        return None;
    }
    if token.starts_with(':') || token.starts_with('@') {
        return Some(TokenClass::Keyword);
    }
    if token.parse::<f64>().is_ok() {
        return Some(TokenClass::Number);
    }
    if is_special_form(mode, token) {
        return Some(TokenClass::Special);
    }
    if known.contains(token) {
        return Some(TokenClass::Builtin);
    }
    None
}

fn contextual_keyword_candidates(
    buffer: &Buffer,
    prefix_start_col: usize,
    runtime_metadata: &HashMap<String, SymbolMetadata>,
) -> Option<Vec<CompletionItem>> {
    let (callee, expects_keyword) = enclosing_form_context(buffer, prefix_start_col)?;
    if !expects_keyword {
        return None;
    }
    let metadata = runtime_metadata.get(&callee)?;
    let mut seen = HashSet::new();
    let items = metadata
        .keyword_args
        .iter()
        .filter(|keyword| seen.insert((*keyword).clone()))
        .map(|keyword| CompletionItem {
            label: keyword.clone(),
            category: Some(format!("{callee} keyword")),
            signature: Some(metadata.signature.clone()),
            docs: Some(format!("Keyword argument for {callee}.\n{}", metadata.docs)),
        })
        .collect::<Vec<_>>();
    (!items.is_empty()).then_some(items)
}

/// Return the innermost open form's callee and whether the cursor is at a
/// keyword-name position. The scanner deliberately considers only top-level
/// arguments in that form, so nested callback/list expressions do not disturb
/// keyword/value pairing.
fn enclosing_form_context(buffer: &Buffer, prefix_start_col: usize) -> Option<(String, bool)> {
    let mut source = String::new();
    for (row, line) in buffer.lines.iter().enumerate().take(buffer.cursor.0 + 1) {
        if row == buffer.cursor.0 {
            source.push_str(&line[..prefix_start_col.min(line.len())]);
        } else {
            source.push_str(line);
            source.push('\n');
        }
    }

    let bytes = source.as_bytes();
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
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
            continue;
        }
        match byte {
            b';' | b'#' => in_comment = true,
            b'"' => in_string = true,
            b'(' => stack.push(index),
            b')' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let open = *stack.last()?;
    let tokens = top_level_form_tokens(&source[open + 1..]);
    let callee = tokens.first()?.clone();

    let mut expects_keyword = true;
    for token in tokens.iter().skip(1) {
        if expects_keyword && token.starts_with(':') {
            expects_keyword = false;
        } else if !expects_keyword {
            expects_keyword = true;
        }
    }
    Some((callee, expects_keyword))
}

fn top_level_form_tokens(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut token_start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    let finish_token = |end: usize, token_start: &mut Option<usize>, tokens: &mut Vec<String>| {
        if let Some(start) = token_start.take() {
            tokens.push(source[start..end].to_string());
        }
    };

    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                if depth == 0 {
                    finish_token(index + 1, &mut token_start, &mut tokens);
                }
            }
            continue;
        }
        match byte {
            b';' | b'#' if depth == 0 => {
                finish_token(index, &mut token_start, &mut tokens);
                in_comment = true;
            }
            b'"' => {
                if depth == 0 && token_start.is_none() {
                    token_start = Some(index);
                }
                in_string = true;
            }
            b'(' | b'[' | b'{' => {
                if depth == 0 {
                    finish_token(index, &mut token_start, &mut tokens);
                    token_start = Some(index);
                }
                depth += 1;
            }
            b')' | b']' | b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    finish_token(index + 1, &mut token_start, &mut tokens);
                }
            }
            byte if depth == 0 && (byte as char).is_whitespace() => {
                finish_token(index, &mut token_start, &mut tokens);
            }
            _ if depth == 0 && token_start.is_none() => token_start = Some(index),
            _ => {}
        }
    }
    finish_token(source.len(), &mut token_start, &mut tokens);
    tokens
}

fn completion_candidates(
    mode: &BufferMode,
    runtime_symbols: &[String],
    buffer: &Buffer,
) -> Vec<CompletionItem> {
    let mut items = match mode {
        BufferMode::ESeqLisp | BufferMode::Named(_) => {
            static_items(ESEQLISP_SPECIALS, Some("special form"))
        }
        BufferMode::DGenLisp => Vec::new(),
    };
    items.extend(buffer_defined_symbols(buffer));
    match mode {
        BufferMode::ESeqLisp | BufferMode::Named(_) => {
            items.extend(static_items(ESEQLISP_BUILTINS, Some("builtin")));
            items.extend(runtime_symbols.iter().cloned().map(|label| CompletionItem {
                label,
                category: None,
                signature: None,
                docs: None,
            }));
        }
        BufferMode::DGenLisp => items.extend(dgenlisp_manifest_items().iter().cloned()),
    }
    items
}

/// Label-only view of [`completion_candidates`] for syntax highlighting.
///
/// Highlighting runs over the visible lines on every frame and only ever asks
/// whether a token is a known name, so it must not clone the signatures and
/// docs that ride along on a [`CompletionItem`] — the DGenLisp manifest alone
/// is ~200 entries with multi-line docs. Static and manifest labels are
/// borrowed; only buffer-local definitions allocate.
fn completion_labels<'a>(
    mode: &BufferMode,
    runtime_symbols: &'a [String],
    buffer: &Buffer,
) -> HashSet<Cow<'a, str>> {
    let mut labels = HashSet::new();
    labels.extend(
        buffer_defined_symbols(buffer)
            .into_iter()
            .map(|item| Cow::Owned(item.label)),
    );
    match mode {
        BufferMode::ESeqLisp | BufferMode::Named(_) => {
            labels.extend(
                ESEQLISP_SPECIALS
                    .iter()
                    .chain(ESEQLISP_BUILTINS)
                    .map(|(label, _, _)| Cow::Borrowed(*label)),
            );
            labels.extend(runtime_symbols.iter().map(|label| Cow::Borrowed(&**label)));
        }
        BufferMode::DGenLisp => labels.extend(
            dgenlisp_manifest_labels()
                .iter()
                .map(|label| Cow::Borrowed(&**label)),
        ),
    }
    labels
}

fn dgenlisp_manifest_labels() -> &'static HashSet<String> {
    static LABELS: OnceLock<HashSet<String>> = OnceLock::new();
    LABELS.get_or_init(|| {
        dgenlisp_manifest_items()
            .iter()
            .map(|item| item.label.clone())
            .collect()
    })
}

fn dgenlisp_manifest_items() -> &'static [CompletionItem] {
    static ITEMS: OnceLock<Vec<CompletionItem>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        let manifest = crate::dgenlisp::manifest();
        let mut items = Vec::new();
        for (key, fallback_category) in [
            ("operators", "operator"),
            ("special_forms", "special form"),
            ("constants", "constant"),
            ("attributes", "attribute"),
        ] {
            let entries = manifest
                .get(key)
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("bundled dgenlisp-operators.json must contain {key}"));
            for entry in entries {
                let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let category = entry
                    .get("category")
                    .and_then(serde_json::Value::as_str)
                    .filter(|category| !matches!(*category, "uncategorized" | "internal"))
                    .map(|category| match category {
                        "preamble" => "macro".to_string(),
                        other => other.replace('_', " "),
                    })
                    .unwrap_or_else(|| fallback_category.to_string());
                let signatures = entry
                    .get("signatures")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let mut docs = entry
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .into_iter()
                    .collect::<Vec<_>>();
                docs.extend(signatures.iter().skip(1).cloned());
                if let Some(inputs) = manifest_port_summary(entry, "inputs") {
                    docs.push(format!("inlets: {inputs}"));
                }
                if let Some(outputs) = manifest_port_summary(entry, "outputs") {
                    docs.push(format!("outlets: {outputs}"));
                }
                let item = CompletionItem {
                    label: name.to_string(),
                    category: Some(category),
                    signature: signatures.first().cloned(),
                    docs: (!docs.is_empty()).then(|| docs.join("\n")),
                };
                items.push(item.clone());
                if let Some(aliases) = entry.get("aliases").and_then(serde_json::Value::as_array) {
                    for alias in aliases.iter().filter_map(serde_json::Value::as_str) {
                        let mut alias_item = item.clone();
                        alias_item.label = alias.to_string();
                        items.push(alias_item);
                    }
                }
            }
        }
        items
    })
}

fn manifest_port_summary(entry: &serde_json::Value, key: &str) -> Option<String> {
    let ports = entry.get(key)?.as_array()?;
    if ports.is_empty() {
        return None;
    }
    Some(
        ports
            .iter()
            .map(|port| {
                let name = port
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("value");
                let kind = port.get("kind").and_then(serde_json::Value::as_str);
                match kind {
                    Some(kind) => format!("{name}: {kind}"),
                    None => name.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
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
                    category: Some("local".to_string()),
                    signature: Some(format!("({name} ...)")),
                    docs: Some("User-defined symbol from the current buffer.".to_string()),
                });
            }
        }
    }
    items
}

fn is_special_form(mode: &BufferMode, token: &str) -> bool {
    match mode {
        BufferMode::ESeqLisp | BufferMode::Named(_) => ESEQLISP_SPECIALS
            .iter()
            .any(|(label, _, _)| *label == token),
        BufferMode::DGenLisp => DGENLISP_HIGHLIGHT_SPECIALS.contains(&token),
    }
}

fn static_items(entries: &[(&str, &str, &str)], category: Option<&str>) -> Vec<CompletionItem> {
    entries
        .iter()
        .map(|(label, signature, docs)| CompletionItem {
            label: (*label).to_string(),
            category: category.map(str::to_string),
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
    use super::{
        BufferMode, completion_candidates, completion_labels, completion_match, highlight_line,
        highlight_lines,
    };
    use crate::buffer::Buffer;
    use crate::runtime::SymbolMetadata;
    use std::borrow::Cow;
    use std::collections::{HashMap, HashSet};

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
    fn completion_suggests_contextual_keywords_from_callee_metadata() {
        let mut buffer = Buffer::from_text(0, "*test*", "(box :backg");
        buffer.cursor = (0, buffer.lines[0].len());
        let metadata = HashMap::from([(
            "box".to_string(),
            SymbolMetadata {
                signature: "(box :background color :border-color color :padding cells child ...)"
                    .to_string(),
                docs: "Layout container.".to_string(),
                keyword_args: vec![
                    ":background".to_string(),
                    ":border-color".to_string(),
                    ":padding".to_string(),
                ],
            },
        )]);

        let result = completion_match(&BufferMode::ESeqLisp, &buffer, &[], &metadata).unwrap();

        assert_eq!(result.start_col, 5);
        assert_eq!(
            result.items.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(),
            vec![":background"]
        );
    }

    #[test]
    fn bare_colon_lists_all_contextual_keywords() {
        let mut buffer = Buffer::from_text(0, "*test*", "(def-sequencer \"pulse\" :");
        buffer.cursor = (0, buffer.lines[0].len());
        let metadata = HashMap::from([(
            "def-sequencer".to_string(),
            SymbolMetadata {
                signature: "(def-sequencer name :resolution value :tick callback :init callback)"
                    .to_string(),
                docs: "Define a sequencer.".to_string(),
                keyword_args: vec![
                    ":resolution".to_string(),
                    ":tick".to_string(),
                    ":init".to_string(),
                ],
            },
        )]);

        let result = completion_match(&BufferMode::ESeqLisp, &buffer, &[], &metadata).unwrap();
        let labels = result.items.iter().map(|item| item.label.as_str()).collect::<Vec<_>>();

        assert_eq!(labels, vec![":init", ":resolution", ":tick"]);
    }

    #[test]
    fn keyword_valued_signature_examples_are_not_treated_as_argument_names() {
        let mut buffer = Buffer::from_text(0, "*test*", "(seq-emit :");
        buffer.cursor = (0, buffer.lines[0].len());
        let metadata = HashMap::from([(
            "seq-emit".to_string(),
            SymbolMetadata {
                signature: "(seq-emit :track track :at :now)".to_string(),
                docs: "Emit an event.".to_string(),
                keyword_args: vec![":track".to_string(), ":at".to_string()],
            },
        )]);

        let result = completion_match(&BufferMode::ESeqLisp, &buffer, &[], &metadata).unwrap();
        let labels = result.items.iter().map(|item| item.label.as_str()).collect::<Vec<_>>();

        assert_eq!(labels, vec![":at", ":track"]);
    }

    #[test]
    fn contextual_keywords_are_not_offered_in_a_keyword_value_position() {
        let mut buffer = Buffer::from_text(0, "*test*", "(box :background :");
        buffer.cursor = (0, buffer.lines[0].len());
        let metadata = HashMap::from([(
            "box".to_string(),
            SymbolMetadata {
                signature: "(box :background color :padding cells child ...)".to_string(),
                docs: "Layout container.".to_string(),
                keyword_args: vec![":background".to_string(), ":padding".to_string()],
            },
        )]);

        assert!(completion_match(&BufferMode::ESeqLisp, &buffer, &[], &metadata).is_none());
    }

    #[test]
    fn contextual_keyword_pairing_ignores_nested_forms() {
        let mut buffer = Buffer::from_text(
            0,
            "*test*",
            "(box :on-click (lambda (x) (do x))\n  :backg",
        );
        buffer.cursor = (1, buffer.lines[1].len());
        let metadata = HashMap::from([(
            "box".to_string(),
            SymbolMetadata {
                signature: "(box :background color :on-click callback child ...)".to_string(),
                docs: "Layout container.".to_string(),
                keyword_args: vec![":background".to_string(), ":on-click".to_string()],
            },
        )]);

        let result = completion_match(&BufferMode::ESeqLisp, &buffer, &[], &metadata).unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].label, ":background");
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
    fn dgenlisp_completion_uses_generated_operator_manifest() {
        let mut buffer = Buffer::from_text(0, "*dsp*", "(polyb");
        buffer.cursor = (0, 6);
        let result = completion_match(
            &BufferMode::DGenLisp,
            &buffer,
            &[String::from("polybook-from-eseqlisp-runtime")],
            &HashMap::new(),
        )
        .expect("DGenLisp manifest should provide polyblep completions");
        let polyblep = result
            .items
            .iter()
            .find(|item| item.label == "polyblep")
            .expect("polyblep completion");

        assert_eq!(polyblep.category.as_deref(), Some("macro"));
        assert_eq!(polyblep.signature.as_deref(), Some("(polyblep phase freq)"));
        assert!(
            polyblep
                .docs
                .as_deref()
                .is_some_and(|docs| !docs.is_empty())
        );
        assert!(
            !result
                .items
                .iter()
                .any(|item| item.label == "polybook-from-eseqlisp-runtime"),
            "ESeqLisp runtime symbols must not leak into DGenLisp buffers"
        );
    }

    #[test]
    fn eseqlisp_completion_does_not_include_dgenlisp_manifest_symbols() {
        let mut buffer = Buffer::from_text(0, "*ui*", "(polyb");
        buffer.cursor = (0, 6);

        assert!(
            completion_match(&BufferMode::ESeqLisp, &buffer, &[], &HashMap::new()).is_none(),
            "ESeqLisp buffers should complete from ESeqLisp symbols, not DGenLisp"
        );
    }

    #[test]
    fn completion_labels_match_completion_candidate_labels() {
        let buffer = Buffer::from_text(0, "*dsp*", "(def local 1)");
        let runtime = vec![String::from("seq-step")];
        for mode in [
            BufferMode::DGenLisp,
            BufferMode::ESeqLisp,
            BufferMode::Named("custom".to_string()),
        ] {
            let expected = completion_candidates(&mode, &runtime, &buffer)
                .into_iter()
                .map(|item| item.label)
                .collect::<HashSet<_>>();
            let actual = completion_labels(&mode, &runtime, &buffer)
                .into_iter()
                .map(Cow::into_owned)
                .collect::<HashSet<_>>();
            assert_eq!(
                actual, expected,
                "highlighting must see the same names as completion in {mode:?}"
            );
        }
    }

    #[test]
    fn batch_highlight_matches_single_line_highlight() {
        let buffer = Buffer::from_text(0, "*test*", "(def local 1)\n(local seq-step)");
        let symbols = vec![String::from("seq-step")];
        let batched = highlight_lines(
            &BufferMode::ESeqLisp,
            buffer.lines.iter(),
            &symbols,
            &buffer,
        );
        let single = buffer
            .lines
            .iter()
            .map(|line| highlight_line(&BufferMode::ESeqLisp, line, &symbols, &buffer))
            .collect::<Vec<_>>();
        assert_eq!(batched, single);
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
                keyword_args: Vec::new(),
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

    #[test]
    fn completion_keeps_exact_special_form_match_visible() {
        let mut buffer = Buffer::from_text(0, "*test*", "(def");
        buffer.cursor = (0, 4);
        let result = completion_match(&BufferMode::ESeqLisp, &buffer, &[], &HashMap::new())
            .expect("exact special form should still produce a completion item");

        assert!(result.items.iter().any(|item| item.label == "def"));
    }
}
