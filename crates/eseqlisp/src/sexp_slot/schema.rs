//! The guard rails (docs/sexp-slot-spec.md §4).
//!
//! A schema is a small s-expression a host hands the widget:
//!
//! ```lisp
//! (num :min 1 :max 16 :step 1 :decimals 0 :default 4)
//! (word rev swap stac)                         ; first word is the default
//! (or (num :min 0 :max 1) (word off))
//! (form every (num :min 1 :max 16) (word rev swap))
//! (forms (word left right) (form trunc (num :min 1 :max 32 :default 3)) …)
//! (fixed (num :min 0 :max 1))                   ; never a list
//! (form seq (word :hit :cycle) (rest (num)))   ; last arg repeats: (seq :hit 0 3 7)
//! (form plock (fixed (dyn param)) (num))         ; a host-resolved string atom
//! (form plock (fixed (dyn param)) (dyn-num param (num :min -1e5 :max 1e5)))
//!                                   ; rails from the named word, else fallback
//! ```
//!
//! Every slot except `(fixed …)` also accepts a list of itself, nested freely:
//! that is how `(fast (1 2 3))` and `(every 2 (rev swap))` come for free. What
//! a list *means* (one per cycle, per fire…) is the host's business.
//!
//! Stored values: numbers stay numbers, words are strings (a word written
//! `:16t` stays the string ":16t"), lists are lists, a form is a list whose
//! first element is its head string. `(forms …)` is a row: a list of items,
//! each a word or form of one of its alternatives. A row is a sequence, not a
//! cycle; an item that is itself a list of words (`(left left right)`) is a
//! cycle of words.

use std::cell::RefCell;
use std::rc::Rc;

use crate::vm::{Value, format_lisp_source};

#[derive(Clone, Debug, PartialEq)]
pub enum Schema {
    Num {
        min: f64,
        max: f64,
        step: f64,
        decimals: u32,
        default: f64,
    },
    Word(Vec<String>),
    Or(Vec<Schema>),
    Form { head: String, args: Vec<Schema> },
    Forms(Vec<Schema>),
    Fixed(Box<Schema>),
    /// A form's last arg only: one or more values of the inner schema, so
    /// the form takes any number of trailing args (`+` inside its parens).
    Rest(Box<Schema>),
    /// A string atom whose words come from the host source of this name
    /// (`sexp_slot::dyn_words`). Any string checks: whether the source
    /// offers it is shown, never enforced, so a name survives a reroute.
    Dyn(String),
    /// A number whose rails come from the `(dyn SOURCE)` word of the nearest
    /// enclosing form that has one (`(plock "instrument:cutoff" (seq :hit 1
    /// 2))`: every number of the value takes the cutoff's range), resolved by
    /// the widget through `DynItem::num`. `fallback` is used when the word is
    /// unknown, the context is `nil`, or no lookup is at hand. `Schema::check`
    /// only type-checks it: rails (resolved or fallback) clamp and snap where
    /// the user edits, so a stored value outside them is kept.
    DynNum { source: String, fallback: NumSpec, default: f64 },
}

/// A number slot's rails, for widgets that format and scrub it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumSpec {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub decimals: u32,
}

impl NumSpec {
    /// Clamp and snap exactly as `Schema::check` does.
    pub fn snap(&self, value: f64) -> f64 {
        snap(value, self.min, self.max, self.step)
    }
}

fn items(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::List(items) => Some(items.iter().map(|item| item.borrow().clone()).collect()),
        _ => None,
    }
}

fn list(values: Vec<Value>) -> Value {
    Value::List(values.into_iter().map(|value| Rc::new(RefCell::new(value))).collect())
}

/// A word as the schema spells it: symbols and strings by name, keywords with
/// their leading `:` so `:16t` survives the round trip.
fn word_text(value: &Value) -> Option<String> {
    match value {
        Value::Symbol(name) | Value::String(name) => Some(name.clone()),
        Value::Keyword(name) => Some(format!(":{}", name.trim_start_matches(':'))),
        _ => None,
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => Some(*number),
        _ => None,
    }
}

