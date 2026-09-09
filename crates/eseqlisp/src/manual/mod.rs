//! Manual page parser: the markdown subset from `docs/manual-spec.md` §2/§3
//! to the Lisp AST of §4.
//!
//! The parser is total. Anything it does not recognise falls back to literal
//! paragraph text; it never returns an error for malformed markdown. The only
//! failure the natives report is an unreadable file.
//!
//! Span granularity (spec §4, decided here for `eseq-ug3m.3`): a plain-text
//! run between styled fragments is one `span` whose text may contain spaces.
//! The renderer splits spans on whitespace when it needs per-word wrapping;
//! the AST stays compact and the exporter gets whole runs.

mod inline;
mod parse;
mod runs;

use std::cell::RefCell;
use std::rc::Rc;

use crate::runtime::Runtime;
use crate::vm::Value;

pub use inline::Inline;
pub use parse::parse_manual_source;
pub use runs::{wrap_runs, Fragment};

/// One block-level node of a page.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// `#`, `##`, `###` — level 1..=3, plain text.
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(Vec<Inline>),
    /// Fenced code; `info` is the info string (possibly empty), `code` is
    /// literal with lines joined by `\n` and no trailing newline.
    CodeBlock {
        info: String,
        code: String,
    },
    /// `- ` or `1. ` list; one paragraph of inline content per item.
    List {
        ordered: bool,
        items: Vec<Vec<Inline>>,
    },
    /// An unordered list whose every item starts with a cross-reference link.
    Menu(Vec<MenuEntry>),
}

/// One Info-style menu row: `- [label](target) description`.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    pub label: String,
    pub target: String,
    pub description: String,
}

/// A parsed page: the block sequence in source order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub blocks: Vec<Block>,
}

impl Page {
    /// The display title: text of the first `# H1`, if any (spec §1).
    pub fn title(&self) -> Option<&str> {
        self.blocks.iter().find_map(|block| match block {
            Block::Heading { level: 1, text } => Some(text.as_str()),
            _ => None,
        })
    }
}

fn cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn list(values: impl IntoIterator<Item = Value>) -> Value {
    Value::List(values.into_iter().map(cell).collect())
}

fn sym(name: &str) -> Value {
    Value::Symbol(name.to_string())
}

fn string(text: &str) -> Value {
    Value::String(text.to_string())
}

fn inline_to_value(inline: &Inline) -> Value {
    match inline {
        Inline::Text(text) => list([sym("span"), string(text)]),
        Inline::Bold(text) => list([sym("b"), string(text)]),
        Inline::Em(text) => list([sym("em"), string(text)]),
        Inline::Code(text) => list([sym("code"), string(text)]),
        Inline::Link { label, target } => list([sym("link"), string(label), string(target)]),
        Inline::Action { label, form } => list([sym("action-link"), string(label), string(form)]),
    }
}

fn inlines_to_values(head: &str, inlines: &[Inline]) -> Value {
    list(std::iter::once(sym(head)).chain(inlines.iter().map(inline_to_value)))
}

fn block_to_value(block: &Block) -> Value {
    match block {
        Block::Heading { level, text } => list([sym(&format!("h{level}")), string(text)]),
        Block::Paragraph(inlines) => inlines_to_values("p", inlines),
        Block::CodeBlock { info, code } => list([sym("code-block"), string(info), string(code)]),
        Block::List { ordered, items } => list(
            std::iter::once(sym(if *ordered { "ol" } else { "ul" }))
                .chain(items.iter().map(|item| inlines_to_values("li", item))),
        ),
        Block::Menu(entries) => list(std::iter::once(sym("menu")).chain(entries.iter().map(
            |entry| {
                list([
                    sym("entry"),
                    string(&entry.label),
                    string(&entry.target),
                    string(&entry.description),
                ])
            },
        ))),
    }
}

/// Convert a page to the `(page …)` s-expression of spec §4.
pub fn page_to_value(page: &Page) -> Value {
    list(std::iter::once(sym("page")).chain(page.blocks.iter().map(block_to_value)))
}

fn string_at(items: &[Rc<RefCell<Value>>], index: usize) -> Option<String> {
    match &*items.get(index)?.borrow() {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// Read one `(span …)`/`(b …)`/`(em …)`/`(code …)`/`(link …)`/`(action-link …)`
/// value back into an [`Inline`]. Unknown shapes become empty text.
fn inline_from_value(value: &Value) -> Inline {
    let Value::List(items) = value else {
        return Inline::Text(String::new());
    };
    let head = match items.first().map(|cell| cell.borrow().clone()) {
        Some(Value::Symbol(head)) => head,
        _ => return Inline::Text(String::new()),
    };
    let text = string_at(items, 1).unwrap_or_default();
    match head.as_str() {
        "b" => Inline::Bold(text),
        "em" => Inline::Em(text),
        "code" => Inline::Code(text),
        "link" => Inline::Link {
            label: text,
            target: string_at(items, 2).unwrap_or_default(),
        },
        "action-link" => Inline::Action {
            label: text,
            form: string_at(items, 2).unwrap_or_default(),
        },
        _ => Inline::Text(text),
    }
}

/// `((kind text [target]) …)` per group.
fn runs_to_value(groups: &[Vec<Fragment>]) -> Value {
    list(groups.iter().map(|group| {
        list(group.iter().map(|frag| {
            let mut items = vec![sym(frag.kind), string(&frag.text)];
            if let Some(target) = &frag.target {
                items.push(string(target));
            }
            list(items)
        }))
    }))
}

/// Parse a manual page from a file on disk.
pub fn parse_manual_file(path: &str) -> Result<Page, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|err| format!("parse-manual-page: cannot read {path}: {err}"))?;
    Ok(parse_manual_source(&source))
}

pub(crate) fn register_manual_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "parse-manual-page",
        "(parse-manual-page path)",
        "Parse a docs/manual/ markdown page into the (page …) AST of docs/manual-spec.md §4. Malformed markdown never fails; only an unreadable file errors.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("parse-manual-page expects a path string".to_string());
            };
            parse_manual_file(path).map(|page| page_to_value(&page))
        },
    );

    runtime.register_native_with_docs(
        "manual-wrap-runs",
        "(manual-wrap-runs inlines)",
        "Split a paragraph's inline nodes (the tail of a p/li form) on whitespace into groups of glued word fragments ((kind text [target]) …) for a wrap container.",
        |args, _ctx| {
            let inlines: Vec<Inline> = match args.first() {
                Some(Value::List(items)) => items
                    .iter()
                    .map(|cell| inline_from_value(&cell.borrow()))
                    .collect(),
                _ => return Err("manual-wrap-runs expects a list of inline nodes".to_string()),
            };
            Ok(runs_to_value(&wrap_runs(&inlines)))
        },
    );

    runtime.register_native_with_docs(
        "parse-manual-source",
        "(parse-manual-source markdown)",
        "Parse manual markdown held in a string into the (page …) AST. Used for generated ref-* nodes that never touch disk.",
        |args, _ctx| {
            let Some(Value::String(source)) = args.first() else {
                return Err("parse-manual-source expects a string".to_string());
            };
            Ok(page_to_value(&parse_manual_source(source)))
        },
    );
}

#[cfg(test)]
mod tests;
