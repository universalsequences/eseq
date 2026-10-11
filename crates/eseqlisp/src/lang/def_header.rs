//! Definition headers: leading `:key value` pairs in a function `def` body
//! that document it and mark it as a palette command
//! (docs/command-palette-spec.md §3).
//!
//! ```lisp
//! (def seq-theme-pick (theme)
//!   :doc "Switch the UI theme."
//!   :command "Change Theme"
//!   :choices seq-theme-choices
//!   (seq-apply-theme theme))
//! ```
//!
//! The header compiles to nothing; the compiler records it per qualified
//! global and the VM keeps the latest one for each definition.

use super::parser::Expression;

#[derive(Debug, Clone, Default)]
pub struct DefinitionHeader {
    /// `(name a b &optional c)`, rendered from the arglist.
    pub signature: String,
    /// Positional parameters before any `&optional`/`&rest`/`&key`.
    pub required_arity: usize,
    /// The first required parameter, when it is a plain symbol (not a
    /// destructuring pattern): the default choice-stage badge.
    pub first_param: Option<String>,
    pub doc: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub group: Option<String>,
    /// Unevaluated `:choices` / `:current` forms; phase 4 of the spec gives
    /// them behavior.
    pub choices: Option<Expression>,
    pub current: Option<Expression>,
}

impl DefinitionHeader {
    /// First line of `:doc`, for one-line surfaces (palette rows).
    pub fn summary(&self) -> Option<&str> {
        self.doc.as_deref().and_then(|doc| doc.lines().map(str::trim).find(|line| !line.is_empty()))
    }

    /// The badge noun for this command's choice stage: `:prompt`, else the
    /// first required parameter title-cased (`instrument-type` →
    /// `Instrument Type`).
    pub fn choice_prompt(&self) -> Option<String> {
        self.prompt.clone().or_else(|| self.first_param.as_deref().map(title_case))
    }

    /// Records one `:key value` pair. `Ok(false)` when `key` is not a header
    /// key, which ends the header.
    fn set(&mut self, name: &str, key: &str, value: &Expression) -> Result<bool, String> {
        let text = || match value {
            Expression::String(text) => Ok(text.clone()),
            _ => Err(format!("{name}: header :{key} must be a string literal")),
        };
        let duplicate = || format!("{name}: header :{key} appears twice");
        match key {
            "doc" => put(&mut self.doc, text()?, duplicate),
            "command" => put(&mut self.command, text()?, duplicate),
            "prompt" => put(&mut self.prompt, text()?, duplicate),
            "group" => put(&mut self.group, text()?, duplicate),
            "choices" => put(&mut self.choices, value.clone(), duplicate),
            "current" => put(&mut self.current, value.clone(), duplicate),
            _ => Ok(false),
        }
    }
}

fn put<T>(slot: &mut Option<T>, value: T, duplicate: impl Fn() -> String) -> Result<bool, String> {
    if slot.is_some() {
        return Err(duplicate());
    }
    *slot = Some(value);
    Ok(true)
}

