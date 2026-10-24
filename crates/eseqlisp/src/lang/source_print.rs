//! Pretty-print Lisp data as a source file: `(pp-str form [width])` breaks a
//! form to a width, `(pp-forms forms [width])` prints one top-level form per
//! line. Atoms print as `source` prints them (symbols bare), so a program
//! built from quasiquoted templates becomes a file other s-expression tools
//! accept, such as DGenLisp.
//!
//! The reader does not treat `[`/`]` as delimiters: `[ 8 8 ]` reads as `[`,
//! `8`, `8`, `]`. Printing joins such runs without inner padding (`[8 8]`,
//! which DGenLisp reads the same, though the eseqlisp reader would then see
//! one `[8` symbol) and never breaks them across lines. An `@attr value`
//! pair, or a `:key value` pair whose value is not a list, stays on one line.

use super::vm::{Value, format_lisp_source};

pub const DEFAULT_WIDTH: usize = 96;

/// Head symbols whose first N arguments stay on the head's line when a form
/// is broken (`(def name` / `(defmacro name (params)` / `(let (bindings)`).
/// `param`, `make-history`, `write-history` and `gswitch` are DGenLisp's,
/// the main target of generated source.
fn header_args(head: &str) -> usize {
    match head {
        "defmacro" | "defn" => 2,
        "def" | "let" | "lambda" | "fn" | "if" | "when" | "param" | "make-history"
        | "write-history" | "gswitch" => 1,
        _ => 0,
    }
}

/// The `x` of a `(quote x)` pair, printed as `'x`: the eseqlisp reader has
/// no `quote` special form, so `(quote ("a" ...))` would read as a call.
fn as_quoted(items: &[Value]) -> Option<&Value> {
    match items {
        [Value::Symbol(head), quoted @ (Value::Symbol(_) | Value::List(_))] if head == "quote" => {
            Some(quoted)
        }
        _ => None,
    }
}

pub fn compact(value: &Value) -> String {
    match value {
        Value::List(items) => {
            let items: Vec<Value> = items.iter().map(|item| item.borrow().clone()).collect();
            if let Some(quoted) = as_quoted(&items) {
                return format!("'{}", compact(quoted));
            }
            let chunks = chunk_items(&items);
            format!("({})", chunks.join(" "))
        }
        other => atom(other),
    }
}

pub fn pretty(value: &Value, width: usize) -> String {
    let mut out = String::new();
    layout(value, 0, width, &mut out);
    out
}

pub fn pretty_forms(forms: &Value, width: usize) -> String {
    let Value::List(items) = forms else {
        return pretty(forms, width);
    };
    let mut out = String::new();
    for item in items {
        out.push_str(&pretty(&item.borrow(), width));
        out.push('\n');
    }
    out
}

fn atom(value: &Value) -> String {
    match value {
        Value::List(_) => compact(value),
        other => format_lisp_source(other),
    }
}

fn bracket_delta(value: &Value) -> i32 {
    let Value::Symbol(s) = value else {
        return 0;
    };
    s.chars().fold(0, |depth, ch| match ch {
        '[' => depth + 1,
        ']' => depth - 1,
        _ => depth,
    })
}

/// Group list items into unbreakable text chunks: a `[ ... ]` run becomes
/// one chunk with tidy spacing, and an `@attr` symbol joins its value.
fn chunk_items(items: &[Value]) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < items.len() {
        let (text, next) = chunk_at(items, idx);
        chunks.push(text);
        idx = next;
    }
    chunks
}

fn chunk_at(items: &[Value], start: usize) -> (String, usize) {
    let first = &items[start];
    if let Value::Symbol(s) = first
        && s.starts_with('@')
        && start + 1 < items.len()
    {
        let (value, next) = chunk_at(items, start + 1);
        return (format!("{s} {value}"), next);
    }
    // `:key value` stays together when the value is an atom; a list value
    // keeps its own line so it can still break.
    if let Value::Keyword(_) = first
        && let Some(value) = items.get(start + 1)
        && !matches!(value, Value::List(_))
    {
        let (value, next) = match value {
            Value::Keyword(_) => (atom(value), start + 2),
            _ => chunk_at(items, start + 1),
        };
        return (format!("{} {value}", atom(first)), next);
    }
    let mut depth = bracket_delta(first);
    if depth <= 0 {
        return (atom(first), start + 1);
    }
    let mut text = atom(first);
    let mut idx = start + 1;
    while idx < items.len() && depth > 0 {
        let item = &items[idx];
        let piece = atom(item);
        let glue = !(text.ends_with('[') || piece.starts_with(']'));
        if glue {
            text.push(' ');
        }
        text.push_str(&piece);
        depth += bracket_delta(item);
        idx += 1;
    }
    (text, idx)
}