impl Schema {
    /// Parse a schema value (usually quoted Lisp data).
    pub fn parse(value: &Value) -> Result<Schema, String> {
        let parts = items(value).ok_or_else(|| {
            format!("schema must be a list like (num …), got {}", format_lisp_source(value))
        })?;
        let head = parts.first().and_then(word_text).unwrap_or_default();
        let rest = &parts[parts.len().min(1)..];
        match head.as_str() {
            "num" => {
                let mut schema = Schema::Num {
                    min: f64::NEG_INFINITY,
                    max: f64::INFINITY,
                    step: 0.0,
                    decimals: 2,
                    default: 0.0,
                };
                let mut default = None;
                let mut index = 0;
                while index < rest.len() {
                    let key = word_text(&rest[index]).unwrap_or_default();
                    let value = rest
                        .get(index + 1)
                        .and_then(number)
                        .ok_or_else(|| format!("num {key} expects a number"))?;
                    if let Schema::Num { min, max, step, decimals, .. } = &mut schema {
                        match key.as_str() {
                            ":min" => *min = value,
                            ":max" => *max = value,
                            ":step" => *step = value.max(0.0),
                            ":decimals" => *decimals = value.max(0.0) as u32,
                            ":default" => default = Some(value),
                            _ => return Err(format!("num has no option {key}")),
                        }
                    }
                    index += 2;
                }
                if let Schema::Num { min, max, default: slot, .. } = &mut schema {
                    if *min > *max {
                        return Err("num :min is above :max".to_string());
                    }
                    *slot = default.unwrap_or(if min.is_finite() { *min } else { 0.0 });
                    *slot = slot.clamp(*min, *max);
                }
                Ok(schema)
            }
            "word" => {
                let words: Vec<String> = rest
                    .iter()
                    .map(|word| word_text(word).ok_or_else(|| "word takes words".to_string()))
                    .collect::<Result<_, _>>()?;
                if words.is_empty() {
                    return Err("word needs at least one word".to_string());
                }
                Ok(Schema::Word(words))
            }
            "or" => Ok(Schema::Or(rest.iter().map(Schema::parse).collect::<Result<_, _>>()?)),
            "form" => {
                let head = rest
                    .first()
                    .and_then(word_text)
                    .ok_or_else(|| "form needs a head word".to_string())?;
                Ok(Schema::Form {
                    head,
                    args: rest[1..].iter().map(Schema::parse).collect::<Result<_, _>>()?,
                })
            }
            "forms" => Ok(Schema::Forms(rest.iter().map(Schema::parse).collect::<Result<_, _>>()?)),
            "fixed" => match rest {
                [inner] => Ok(Schema::Fixed(Box::new(Schema::parse(inner)?))),
                _ => Err("fixed wraps exactly one schema".to_string()),
            },
            "rest" => match rest {
                [inner] => Ok(Schema::Rest(Box::new(Schema::parse(inner)?))),
                _ => Err("rest wraps exactly one schema".to_string()),
            },
            "dyn-num" => match rest {
                [source, fallback] => {
                    let source =
                        word_text(source).ok_or_else(|| "dyn-num takes a source name".to_string())?;
                    match Schema::parse(fallback)? {
                        Schema::Num { min, max, step, decimals, default } => Ok(Schema::DynNum {
                            source,
                            fallback: NumSpec { min, max, step, decimals },
                            default,
                        }),
                        _ => Err("dyn-num's fallback must be a (num …)".to_string()),
                    }
                }
                _ => Err("dyn-num takes a source name and a fallback (num …)".to_string()),
            },
            "dyn" => match rest {
                [source] => Ok(Schema::Dyn(
                    word_text(source).ok_or_else(|| "dyn takes a source name".to_string())?,
                )),
                _ => Err("dyn takes exactly one source name".to_string()),
            },
            other => Err(format!("unknown schema kind '{other}'")),
        }
    }

    /// The value a fresh slot of this schema holds.
    pub fn default_value(&self) -> Value {
        match self {
            Schema::Num { default, .. } | Schema::DynNum { default, .. } => Value::Number(*default),
            Schema::Word(words) => Value::String(words[0].clone()),
            Schema::Dyn(_) => Value::String(String::new()),
            Schema::Or(alternatives) => alternatives
                .first()
                .map(Schema::default_value)
                .unwrap_or(Value::Nil),
            Schema::Form { head, args } => list(
                std::iter::once(Value::String(head.clone()))
                    .chain(args.iter().map(Schema::default_value))
                    .collect(),
            ),
            Schema::Forms(_) => list(Vec::new()),
            Schema::Fixed(inner) | Schema::Rest(inner) => inner.default_value(),
        }
    }