fn title_case(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses the header at the head of a `def` body: the header and the index
/// of the first body form after it.
///
/// A pair is header only while at least one body form follows it, so
/// `(def f () :doc "x")` stays a function returning `"x"`. Once a header has
/// started, a trailing header pair with no body after it is an error rather
/// than a silent return value. Returns `Ok(None)` when there is no header.
pub fn split_header(
    name: &str,
    args: &[Expression],
    body: &[Expression],
) -> Result<Option<(DefinitionHeader, usize)>, String> {
    let mut header = DefinitionHeader::default();
    let mut idx = 0;
    while body.len() - idx >= 2 {
        let Expression::Keyword(key) = &body[idx] else { break };
        if body.len() - idx == 2 {
            if idx > 0 && DefinitionHeader::default().set(name, key, &body[idx + 1])? {
                return Err(format!("{name}: header :{key} needs a body form after it"));
            }
            break;
        }
        if !header.set(name, key, &body[idx + 1])? {
            break;
        }
        idx += 2;
    }
    if idx == 0 {
        return Ok(None);
    }
    header.signature = render_signature(name, args);
    let required = args
        .iter()
        .take_while(|arg| !matches!(arg, Expression::Symbol(s) if s.starts_with('&')));
    header.required_arity = required.clone().count();
    header.first_param = match required.clone().next() {
        Some(Expression::Symbol(param)) => Some(param.clone()),
        _ => None,
    };
    Ok(Some((header, idx)))
}

fn render_signature(name: &str, args: &[Expression]) -> String {
    let display = super::modules::strip_implicit(name);
    let mut signature = format!("({display}");
    for arg in args {
        signature.push(' ');
        render_arg(arg, &mut signature);
    }
    signature.push(')');
    signature
}

fn render_arg(arg: &Expression, out: &mut String) {
    match arg {
        Expression::Symbol(text) => out.push_str(text),
        Expression::Keyword(text) => {
            out.push(':');
            out.push_str(text);
        }
        Expression::String(text) => out.push_str(&super::parser::string_literal(text)),
        Expression::Number(value) => out.push_str(&value.to_string()),
        Expression::List(items) | Expression::QuoteList(items) => {
            out.push('(');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                render_arg(item, out);
            }
            out.push(')');
        }
        _ => out.push_str(&super::parser::format_expression(arg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::parser::{ASTParser, Parser};

    fn def_parts(src: &str) -> (String, Vec<Expression>, Vec<Expression>) {
        let tokens = Parser::new(src.to_string()).parse().expect("lex");
        let mut exprs = ASTParser::new(tokens).parse().expect("parse");
        let Expression::List(items) = exprs.remove(0) else { panic!("list") };
        let Expression::Symbol(name) = &items[1] else { panic!("name") };
        let Expression::List(args) = &items[2] else { panic!("args") };
        (name.clone(), args.clone(), items[3..].to_vec())
    }

    #[test]
    fn header_pairs_strip_before_body() {
        let (name, args, body) = def_parts(
            r#"(def add-instrument (instrument-type &optional slot)
                 :doc "Add a track.
                   More detail."
                 :command "Add Instrument"
                 :choices instrument-choices
                 (do-it instrument-type))"#,
        );
        let (header, body_start) = split_header(&name, &args, &body).unwrap().unwrap();
        assert_eq!(body_start, body.len() - 1);
        assert_eq!(header.signature, "(add-instrument instrument-type &optional slot)");
        assert_eq!(header.summary(), Some("Add a track."));
        assert_eq!(header.command.as_deref(), Some("Add Instrument"));
        assert!(matches!(header.choices, Some(Expression::Symbol(ref s)) if s == "instrument-choices"));
        assert_eq!(header.required_arity, 1);
        assert_eq!(header.first_param.as_deref(), Some("instrument-type"));
        assert_eq!(header.choice_prompt().as_deref(), Some("Instrument Type"));
    }

    #[test]
    fn lone_pair_is_the_body_not_a_header() {
        let (name, args, body) = def_parts(r#"(def label () :doc "x")"#);
        assert!(split_header(&name, &args, &body).unwrap().is_none());
    }

    #[test]
    fn unknown_keyword_ends_the_header() {
        let (name, args, body) = def_parts(r#"(def f () :doc "d" :width 3 (g))"#);
        let (header, body_start) = split_header(&name, &args, &body).unwrap().unwrap();
        assert_eq!(header.doc.as_deref(), Some("d"));
        assert_eq!(body_start, 2);
    }

    #[test]
    fn non_string_doc_and_duplicates_are_errors() {
        let (name, args, body) = def_parts(r#"(def f () :doc 3 (g))"#);
        assert!(split_header(&name, &args, &body).unwrap_err().contains(":doc must be a string"));
        let (name, args, body) = def_parts(r#"(def f () :doc "a" :doc "b" (g))"#);
        assert!(split_header(&name, &args, &body).unwrap_err().contains("twice"));
    }

    #[test]
    fn header_pair_without_a_body_after_it_is_an_error() {
        let (name, args, body) = def_parts(r#"(def f () :doc "a" :command "b")"#);
        assert!(split_header(&name, &args, &body).unwrap_err().contains("needs a body form"));
    }

    #[test]
    fn explicit_prompt_wins_over_param_name() {
        let (name, args, body) = def_parts(r#"(def f (theme) :prompt "Look" (g theme))"#);
        let (header, _) = split_header(&name, &args, &body).unwrap().unwrap();
        assert_eq!(header.choice_prompt().as_deref(), Some("Look"));
    }
}
