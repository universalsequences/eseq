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
    /// A `button-icon` glyph name (`:save`, `:swatch`), drawn before the
    /// palette row's title.
    pub icon: Option<String>,
    /// Where the command is bound outside the editor's keymap (a menu
    /// accelerator, `s-s`), shown in the palette's shortcut column.
    pub shortcut: Option<String>,
    /// `:pinned true`: listed when the palette opens, before anything is
    /// typed (the home list, in definition order).
    pub pinned: bool,
    /// `:pinned N`: pinned at the very top of the home list, above the
    /// recently run commands, ascending by `N`.
    pub pin_rank: Option<u32>,
    /// `:searchable true`: the palette's search also lists this command's
    /// final choices (each with the picks before it), so typing a preset
    /// name finds it inside its instrument.
    pub searchable: bool,
    /// `:choices`: one step per argument the palette picks, in order. The
    /// symbol form `:choices themes` is one step for the first parameter;
    /// the list form `:choices (instrument instruments preset presets)`
    /// pairs parameters with sources. A source is a function or global,
    /// named by symbol and resolved in the command's module; a function is
    /// called with the values picked so far (spec §6).
    pub choices: Vec<ChoiceStep>,
    /// `:current`: like a `:choices` source, the value a one-step command
    /// holds now.
    pub current: Option<String>,
    /// `:preview`: a function the palette calls with each choice as the
    /// highlight lands on it, then with the `:current` value it opened on if
    /// the pick is abandoned — `true` names the command itself (spec §6).
    pub preview: Option<String>,
}

/// One argument the palette picks for a command (see
/// [`DefinitionHeader::choices`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceStep {
    /// The parameter it fills; `None` for the symbol form (the first one).
    pub param: Option<String>,
    pub source: String,
}

impl DefinitionHeader {
    /// First line of `:doc`, for one-line surfaces (palette rows).
    pub fn summary(&self) -> Option<&str> {
        self.doc.as_deref().and_then(first_doc_line)
    }

    /// The badge noun for choice step `step`: `:prompt` on a one-step
    /// command, else its parameter title-cased (`instrument-type` →
    /// `Instrument Type`).
    pub fn step_prompt(&self, step: usize) -> Option<String> {
        if self.choices.len() <= 1
            && let Some(prompt) = &self.prompt
        {
            return Some(prompt.clone());
        }
        let param = self.choices.get(step).and_then(|choice| choice.param.as_deref());
        let param = param.or(if step == 0 { self.first_param.as_deref() } else { None })?;
        Some(title_case(param))
    }

    /// Why a `:command` can't run from the palette, if it can't: its
    /// required arguments must be exactly the ones `:choices` supplies
    /// (spec §4).
    pub fn command_problem(&self, name: &str) -> Option<String> {
        self.command.as_ref()?;
        let supplied = self.choices.len();
        (self.required_arity != supplied).then(|| {
            let display = super::modules::strip_implicit(name);
            if supplied > 0 {
                format!(
                    "warning: command `{display}` picks {supplied} argument(s) with :choices, \
                     so it must take exactly that many required arguments (it takes {})",
                    self.required_arity
                )
            } else {
                format!(
                    "warning: command `{display}` takes {} required argument(s); give it \
                     :choices for one, or make them &optional",
                    self.required_arity
                )
            }
        })
    }