    /// Check `value` against the rails and return its stored form: numbers
    /// clamped (and snapped to :step), words as strings. A wrong type or an
    /// unknown word is an error with a short reason.
    pub fn check(&self, value: &Value) -> Result<Value, String> {
        self.check_slot(value, true)
    }

    fn check_slot(&self, value: &Value, lists_ok: bool) -> Result<Value, String> {
        match self {
            Schema::Fixed(inner) => {
                if matches!(value, Value::List(_)) && !matches!(**inner, Schema::Form { .. }) {
                    return Err("this value cannot cycle; give one value".to_string());
                }
                inner.check_slot(value, false)
            }
            Schema::Forms(alternatives) => {
                let entries = items(value).ok_or_else(|| "expected a list of modifiers".to_string())?;
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        check_item(alternatives, item)
                            .map_err(|reason| format!("item {}: {reason}", index + 1))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(list)
            }
            Schema::Form { head, args } => check_form(head, args, value),
            Schema::Rest(inner) => inner.check_slot(value, lists_ok),
            _ => {
                if let Some(form) = self.or_form_for(value) {
                    return form.check_slot(value, lists_ok);
                }
                if let Some(elements) = items(value) {
                    if !lists_ok {
                        return Err("this value cannot cycle; give one value".to_string());
                    }
                    if elements.is_empty() {
                        return Err("a list needs at least one element".to_string());
                    }
                    return elements
                        .iter()
                        .map(|element| self.check_slot(element, true))
                        .collect::<Result<Vec<_>, _>>()
                        .map(list);
                }
                self.check_atom(value)
            }
        }
    }

    /// For an `or`: the form alternative a list headed by its head word is.
    /// Such a list is that form, not a cycle — `(every 2 (fast 2))` under
    /// `(or (word …) (form fast …))`.
    fn or_form_for(&self, value: &Value) -> Option<&Schema> {
        let Schema::Or(alternatives) = self else { return None };
        let head = items(value)?.first().and_then(word_text)?;
        alternatives.iter().find(|alternative| {
            matches!(alternative, Schema::Form { head: name, args }
                if *name == head && !args.is_empty())
        })
    }

    fn check_atom(&self, value: &Value) -> Result<Value, String> {
        match self {
            Schema::Num { min, max, step, .. } => {
                let number = number(value).ok_or_else(|| {
                    format!("expected a number, got {}", format_lisp_source(value))
                })?;
                Ok(Value::Number(snap(number, *min, *max, *step)))
            }
            // Its rails depend on a sibling word and the host's context,
            // which a bare check has neither of: the widget clamps and snaps
            // what the user edits (`Slot::snap_dyn_nums`), and whatever is
            // stored stays as is.
            Schema::DynNum { .. } => number(value)
                .map(Value::Number)
                .ok_or_else(|| format!("expected a number, got {}", format_lisp_source(value))),
            Schema::Word(words) => {
                let word = word_text(value).ok_or_else(|| {
                    format!("expected one of {}", words.join(" "))
                })?;
                if words.contains(&word) {
                    Ok(Value::String(word))
                } else {
                    Err(format!("'{word}' is not one of {}", words.join(" ")))
                }
            }
            Schema::Dyn(_) => word_text(value)
                .map(Value::String)
                .ok_or_else(|| format!("expected a name, got {}", format_lisp_source(value))),
            Schema::Or(alternatives) => {
                let mut reasons = Vec::new();
                for alternative in alternatives {
                    match alternative.check_slot(value, false) {
                        Ok(value) => return Ok(value),
                        Err(reason) => reasons.push(reason),
                    }
                }
                Err(reasons.join("; or "))
            }
            Schema::Form { head, args } => check_form(head, args, value),
            Schema::Rest(inner) => inner.check_atom(value),
            Schema::Forms(_) | Schema::Fixed(_) => self.check_slot(value, false),
        }
    }

    /// Every word this schema offers where one value goes: words, form heads,
    /// nested alternatives. Used for completions and choice popups.
    pub fn choices(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_choices(&mut out);
        out
    }