fn layout(value: &Value, indent: usize, width: usize, out: &mut String) {
    let flat = compact(value);
    let Value::List(cells) = value else {
        out.push_str(&flat);
        return;
    };
    if indent + flat.len() <= width || cells.is_empty() {
        out.push_str(&flat);
        return;
    }
    let items: Vec<Value> = cells.iter().map(|item| item.borrow().clone()).collect();
    if let Some(quoted) = as_quoted(&items) {
        out.push('\'');
        layout(quoted, indent + 1, width, out);
        return;
    }
    let head = match &items[0] {
        Value::Symbol(s) => Some(s.as_str()),
        _ => None,
    };
    out.push('(');
    let child_indent = match head {
        Some(_) => indent + 2,
        None => indent + 1,
    };
    let mut idx;
    match head {
        Some(name) => {
            out.push_str(name);
            idx = 1;
            let mut kept = 0;
            while idx < items.len() && kept < header_args(name) {
                let (text, next) = chunk_at(&items, idx);
                out.push(' ');
                out.push_str(&text);
                idx = next;
                kept += 1;
            }
        }
        None => {
            layout(&items[0], indent + 1, width, out);
            idx = 1;
        }
    }
    while idx < items.len() {
        out.push('\n');
        out.push_str(&" ".repeat(child_indent));
        let item = &items[idx];
        if matches!(item, Value::List(_)) {
            layout(item, child_indent, width, out);
            idx += 1;
        } else {
            let (text, next) = chunk_at(&items, idx);
            out.push_str(&text);
            idx = next;
        }
    }
    out.push(')');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::vm::VM;

    fn eval(vm: &mut VM, source: &str) -> Value {
        vm.eval_str(source).expect("eval").expect("value")
    }

    fn vm() -> VM {
        let mut vm = VM::new(Vec::new());
        crate::lang::vm::register_core_natives(&mut vm);
        vm
    }

    #[test]
    fn compact_prints_symbols_bare_and_tidies_brackets() {
        let mut vm = vm();
        let form = eval(
            &mut vm,
            "(let ((n 80) (tiny 0.0000001))\n\
               `(make-history str_q1 @shape [ ,n ] @data [0.5 -0.25 ,tiny ]))",
        );
        assert_eq!(
            compact(&form),
            "(make-history str_q1 @shape [80] @data [0.5 -0.25 0.0000001])"
        );
    }

    #[test]
    fn strings_keywords_and_numbers_round_trip_through_the_reader() {
        let mut vm = vm();
        let source = "'(param length @default 10 @unit \"m \\\"x\\\"\" :k 2.5 -3 true nil)";
        let form = eval(&mut vm, source);
        let printed = compact(&form);
        assert_eq!(
            printed,
            "(param length @default 10 @unit \"m \\\"x\\\"\" :k 2.5 -3 true nil)"
        );
        let reread = eval(&mut vm, &format!("'{printed}"));
        assert_eq!(reread, form);
    }

    #[test]
    fn pretty_breaks_long_forms_but_keeps_headers_brackets_and_attrs() {
        let mut vm = vm();
        let form = eval(
            &mut vm,
            "'(defmacro modal-apply (h1 h2 sfree f phi)\n\
               (def sn (+ sfree (* f phi)))\n\
               (write-history h2 (read-history h1))\n\
               (make-history s1 @shape [8 8] @hop 4)\n\
               sn)",
        );
        assert_eq!(
            pretty(&form, 40),
            "(defmacro modal-apply (h1 h2 sfree f phi)\n\
             \x20 (def sn (+ sfree (* f phi)))\n\
             \x20 (write-history h2 (read-history h1))\n\
             \x20 (make-history s1 @shape [8 8] @hop 4)\n\
             \x20 sn)"
        );
    }

    #[test]
    fn pretty_keeps_keyword_value_pairs_together() {
        let mut vm = vm();
        let form = eval(&mut vm, "'(h-stack :width :fill :gap 0.35 (knob :label \"a\") (knob :label \"b\"))");
        assert_eq!(
            pretty(&form, 30),
            "(h-stack\n  :width :fill\n  :gap 0.35\n  (knob :label \"a\")\n  (knob :label \"b\"))"
        );
    }

    #[test]
    fn broken_quoted_lists_keep_the_reader_quote() {
        let mut vm = vm();
        let form = eval(&mut vm, "(list 'option (list 'quote (list \"uranium\" \"steel\" \"brass\")))");
        assert_eq!(pretty(&form, 16), "(option\n  '(\"uranium\"\n    \"steel\"\n    \"brass\"))");
    }

    #[test]
    fn natives_are_registered() {
        let mut vm = vm();
        assert_eq!(
            eval(&mut vm, "(pp-forms (list '(def a 1) '(def b 2)))"),
            Value::String("(def a 1)\n(def b 2)\n".to_string())
        );
        assert_eq!(
            eval(&mut vm, "(pp-str '(def a (+ 1 2)) 10)"),
            Value::String("(def a\n  (+ 1 2))".to_string())
        );
        assert_eq!(
            eval(&mut vm, "(symbol 'str \"_q\" 1)"),
            Value::Symbol("str_q1".to_string())
        );
    }
}