    /// Records one `:key value` pair. `Ok(false)` when `key` is not a header
    /// key, which ends the header.
    fn set(&mut self, name: &str, key: &str, value: &Expression) -> Result<bool, String> {
        let text = || match value {
            Expression::String(text) => Ok(text.clone()),
            _ => Err(format!("{name}: header :{key} must be a string literal")),
        };
        let name_like = || match value {
            Expression::Keyword(name) | Expression::String(name) => Ok(name.clone()),
            _ => Err(format!("{name}: header :{key} must be a keyword or string")),
        };
        let symbol = || match value {
            Expression::Symbol(symbol) => Ok(symbol.clone()),
            _ => Err(format!("{name}: header :{key} must name a function or global (a symbol)")),
        };
        let duplicate = || format!("{name}: header :{key} appears twice");
        match key {
            "doc" => put(&mut self.doc, text()?, duplicate),
            "command" => put(&mut self.command, text()?, duplicate),
            "prompt" => put(&mut self.prompt, text()?, duplicate),
            "group" => put(&mut self.group, text()?, duplicate),
            "icon" => put(&mut self.icon, name_like()?, duplicate),
            "shortcut" => put(&mut self.shortcut, text()?, duplicate),
            "choices" => {
                if !self.choices.is_empty() {
                    return Err(duplicate());
                }
                self.choices = choice_steps(name, value)?;
                Ok(true)
            }
            "current" => put(&mut self.current, symbol()?, duplicate),
            "preview" => put(&mut self.preview, symbol()?, duplicate),
            "pinned" => match value {
                Expression::Number(rank) if *rank >= 0.0 && rank.fract() == 0.0 => {
                    self.pinned = true;
                    self.pin_rank = Some(*rank as u32);
                    Ok(true)
                }
                Expression::Number(_) => Err(format!("{name}: header :pinned rank must be a whole number")),
                _ => flag(&mut self.pinned, name, key, value),
            },
            "searchable" => flag(&mut self.searchable, name, key, value),
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

/// A `true`/`false` header value.
fn flag(slot: &mut bool, name: &str, key: &str, value: &Expression) -> Result<bool, String> {
    match value {
        Expression::Symbol(flag) if flag == "true" || flag == "false" => {
            *slot = flag == "true";
            Ok(true)
        }
        _ => Err(format!("{name}: header :{key} must be true or false")),
    }
}

/// `:choices`' steps: a symbol, or `(param source param source …)`.
fn choice_steps(name: &str, value: &Expression) -> Result<Vec<ChoiceStep>, String> {
    let error = || format!("{name}: header :choices must be a symbol or (param source …) pairs of symbols");
    match value {
        Expression::Symbol(source) => Ok(vec![ChoiceStep { param: None, source: source.clone() }]),
        Expression::List(items) if !items.is_empty() && items.len() % 2 == 0 => items
            .chunks(2)
            .map(|pair| match pair {
                [Expression::Symbol(param), Expression::Symbol(source)] => {
                    Ok(ChoiceStep { param: Some(param.clone()), source: source.clone() })
                }
                _ => Err(error()),
            })
            .collect(),
        _ => Err(error()),
    }
}

/// The first non-blank line of a docstring.
pub fn first_doc_line(doc: &str) -> Option<&str> {
    doc.lines().map(str::trim).find(|line| !line.is_empty())
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
        assert_eq!(header.choices, [ChoiceStep { param: None, source: "instrument-choices".into() }]);
        assert_eq!(header.required_arity, 1);
        assert_eq!(header.first_param.as_deref(), Some("instrument-type"));
        assert_eq!(header.step_prompt(0).as_deref(), Some("Instrument Type"));
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
        let (name, args, body) = def_parts(r#"(def f (x) :choices (list 1 2 3) (g x))"#);
        assert!(split_header(&name, &args, &body).unwrap_err().contains("pairs of symbols"));
        let (name, args, body) = def_parts(r#"(def f () :doc "a" :doc "b" (g))"#);
        assert!(split_header(&name, &args, &body).unwrap_err().contains("twice"));
    }

    #[test]
    fn header_pair_without_a_body_after_it_is_an_error() {
        let (name, args, body) = def_parts(r#"(def f () :doc "a" :command "b")"#);
        assert!(split_header(&name, &args, &body).unwrap_err().contains("needs a body form"));
    }

    #[test]
    fn command_arity_must_match_choices() {
        let problem = |src: &str| {
            let (name, args, body) = def_parts(src);
            split_header(&name, &args, &body).unwrap().unwrap().0.command_problem(&name)
        };
        assert!(problem(r#"(def f () :command "F" (g))"#).is_none());
        assert!(problem(r#"(def f (x &optional y) :command "F" :choices xs (g x))"#).is_none());
        assert!(problem(r#"(def f (x) :command "F" (g x))"#).unwrap().contains(":choices"));
        assert!(problem(r#"(def f () :command "F" :choices xs (g))"#).unwrap().contains("exactly that many"));
        assert!(problem(r#"(def f (a b) :command "F" :choices (a as b bs) (g a b))"#).is_none());
        assert!(problem(r#"(def f (a) :command "F" :choices (a as b bs) (g a))"#).is_some());
        assert!(problem(r#"(def f (x) :doc "not a command" (g x))"#).is_none());
    }

    #[test]
    fn list_choices_name_a_prompt_per_parameter() {
        let (name, args, body) =
            def_parts(r#"(def f (instrument preset) :choices (instrument instruments preset presets-for) (g))"#);
        let (header, _) = split_header(&name, &args, &body).unwrap().unwrap();
        assert_eq!(header.choices.len(), 2);
        assert_eq!(header.choices[1].source, "presets-for");
        assert_eq!(header.step_prompt(0).as_deref(), Some("Instrument"));
        assert_eq!(header.step_prompt(1).as_deref(), Some("Preset"));
    }

    #[test]
    fn explicit_prompt_wins_over_param_name() {
        let (name, args, body) = def_parts(r#"(def f (theme) :prompt "Look" :icon :swatch :shortcut "s-t" (g theme))"#);
        let (header, _) = split_header(&name, &args, &body).unwrap().unwrap();
        assert_eq!(header.step_prompt(0).as_deref(), Some("Look"));
        assert_eq!(header.icon.as_deref(), Some("swatch"));
        assert_eq!(header.shortcut.as_deref(), Some("s-t"));
    }
}