    fn collect_choices(&self, out: &mut Vec<String>) {
        match self {
            Schema::Num { .. } | Schema::Dyn(_) | Schema::DynNum { .. } => {}
            Schema::Word(words) => {
                for word in words {
                    if !out.contains(word) {
                        out.push(word.clone());
                    }
                }
            }
            Schema::Or(alternatives) | Schema::Forms(alternatives) => {
                alternatives.iter().for_each(|schema| schema.collect_choices(out));
            }
            Schema::Form { head, .. } => {
                if !out.contains(head) {
                    out.push(head.clone());
                }
            }
            Schema::Fixed(inner) | Schema::Rest(inner) => inner.collect_choices(out),
        }
    }

    /// The form alternatives this schema holds (itself, or inside or/forms).
    fn forms(&self) -> Vec<(&str, &[Schema])> {
        match self {
            Schema::Form { head, args } => vec![(head.as_str(), args.as_slice())],
            Schema::Or(alternatives) | Schema::Forms(alternatives) => {
                alternatives.iter().flat_map(Schema::forms).collect()
            }
            Schema::Fixed(inner) | Schema::Rest(inner) => inner.forms(),
            _ => Vec::new(),
        }
    }

    /// Heads a form's head may be swapped to keeping its args: the form
    /// alternatives with the same arg schemas (spec §5, head dropdown).
    pub fn compatible_heads(&self, head: &str) -> Vec<String> {
        let forms = self.forms();
        let Some((_, args)) = forms.iter().find(|(name, _)| *name == head) else {
            return Vec::new();
        };
        forms
            .iter()
            .filter(|(_, other)| same_shape(args, other))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// The number rails of a slot that holds a number (directly, fixed, or
    /// as the first number alternative of an `or`).
    pub fn num_spec(&self) -> Option<NumSpec> {
        self.num_rails().map(|(spec, _)| spec)
    }

    /// `num_spec` plus, for a `(dyn-num SOURCE …)`, its source: the spec is
    /// then the fallback the source's word may replace.
    pub fn num_rails(&self) -> Option<(NumSpec, Option<&str>)> {
        match self {
            Schema::DynNum { source, fallback, .. } => Some((*fallback, Some(source.as_str()))),
            Schema::Fixed(inner) | Schema::Rest(inner) => inner.num_rails(),
            Schema::Or(alternatives) => alternatives.iter().find_map(Schema::num_rails),
            other => other.num_spec_static().map(|spec| (spec, None)),
        }
    }

    fn num_spec_static(&self) -> Option<NumSpec> {
        match self {
            Schema::Num { min, max, step, decimals, .. } => Some(NumSpec {
                min: *min,
                max: *max,
                step: *step,
                decimals: *decimals,
            }),
            _ => None,
        }
    }

    /// Whether `head` names one of this schema's forms.
    pub fn is_form_head(&self, head: &str) -> bool {
        self.forms().iter().any(|(name, _)| *name == head)
    }

    /// A fresh `(head args…)` with every arg at its default, or `None` when
    /// `head` is not one of this schema's forms. A zero-arg form is its bare
    /// head.
    pub fn form_default(&self, head: &str) -> Option<Value> {
        let (name, args) = self.forms().into_iter().find(|(name, _)| *name == head)?;
        Some(if args.is_empty() {
            Value::String(name.to_string())
        } else {
            Schema::Form { head: name.to_string(), args: args.to_vec() }.default_value()
        })
    }

    /// The schema a form's arg `index` uses, if `head` is one of this
    /// schema's forms. Every index at or past a `(rest …)` arg is its inner
    /// schema.
    pub fn form_arg(&self, head: &str, index: usize) -> Option<&Schema> {
        let (_, args) = self.forms().into_iter().find(|(name, _)| *name == head)?;
        let arg = match args.last() {
            Some(last @ Schema::Rest(_)) if index + 1 >= args.len() => last,
            _ => args.get(index)?,
        };
        Some(match arg {
            Schema::Rest(inner) => inner,
            other => other,
        })
    }

    /// For a form whose last arg is `(rest …)`: the arg index where the
    /// repeating args start.
    pub fn form_rest_start(&self, head: &str) -> Option<usize> {
        let (_, args) = self.forms().into_iter().find(|(name, _)| *name == head)?;
        matches!(args.last(), Some(Schema::Rest(_))).then(|| args.len() - 1)
    }

    /// The schema of one element of a list held in this slot: the slot's own
    /// schema (lists of itself), or an item alternative for a row.
    pub fn element(&self) -> Option<&Schema> {
        match self {
            Schema::Fixed(_) | Schema::Form { .. } => None,
            _ => Some(self),
        }
    }

    /// Completions for a partly typed field (spec §5): `text` is everything
    /// before the cursor. Returns the words the schema allows at the cursor
    /// that start with the partial word being typed, in schema order.
    pub fn completions(&self, text: &str) -> Vec<String> {
        let (words, _, partial) = self.completion_context(text);
        words.into_iter().filter(|word| word.starts_with(partial.as_str())).collect()
    }

    /// The `(dyn …)` sources this slot draws words from (itself, or inside
    /// or/forms/fixed/rest), in schema order.
    pub fn dyn_sources(&self) -> Vec<&str> {
        match self {
            Schema::Dyn(source) => vec![source.as_str()],
            Schema::Or(alternatives) | Schema::Forms(alternatives) => {
                alternatives.iter().flat_map(Schema::dyn_sources).fold(Vec::new(), |mut out, source| {
                    if !out.contains(&source) {
                        out.push(source);
                    }
                    out
                })
            }
            Schema::Fixed(inner) | Schema::Rest(inner) => inner.dyn_sources(),
            _ => Vec::new(),
        }
    }

    /// What the cursor at the end of `text` expects: the static words (heads
    /// first after a `(`), the `dyn` sources whose words also go there, and
    /// the partial word being typed.
    pub fn completion_context(&self, text: &str) -> (Vec<String>, Vec<String>, String) {
        let (context, partial) = cursor_context(self, text);
        let Some(context) = context else {
            return (Vec::new(), Vec::new(), partial);
        };
        let sources = match context {
            Position::Value(schema) | Position::Head(schema) => {
                schema.dyn_sources().into_iter().map(str::to_string).collect()
            }
        };
        let words = match context {
            Position::Value(schema) => schema.choices(),
            Position::Head(schema) => schema
                .forms()
                .iter()
                .map(|(name, _)| name.to_string())
                .chain(schema.choices())
                .fold(Vec::new(), |mut out, word| {
                    if !out.contains(&word) {
                        out.push(word);
                    }
                    out
                }),
        };
        (words, sources, partial)
    }
}

/// Snap to :step (from :min when finite) and clamp.
fn snap(value: f64, min: f64, max: f64, step: f64) -> f64 {
    let stepped = if step > 0.0 {
        let base = if min.is_finite() { min } else { 0.0 };
        base + ((value - base) / step).round() * step
    } else {
        value
    };
    stepped.clamp(min, max)
}

fn same_shape(a: &[Schema], b: &[Schema]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| std::mem::discriminant(x) == std::mem::discriminant(y))
}

fn check_form(head: &str, args: &[Schema], value: &Value) -> Result<Value, String> {
    let parts = match items(value) {
        Some(parts) => parts,
        // A zero-arg form may be written bare.
        None if args.is_empty() && word_text(value).as_deref() == Some(head) => {
            return Ok(Value::String(head.to_string()));
        }
        None => return Err(format!("expected ({head} …)")),
    };
    if parts.first().and_then(word_text).as_deref() != Some(head) {
        return Err(format!("expected ({head} …)"));
    }
    let given = &parts[1..];
    let rest = match args.last() {
        Some(Schema::Rest(inner)) => Some(&**inner),
        _ => None,
    };
    if rest.is_none() && given.len() > args.len() {
        return Err(format!("{head} takes {} argument(s)", args.len()));
    }
    let mut out = vec![Value::String(head.to_string())];
    let count = if rest.is_some() { given.len().max(args.len()) } else { args.len() };
    for index in 0..count {
        let schema = match (rest, args.get(index)) {
            (Some(inner), None) => inner,
            (_, Some(schema)) => schema,
            (None, None) => break,
        };
        let arg = match given.get(index) {
            Some(arg) => schema
                .check_slot(arg, true)
                .map_err(|reason| format!("{head} argument {}: {reason}", index + 1))?,
            // Missing trailing args take their defaults.
            None => schema.default_value(),
        };
        out.push(arg);
    }
    Ok(list(out))
}

/// One row item: a form of that head if one matches, else a word (or a list
/// of words: a cycle) of the row's word alternatives.
fn check_item(alternatives: &[Schema], item: &Value) -> Result<Value, String> {
    let head = items(item)
        .and_then(|parts| parts.first().and_then(word_text))
        .or_else(|| word_text(item));
    for alternative in alternatives {
        for (name, args) in alternative.forms() {
            if head.as_deref() == Some(name) {
                return check_form(name, args, item);
            }
        }
    }
    let atoms = Schema::Or(
        alternatives
            .iter()
            .filter(|schema| !matches!(schema, Schema::Form { .. }))
            .cloned()
            .collect(),
    );
    let words = atoms.choices();
    match item {
        Value::List(_) => atoms.check_slot(item, true),
        _ => atoms.check_atom(item).map_err(|_| match word_text(item) {
            Some(word) => format!("'{word}' is not a modifier here"),
            None => format!("expected one of {}", words.join(" ")),
        }),
    }
}

enum Position<'a> {
    /// A value goes here.
    Value(&'a Schema),
    /// Right after `(`: a form head or, for list-able slots, a value.
    Head(&'a Schema),
}

/// Walk the typed text with a stack of (schema, how many elements so far,
/// the form head if any) to find what the cursor position expects, and the
/// partial word under the cursor.
fn cursor_context<'a>(root: &'a Schema, text: &str) -> (Option<Position<'a>>, String) {
    struct Frame<'a> {
        schema: &'a Schema,
        count: usize,
        head: Option<String>,
    }
    // Split into (, ), and atom tokens; the last atom may be partial.
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        match ch {
            '(' | ')' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            }
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    let partial = current;
    // The slot's value position is the root; a row's root expects an item.
    let mut stack: Vec<Frame<'a>> = Vec::new();
    let mut top: Option<&'a Schema> = Some(root);
    let mut at_open = false;
    for token in &tokens {
        match token.as_str() {
            "(" => {
                let Some(schema) = top else { return (None, partial) };
                stack.push(Frame { schema, count: 0, head: None });
                at_open = true;
                top = next_in(stack.last().unwrap());
            }
            ")" => {
                stack.pop();
                at_open = false;
                if let Some(frame) = stack.last_mut() {
                    frame.count += 1;
                    top = next_in(frame);
                } else {
                    top = None;
                }
            }
            atom => {
                if let Some(frame) = stack.last_mut() {
                    // only a real head opens a form: in a value list that also
                    // allows forms ((I IV V7) beside (seq …)) the first word is
                    // just the first element
                    if at_open && frame.schema.is_form_head(atom) {
                        frame.head = Some(atom.to_string());
                    } else {
                        frame.count += 1;
                    }
                    at_open = false;
                    top = next_in(frame);
                } else {
                    top = None;
                }
            }
        }
    }
    let position = match (top, at_open, stack.last()) {
        (_, true, Some(frame)) if !frame.schema.forms().is_empty() => {
            Some(Position::Head(frame.schema))
        }
        (Some(schema), _, _) => Some(Position::Value(schema)),
        _ => None,
    };
    return (position, partial);

    fn next_in<'a>(frame: &Frame<'a>) -> Option<&'a Schema> {
        match &frame.head {
            Some(head) => frame.schema.form_arg(head, frame.count),
            None => match frame.schema {
                // A row's elements are items: any of its alternatives.
                Schema::Forms(_) => Some(frame.schema),
                schema => schema.element(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexp_slot::read_value;

    fn schema(text: &str) -> Schema {
        Schema::parse(&read_value(text).expect("schema text")).expect("schema")
    }

    fn checked(schema: &Schema, text: &str) -> Result<String, String> {
        schema
            .check(&read_value(text).expect("value text"))
            .map(|value| format_lisp_source(&value))
    }

    fn row() -> Schema {
        schema(
            "(forms (word left right accent rev stac swap)
                    (form trunc (num :min 1 :max 32 :step 1 :default 3))
                    (form rot (num :min -16 :max 16 :step 1 :default 1))
                    (form every (num :min 1 :max 16 :step 1 :default 4) (word rev swap stac))
                    (form quant (word :16 :16t :8))
                    (form dotdecay (fixed (num :min 0 :max 1 :default 0.85))))",
        )
    }

    #[test]
    fn a_rest_arg_takes_any_number_of_trailing_values() {
        let seq = schema("(form seq (word :hit :cycle) (rest (num :min 0 :max 12 :step 1)))");
        assert_eq!(checked(&seq, "(seq :hit 0 3 40)"), Ok("(\"seq\" \":hit\" 0 3 12)".into()));
        // Missing: one default value; a value may still be a cycle list.
        assert_eq!(checked(&seq, "(seq :cycle)"), Ok("(\"seq\" \":cycle\" 0)".into()));
        assert_eq!(checked(&seq, "(seq :hit (1 2) 5)"), Ok("(\"seq\" \":hit\" (1 2) 5)".into()));
        assert!(checked(&seq, "(seq :bar 1)").is_err());
        assert!(checked(&seq, "(seq :hit x)").is_err());
        assert_eq!(seq.form_rest_start("seq"), Some(1));
        assert!(seq.form_arg("seq", 7).and_then(Schema::num_spec).is_some());
        // Under an `or`, a plain number still works and a seq form reads as one.
        let note = schema("(or (num :min 0 :max 12) (form seq (word :hit) (rest (num :min 0 :max 12))))");
        assert_eq!(checked(&note, "5"), Ok("5".into()));
        assert_eq!(checked(&note, "(3 5)"), Ok("(3 5)".into()));
        assert_eq!(checked(&note, "(seq :hit 3 5)"), Ok("(\"seq\" \":hit\" 3 5)".into()));
    }

    #[test]
    fn numbers_clamp_and_snap_and_cycle_as_lists() {
        let fast = schema("(num :min 1 :max 16 :step 1)");
        assert_eq!(checked(&fast, "40"), Ok("16".into()));
        assert_eq!(checked(&fast, "2.6"), Ok("3".into()));
        assert_eq!(checked(&fast, "(1 2 (3 40))"), Ok("(1 2 (3 16))".into()));
        assert!(checked(&fast, "rev").unwrap_err().contains("expected a number"));
        assert!(checked(&fast, "()").is_err(), "a list is never empty");
    }

    #[test]
    fn words_reject_unknowns_and_keep_keyword_spelling() {
        let words = schema("(word :16 :16t :8)");
        assert_eq!(checked(&words, ":16t"), Ok("\":16t\"".into()));
        assert_eq!(
            checked(&words, ":32"),
            Err("':32' is not one of :16 :16t :8".into())
        );
    }

    #[test]
    fn fixed_slots_refuse_lists() {
        let level = schema("(fixed (num :min 0 :max 1))");
        assert_eq!(checked(&level, "0.5"), Ok("0.5".into()));
        assert_eq!(
            checked(&level, "(0.5 1)"),
            Err("this value cannot cycle; give one value".into())
        );
    }

    #[test]
    fn rows_check_items_as_forms_words_or_word_cycles() {
        let row = row();
        assert_eq!(
            checked(&row, "((trunc (3 1)) right (every 2 (rev swap)) (left left right left))"),
            Ok("((\"trunc\" (3 1)) \"right\" (\"every\" 2 (\"rev\" \"swap\")) (\"left\" \"left\" \"right\" \"left\"))".into())
        );
        // Missing trailing args take their defaults; extra ones are refused.
        assert_eq!(checked(&row, "((trunc))"), Ok("((\"trunc\" 3))".into()));
        assert!(checked(&row, "((trunc 1 2))").unwrap_err().contains("takes 1 argument"));
        assert_eq!(
            checked(&row, "(wobble)"),
            Err("item 1: 'wobble' is not a modifier here".into())
        );
        assert!(checked(&row, "((every 2 (rev bogus)))")
            .unwrap_err()
            .contains("'bogus' is not one of rev swap stac"));
        assert!(checked(&row, "((dotdecay (0.5 0.6)))").unwrap_err().contains("cannot cycle"));
    }

    #[test]
    fn an_or_holds_forms_alone_and_inside_cycles() {
        let every = schema(
            "(form every (num :min 1 :max 16) \
               (or (word rev swap) (form fast (num :min 1 :max 8 :default 2))))",
        );
        // A list headed by a form head is that form, clamped by its rails…
        assert_eq!(checked(&every, "(every 2 (fast 20))"), Ok("(\"every\" 2 (\"fast\" 8))".into()));
        // …and still one member of a per-cycle list.
        assert_eq!(
            checked(&every, "(every 2 (rev (fast 3)))"),
            Ok("(\"every\" 2 (\"rev\" (\"fast\" 3)))".into())
        );
        assert_eq!(every.completions("(every 2 f"), vec!["fast"]);
    }

    #[test]
    fn heads_swap_only_to_the_same_arg_shape() {
        let row = row();
        assert_eq!(row.compatible_heads("trunc"), vec!["trunc", "rot"]);
        assert_eq!(row.compatible_heads("every"), vec!["every"]);
    }

    #[test]
    fn completions_follow_the_cursor_through_forms_and_lists() {
        let row = row();
        // A bare item: words and heads.
        assert_eq!(row.completions("ev"), vec!["every"]);
        assert_eq!(row.completions("r"), vec!["right", "rev", "rot"]);
        // After `(`: heads first.
        assert_eq!(row.completions("(t"), vec!["trunc"]);
        // Inside every: arg 1 is a number (no words), arg 2 is a word slot,
        // also inside a cycle list.
        assert!(row.completions("(every ").is_empty());
        assert_eq!(row.completions("(every 2 s"), vec!["swap", "stac"]);
        assert_eq!(row.completions("(every 2 (rev s"), vec!["swap", "stac"]);
        assert_eq!(row.completions("(quant :16"), vec![":16", ":16t"]);
    }

    #[test]
    fn form_heads_and_their_defaults() {
        let row = row();
        assert!(row.is_form_head("every"));
        assert!(!row.is_form_head("left"));
        assert_eq!(
            row.form_default("every").map(|value| format_lisp_source(&value)),
            Some("(\"every\" 4 \"rev\")".into())
        );
        assert_eq!(row.form_default("left"), None);
    }

    #[test]
    fn dyn_atoms_parse_in_forms_and_accept_any_name() {
        let plock = schema("(form plock (fixed (dyn param)) (num :min -10 :max 10))");
        assert_eq!(
            plock,
            Schema::Form {
                head: "plock".into(),
                args: vec![
                    Schema::Fixed(Box::new(Schema::Dyn("param".into()))),
                    Schema::Num { min: -10.0, max: 10.0, step: 0.0, decimals: 2, default: -10.0 },
                ],
            }
        );
        // Schemas built in Lisp arrive as lists of strings.
        let mut runtime = crate::Runtime::new();
        let from_lisp = runtime
            .eval_str("(list \"form\" \"plock\" (list \"fixed\" (list \"dyn\" \"param\")) (list \"num\" :min -10 :max 10))")
            .unwrap()
            .unwrap();
        assert_eq!(Schema::parse(&from_lisp), Ok(plock.clone()));
        // Validity against the source is shown, never enforced.
        assert_eq!(
            checked(&plock, "(plock instrument:cutoff 3)"),
            Ok("(\"plock\" \"instrument:cutoff\" 3)".into())
        );
        assert_eq!(checked(&plock, "(plock \"not a param\" 3)"), Ok("(\"plock\" \"not a param\" 3)".into()));
        assert!(checked(&plock, "(plock 4 3)").unwrap_err().contains("expected a name"));
        assert!(checked(&plock, "(plock (a b) 3)").unwrap_err().contains("cannot cycle"));
        assert_eq!(format_lisp_source(&plock.default_value()), "(\"plock\" \"\" -10)");
        // Statically it offers no words; the cursor context names the source.
        assert!(plock.completions("(plock ").is_empty());
        let (words, sources, partial) = plock.completion_context("(plock cu");
        assert!(words.is_empty());
        assert_eq!(sources, vec!["param"]);
        assert_eq!(partial, "cu");
        assert!(Schema::parse(&read_value("(dyn)").unwrap()).is_err());
        assert!(Schema::parse(&read_value("(dyn a b)").unwrap()).is_err());
    }

    #[test]
    fn defaults_follow_the_schema() {
        let every = schema("(form every (num :min 1 :max 16 :default 4) (word rev swap))");
        assert_eq!(format_lisp_source(&every.default_value()), "(\"every\" 4 \"rev\")");
        assert!(Schema::parse(&read_value("(num :min 2 :max 1)").unwrap()).is_err());
        assert!(Schema::parse(&read_value("(wat)").unwrap()).is_err());
    }
}
