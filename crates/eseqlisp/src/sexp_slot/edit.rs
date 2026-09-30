//! The slot's editing model (docs/sexp-slot-spec.md §5): the value as a small
//! tree, the internal cursor, the inline text field, the choice popup, and
//! what every key does to them. Pure data in, data out: the widget keeps a
//! `SlotState` per widget id and hands keys here; nothing in this file draws.
//!
//! Vocabulary:
//! - a **stop** is one place the cursor can sit: an item (an atom, a list's
//!   `(`, a form's head) or a list's / row's `+`;
//! - an item is addressed by its **path** of indices from the root value, so
//!   inside a form the args are indices 1.. (index 0 is the head, which is
//!   not a stop of its own: the form's stop *is* its head);
//! - a **row** (`(forms …)` schema) is a root list shown without parens, its
//!   items side by side, then a `+`.

use crossterm::event::{KeyCode, KeyModifiers};

use super::read_value;
use super::schema::{NumSpec, Schema};
use crate::vm::Value;

// ── the value tree ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Num(f64),
    /// A word as typed and stored: `rev`, `:16t` keeps its colon.
    Word(String),
    List(Vec<Datum>),
}

impl Datum {
    pub fn from_value(value: &Value) -> Datum {
        match value {
            Value::Number(number) => Datum::Num(*number),
            Value::String(text) | Value::Symbol(text) => Datum::Word(text.clone()),
            Value::Keyword(name) => Datum::Word(format!(":{}", name.trim_start_matches(':'))),
            Value::List(items) => {
                Datum::List(items.iter().map(|item| Datum::from_value(&item.borrow())).collect())
            }
            Value::Bool(flag) => Datum::Word(flag.to_string()),
            Value::Nil => Datum::Word("nil".to_string()),
            other => Datum::Word(crate::vm::format_lisp_source(other)),
        }
    }

    /// Stored form: numbers, word strings, lists.
    pub fn to_value(&self) -> Value {
        match self {
            Datum::Num(number) => Value::Number(*number),
            Datum::Word(word) => Value::String(word.clone()),
            Datum::List(items) => Value::List(
                items
                    .iter()
                    .map(|item| std::rc::Rc::new(std::cell::RefCell::new(item.to_value())))
                    .collect(),
            ),
        }
    }

    /// The datum as the user would type it: `(every 2 (rev swap))`.
    pub fn text(&self) -> String {
        match self {
            Datum::Num(number) => format_number(*number, None),
            Datum::Word(word) => word.clone(),
            Datum::List(items) => {
                let inner: Vec<String> = items.iter().map(Datum::text).collect();
                format!("({})", inner.join(" "))
            }
        }
    }

    pub fn get(&self, path: &[usize]) -> Option<&Datum> {
        path.iter().try_fold(self, |node, &index| match node {
            Datum::List(items) => items.get(index),
            _ => None,
        })
    }

    fn get_mut(&mut self, path: &[usize]) -> Option<&mut Datum> {
        path.iter().try_fold(self, |node, &index| match node {
            Datum::List(items) => items.get_mut(index),
            _ => None,
        })
    }

    fn items(&self) -> Option<&[Datum]> {
        match self {
            Datum::List(items) => Some(items),
            _ => None,
        }
    }
}

/// `decimals` from the schema when known; otherwise the shortest spelling
/// (`3`, `0.85`).
pub fn format_number(number: f64, decimals: Option<u32>) -> String {
    match decimals {
        Some(decimals) => format!("{:.*}", decimals as usize, number),
        None => format!("{number}"),
    }
}

// ── stops ────────────────────────────────────────────────────────────────────

pub type Path = Vec<usize>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// An atom, a list (its `(`) or a form (its head).
    Item(Path),
    /// The `+` at the end of the list (or row) at this path.
    Plus(Path),
}

/// The slot being edited: its rails and its current value.
#[derive(Clone, Copy)]
pub struct Slot<'a> {
    pub schema: &'a Schema,
    pub value: &'a Datum,
}

impl<'a> Slot<'a> {
    pub fn new(schema: &'a Schema, value: &'a Datum) -> Self {
        Self { schema, value }
    }

    pub fn is_row(&self) -> bool {
        matches!(self.schema, Schema::Forms(_))
    }

    /// The head of the list at `path` if it is a form of the schema that
    /// applies there (a nested `(seq …)` or `(fig 2)` is a form of its own
    /// position's schema, not of the root's). A row's root is never a form.
    pub fn form_head_at(&self, path: &[usize]) -> Option<&'a str> {
        if path.is_empty() && self.is_row() {
            return None;
        }
        let items = self.value.get(path)?.items()?;
        form_head_in(self.schema_at(path)?, items)
    }

    pub fn is_form_at(&self, path: &[usize]) -> bool {
        self.form_head_at(path).is_some()
    }

    /// The stops one level inside the list at `container`, in order.
    pub fn children(&self, container: &[usize]) -> Vec<Stop> {
        let Some(items) = self.value.get(container).and_then(Datum::items) else {
            return Vec::new();
        };
        let child = |index: usize| {
            let mut path = container.to_vec();
            path.push(index);
            Stop::Item(path)
        };
        if let Some(head) = self.form_head_at(container) {
            // A form's args are fixed: no `+`, unless its last arg repeats.
            let args = (1..items.len()).map(child);
            return match self.schema_at(container).and_then(|schema| schema.form_rest_start(head)) {
                Some(_) => args.chain(std::iter::once(Stop::Plus(container.to_vec()))).collect(),
                None => args.collect(),
            };
        }
        (0..items.len())
            .map(child)
            .chain(std::iter::once(Stop::Plus(container.to_vec())))
            .collect()
    }

    /// The outermost stops: a row's items and `+`, or the single root value.
    pub fn top_stops(&self) -> Vec<Stop> {
        if self.is_row() { self.children(&[]) } else { vec![Stop::Item(Vec::new())] }
    }

    fn is_valid(&self, stop: &Stop) -> bool {
        match stop {
            Stop::Item(path) if path.is_empty() => !self.is_row(),
            Stop::Item(path) => self.children(&path[..path.len() - 1]).contains(stop),
            Stop::Plus(path) => self.children(path).contains(stop),
        }
    }

    /// `stop` if it still addresses something in the value, else the nearest
    /// stop that does (the value may have changed under the cursor): the same
    /// index in the deepest list that still exists, else its last item.
    pub fn normalize(&self, stop: &Stop) -> Stop {
        if self.is_valid(stop) {
            return stop.clone();
        }
        let path = match stop {
            Stop::Item(path) | Stop::Plus(path) => path,
        };
        for depth in (0..path.len()).rev() {
            let container = &path[..depth];
            let reachable = container.is_empty() || self.is_valid(&Stop::Item(container.to_vec()));
            if !reachable || self.value.get(container).and_then(Datum::items).is_none() {
                continue;
            }
            let children = self.children(container);
            let same_index = children
                .iter()
                .find(|s| matches!(s, Stop::Item(p) if p.last() == Some(&path[depth])));
            let last_item = || children.iter().rev().find(|s| matches!(s, Stop::Item(_)));
            if let Some(found) = same_index.or_else(last_item).or(children.last()) {
                return found.clone();
            }
        }
        self.top_stops().into_iter().next().unwrap_or(Stop::Item(Vec::new()))
    }

    fn container_of(stop: &Stop) -> Option<&[usize]> {
        match stop {
            Stop::Item(path) if path.is_empty() => None,
            Stop::Item(path) => Some(&path[..path.len() - 1]),
            Stop::Plus(path) => Some(path),
        }
    }

    fn siblings(&self, stop: &Stop) -> Vec<Stop> {
        match Self::container_of(stop) {
            None => vec![stop.clone()],
            Some(container) => self.children(container),
        }
    }

    /// Left / Right: the previous / next sibling, or `None` at either end
    /// (focus leaves the slot).
    pub fn step(&self, stop: &Stop, forward: bool) -> Option<Stop> {
        let siblings = self.siblings(stop);
        let index = siblings.iter().position(|s| s == stop)?;
        if forward {
            siblings.get(index + 1).cloned()
        } else {
            index.checked_sub(1).and_then(|i| siblings.get(i).cloned())
        }
    }

    /// Up: the enclosing list's `(` or form's head; `None` at the top.
    pub fn up(&self, stop: &Stop) -> Option<Stop> {
        let container = Self::container_of(stop)?;
        if container.is_empty() && self.is_row() {
            return None;
        }
        Some(Stop::Item(container.to_vec()))
    }

    /// Down: the first element of the list or form at the cursor.
    pub fn down(&self, stop: &Stop) -> Option<Stop> {
        let Stop::Item(path) = stop else { return None };
        self.value.get(path).and_then(Datum::items)?;
        self.children(path).into_iter().next()
    }

    /// The schema of the value at `path`: each level's arg or element schema
    /// comes from the schema of the list it sits in.
    pub fn schema_at(&self, path: &[usize]) -> Option<&'a Schema> {
        let mut schema = self.schema;
        let mut node = self.value;
        for (depth, &index) in path.iter().enumerate() {
            let items = node.items()?;
            schema = if depth == 0 && self.is_row() {
                // A row's elements are items: any of its alternatives.
                self.schema
            } else if let Some(head) = form_head_in(schema, items) {
                schema.form_arg(head, index.checked_sub(1)?)?
            } else {
                schema.element()?
            };
            node = items.get(index)?;
        }
        Some(schema)
    }

    pub fn num_spec_at(&self, path: &[usize]) -> Option<NumSpec> {
        self.schema_at(path).and_then(Schema::num_spec)
    }

    /// How a number at `path` reads: the schema's decimals when known.
    pub fn number_text(&self, path: &[usize], number: f64) -> String {
        format_number(number, self.num_spec_at(path).map(|spec| spec.decimals))
    }

    /// The text the schema's completer needs before a field at `target`: the
    /// enclosing lists' `(` and the siblings before it, so `(every 2 ` puts
    /// the cursor on every's word argument. A row's own items are not
    /// context (the row is a sequence, each item starts fresh).
    pub fn context_prefix(&self, target: &Stop) -> String {
        let full: Path = match target {
            Stop::Item(path) => path.clone(),
            Stop::Plus(container) => {
                let mut path = container.clone();
                path.push(self.value.get(container).and_then(Datum::items).map_or(0, <[_]>::len));
                path
            }
        };
        let mut out = String::new();
        let mut node = self.value;
        for (depth, &index) in full.iter().enumerate() {
            let Some(items) = node.items() else { break };
            if !(depth == 0 && self.is_row()) {
                out.push('(');
                for item in &items[..index.min(items.len())] {
                    out.push_str(&item.text());
                    out.push(' ');
                }
            }
            match items.get(index) {
                Some(next) => node = next,
                None => break,
            }
        }
        out
    }

    /// Words the schema allows at a field whose text before the caret is
    /// `typed`.
    pub fn completions(&self, target: &Stop, typed: &str) -> Vec<String> {
        self.schema.completions(&format!("{}{typed}", self.context_prefix(target)))
    }

    /// Choices for the word at `path` (its popup).
    pub fn word_choices(&self, path: &[usize]) -> Vec<String> {
        self.completions(&Stop::Item(path.to_vec()), "")
    }

    /// A word picked or typed where an item goes: a form head with args
    /// becomes its default form, anything else stays a word.
    fn expand_word(&self, word: &str) -> Datum {
        match self.schema.form_default(word) {
            Some(value @ Value::List(_)) => Datum::from_value(&value),
            _ => Datum::Word(word.to_string()),
        }
    }

    // ── structural edits: each returns the new root and where the cursor goes

    pub fn replace(&self, path: &[usize], datum: Datum) -> Datum {
        let mut root = self.value.clone();
        if let Some(slot) = root.get_mut(path) {
            *slot = datum;
        }
        root
    }

    pub fn append(&self, container: &[usize], datum: Datum) -> (Datum, Stop) {
        let mut root = self.value.clone();
        let mut path = container.to_vec();
        if let Some(Datum::List(items)) = root.get_mut(container) {
            path.push(items.len());
            items.push(datum);
        }
        (root, Stop::Item(path))
    }

    /// `+` inside `)`: a copy of the last element.
    pub fn append_copy(&self, container: &[usize]) -> Option<(Datum, Stop)> {
        let last = self.value.get(container)?.items()?.last()?.clone();
        Some(self.append(container, last))
    }

    /// Backspace on an item: a row item or list element goes; a list left
    /// with one element collapses to it; a list left empty goes itself.
    /// Form args and the root are fixed.
    pub fn delete(&self, stop: &Stop) -> Option<(Datum, Stop)> {
        let Stop::Item(path) = stop else { return None };
        let (&index, parent) = path.split_last()?;
        let items = self.value.get(parent)?.items()?;
        let row_root = parent.is_empty() && self.is_row();
        if !row_root && let Some(head) = self.form_head_at(parent) {
            // Only a repeating arg past the first goes: (seq :hit 0 3) → (seq :hit 0).
            let start = self.schema_at(parent)?.form_rest_start(head)? + 1;
            if index < start || items.len() <= start + 1 {
                return None;
            }
            let mut root = self.value.clone();
            let Some(Datum::List(list)) = root.get_mut(parent) else { return None };
            list.remove(index);
            let mut next = parent.to_vec();
            next.push(index.min(list.len() - 1));
            return Some((root, Stop::Item(next)));
        }
        if !row_root && items.len() <= 1 {
            return self.delete(&Stop::Item(parent.to_vec()));
        }
        let mut root = self.value.clone();
        if !row_root && items.len() == 2 {
            let keep = items[1 - index].clone();
            *root.get_mut(parent)? = keep;
            return Some((root, Stop::Item(parent.to_vec())));
        }
        let Some(Datum::List(list)) = root.get_mut(parent) else { return None };
        list.remove(index);
        let remaining = list.len();
        let cursor = if remaining == 0 {
            Stop::Plus(parent.to_vec())
        } else {
            let mut next = parent.to_vec();
            next.push(index.min(remaining - 1));
            Stop::Item(next)
        };
        Some((root, cursor))
    }
}

/// The head of `items` if it names one of `schema`'s forms.
fn form_head_in<'d>(schema: &Schema, items: &'d [Datum]) -> Option<&'d str> {
    match items.first() {
        Some(Datum::Word(head)) if schema.is_form_head(head) => Some(head),
        _ => None,
    }
}

// ── view state ──────────────────────────────────────────────────────────────

/// The inline text field. `target` is `Item(path)` to replace that item, or
/// `Plus(container)` to append to that list.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub text: String,
    /// Caret position in chars.
    pub caret: usize,
    pub target: Stop,
    /// Highlighted completion.
    pub highlight: usize,
    /// Why the last Enter was refused (drawn red under the field).
    pub error: Option<String>,
}

impl Field {
    fn new(target: Stop, text: String) -> Self {
        let caret = text.chars().count();
        Self { text, caret, target, highlight: 0, error: None }
    }

    pub fn before_caret(&self) -> String {
        self.text.chars().take(self.caret).collect()
    }

    /// The partial word just before the caret.
    pub fn partial(&self) -> String {
        let before = self.before_caret();
        let start = before
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace() || *ch == '(' || *ch == ')')
            .map_or(0, |(index, ch)| index + ch.len_utf8());
        before[start..].to_string()
    }

    fn byte_at(&self, caret: usize) -> usize {
        self.text.char_indices().nth(caret).map_or(self.text.len(), |(index, _)| index)
    }

    /// Replace the partial word before the caret with `word`.
    fn accept(&mut self, word: &str) {
        let partial_chars = self.partial().chars().count();
        let start = self.byte_at(self.caret - partial_chars);
        let end = self.byte_at(self.caret);
        self.text.replace_range(start..end, word);
        self.caret = self.caret - partial_chars + word.chars().count();
    }
}

/// A choice popup: a word's choices, or a form head's compatible heads.
#[derive(Clone, Debug, PartialEq)]
pub struct Popup {
    pub path: Path,
    pub head: bool,
    pub options: Vec<String>,
    pub highlight: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotState {
    pub cursor: Stop,
    pub field: Option<Field>,
    pub popup: Option<Popup>,
}

impl Default for SlotState {
    fn default() -> Self {
        Self { cursor: Stop::Item(Vec::new()), field: None, popup: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Not ours: let it fall through (spatial focus, global keys).
    Ignored,
    /// Cursor / field / popup changed; no value change.
    Changed,
    /// A committed edit: the new stored value (already checked).
    Commit(Value),
}

/// Most completion rows shown at once under a field (the list scrolls to
/// keep the highlight in view).
pub const MAX_COMPLETIONS: usize = 8;

impl SlotState {
    /// Completions for the open field.
    pub fn field_completions(&self, slot: &Slot<'_>) -> Vec<String> {
        let Some(field) = &self.field else { return Vec::new() };
        slot.completions(&field.target, &field.before_caret())
    }

    /// Put the cursor on `stop` (after normalizing it) and close popups.
    pub fn place(&mut self, slot: &Slot<'_>, stop: Stop) {
        self.cursor = slot.normalize(&stop);
        self.field = None;
        self.popup = None;
    }

    pub fn open_field(&mut self, target: Stop, text: String) {
        self.popup = None;
        self.field = Some(Field::new(target, text));
    }

    /// Open the choices of the word at `path`; false when it has none.
    pub fn open_word_popup(&mut self, slot: &Slot<'_>, path: &[usize]) -> bool {
        let options = slot.word_choices(path);
        let current = match slot.value.get(path) {
            Some(Datum::Word(word)) => Some(word.clone()),
            _ => None,
        };
        self.open_popup(path, false, options, current)
    }

    /// Open the compatible heads of the form at `path`.
    pub fn open_head_popup(&mut self, slot: &Slot<'_>, path: &[usize]) -> bool {
        let Some(head) = slot.form_head_at(path) else {
            return false;
        };
        let options = slot.schema_at(path).map(|schema| schema.compatible_heads(head)).unwrap_or_default();
        self.open_popup(path, true, options, Some(head.to_string()))
    }

    fn open_popup(&mut self, path: &[usize], head: bool, options: Vec<String>, current: Option<String>) -> bool {
        if options.is_empty() {
            return false;
        }
        let highlight = current
            .and_then(|current| options.iter().position(|option| *option == current))
            .unwrap_or(0);
        self.field = None;
        self.popup = Some(Popup { path: path.to_vec(), head, options, highlight });
        self.cursor = Stop::Item(path.to_vec());
        true
    }

    /// Pick popup option `index`.
    pub fn pick(&mut self, slot: &Slot<'_>, index: usize) -> Outcome {
        let Some(popup) = self.popup.take() else { return Outcome::Changed };
        let Some(option) = popup.options.get(index) else { return Outcome::Changed };
        let root = if popup.head {
            let mut head_path = popup.path.clone();
            head_path.push(0);
            slot.replace(&head_path, Datum::Word(option.clone()))
        } else {
            slot.replace(&popup.path, slot.expand_word(option))
        };
        self.cursor = Stop::Item(popup.path);
        match commit(slot, &root) {
            Ok(value) => Outcome::Commit(value),
            Err(_) => Outcome::Changed,
        }
    }

    /// Enter on the cursor (no field or popup open).
    pub fn activate(&mut self, slot: &Slot<'_>) -> Outcome {
        match self.cursor.clone() {
            Stop::Plus(container) if container.is_empty() && slot.is_row() => {
                self.open_field(Stop::Plus(container), String::new());
                Outcome::Changed
            }
            Stop::Plus(container) => match slot.append_copy(&container) {
                Some((root, cursor)) => self.commit_structural(slot, root, cursor),
                None => Outcome::Changed,
            },
            Stop::Item(path) => match slot.value.get(&path) {
                Some(Datum::Word(_)) => {
                    if !self.open_word_popup(slot, &path) {
                        self.open_field(Stop::Item(path.clone()), slot.value.get(&path).map(Datum::text).unwrap_or_default());
                    }
                    Outcome::Changed
                }
                Some(Datum::Num(number)) => {
                    let text = slot.number_text(&path, *number);
                    self.open_field(Stop::Item(path), text);
                    Outcome::Changed
                }
                Some(list @ Datum::List(_)) => {
                    let text = list.text();
                    self.open_field(Stop::Item(path), text);
                    Outcome::Changed
                }
                None => Outcome::Changed,
            },
        }
    }

    fn commit_structural(&mut self, slot: &Slot<'_>, root: Datum, cursor: Stop) -> Outcome {
        match commit(slot, &root) {
            Ok(value) => {
                self.cursor = cursor;
                Outcome::Commit(value)
            }
            Err(_) => Outcome::Changed,
        }
    }

    /// Enter in the field: accept a half-typed completion, read, check,
    /// commit — or keep the field open with the reason.
    fn commit_field(&mut self, slot: &Slot<'_>) -> Outcome {
        let completions = self.field_completions(slot);
        let Some(field) = self.field.as_mut() else { return Outcome::Changed };
        let partial = field.partial();
        if !partial.is_empty()
            && let Some(word) = completions.get(field.highlight)
            && !completions.contains(&partial)
        {
            let word = word.clone();
            field.accept(&word);
        }
        let datum = match read_value(&field.text) {
            Ok(value) => match Datum::from_value(&value) {
                Datum::Word(word) => slot.expand_word(&word),
                datum => datum,
            },
            Err(reason) => {
                field.error = Some(reason);
                return Outcome::Changed;
            }
        };
        let (root, cursor) = match field.target.clone() {
            Stop::Item(path) => (slot.replace(&path, datum), Stop::Item(path)),
            Stop::Plus(container) => slot.append(&container, datum),
        };
        match commit(slot, &root) {
            Ok(value) => {
                self.field = None;
                self.cursor = cursor;
                Outcome::Commit(value)
            }
            Err(reason) => {
                if let Some(field) = self.field.as_mut() {
                    field.error = Some(reason);
                }
                Outcome::Changed
            }
        }
    }

    pub fn handle_key(&mut self, slot: &Slot<'_>, code: KeyCode, modifiers: KeyModifiers) -> Outcome {
        self.cursor = slot.normalize(&self.cursor);
        if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT) {
            return Outcome::Ignored;
        }
        if self.popup.is_some() {
            return self.popup_key(slot, code);
        }
        if self.field.is_some() {
            return self.field_key(slot, code);
        }
        match code {
            KeyCode::Left | KeyCode::Right => match slot.step(&self.cursor, code == KeyCode::Right) {
                Some(stop) => {
                    self.cursor = stop;
                    Outcome::Changed
                }
                None => Outcome::Ignored,
            },
            KeyCode::Up => self.move_to(slot.up(&self.cursor)),
            KeyCode::Down => self.move_to(slot.down(&self.cursor)),
            KeyCode::Enter => self.activate(slot),
            KeyCode::Backspace | KeyCode::Delete => match slot.delete(&self.cursor) {
                Some((root, cursor)) => self.commit_structural(slot, root, cursor),
                // Fixed (a form arg, the root): consumed, nothing happens.
                None => Outcome::Changed,
            },
            KeyCode::Char(ch) if starts_field(ch) => {
                let target = match &self.cursor {
                    Stop::Plus(container) => Stop::Plus(container.clone()),
                    Stop::Item(path) => Stop::Item(path.clone()),
                };
                self.open_field(target, ch.to_string());
                Outcome::Changed
            }
            _ => Outcome::Ignored,
        }
    }

    fn move_to(&mut self, stop: Option<Stop>) -> Outcome {
        match stop {
            Some(stop) => {
                self.cursor = stop;
                Outcome::Changed
            }
            None => Outcome::Ignored,
        }
    }

    fn popup_key(&mut self, slot: &Slot<'_>, code: KeyCode) -> Outcome {
        let Some(popup) = self.popup.as_mut() else { return Outcome::Ignored };
        let count = popup.options.len();
        match code {
            KeyCode::Down => {
                popup.highlight = (popup.highlight + 1).min(count.saturating_sub(1));
                Outcome::Changed
            }
            KeyCode::Up => {
                popup.highlight = popup.highlight.saturating_sub(1);
                Outcome::Changed
            }
            KeyCode::Enter | KeyCode::Tab => {
                let index = popup.highlight;
                self.pick(slot, index)
            }
            KeyCode::Esc => {
                self.popup = None;
                Outcome::Changed
            }
            // Typing over a word (or a head: its whole form) replaces it:
            // the popup gives way to a field.
            KeyCode::Char(ch) if starts_field(ch) => {
                let target = Stop::Item(popup.path.clone());
                self.open_field(target, ch.to_string());
                Outcome::Changed
            }
            _ => Outcome::Changed,
        }
    }

    fn field_key(&mut self, slot: &Slot<'_>, code: KeyCode) -> Outcome {
        let completions = self.field_completions(slot);
        let Some(field) = self.field.as_mut() else { return Outcome::Ignored };
        match code {
            KeyCode::Char(ch) => {
                let at = field.byte_at(field.caret);
                field.text.insert(at, ch);
                field.caret += 1;
                field.highlight = 0;
                field.error = None;
                Outcome::Changed
            }
            KeyCode::Backspace => {
                if field.caret > 0 {
                    let at = field.byte_at(field.caret - 1);
                    field.text.remove(at);
                    field.caret -= 1;
                }
                field.highlight = 0;
                field.error = None;
                Outcome::Changed
            }
            KeyCode::Delete => {
                if field.caret < field.text.chars().count() {
                    let at = field.byte_at(field.caret);
                    field.text.remove(at);
                }
                field.error = None;
                Outcome::Changed
            }
            KeyCode::Left => {
                field.caret = field.caret.saturating_sub(1);
                Outcome::Changed
            }
            KeyCode::Right => {
                field.caret = (field.caret + 1).min(field.text.chars().count());
                Outcome::Changed
            }
            KeyCode::Home => {
                field.caret = 0;
                Outcome::Changed
            }
            KeyCode::End => {
                field.caret = field.text.chars().count();
                Outcome::Changed
            }
            KeyCode::Down => {
                field.highlight = (field.highlight + 1).min(completions.len().saturating_sub(1));
                Outcome::Changed
            }
            KeyCode::Up => {
                field.highlight = field.highlight.saturating_sub(1);
                Outcome::Changed
            }
            KeyCode::Tab => {
                if let Some(word) = completions.get(field.highlight) {
                    let word = word.clone();
                    field.accept(&word);
                    // A bare head with args in an empty item position is the
                    // whole item: insert its default form right away.
                    if field.text == word
                        && let Some(Value::List(_)) = slot.schema.form_default(&word)
                    {
                        return self.commit_field(slot);
                    }
                    field.accept(&format!("{word} "));
                    field.highlight = 0;
                }
                Outcome::Changed
            }
            KeyCode::Enter => self.commit_field(slot),
            KeyCode::Esc => {
                self.field = None;
                Outcome::Changed
            }
            _ => Outcome::Changed,
        }
    }

    /// Whether Escape would close something (so the slot keeps focus).
    pub fn has_open_editor(&self) -> bool {
        self.field.is_some() || self.popup.is_some()
    }
}

/// Keys that open the field on an item: the start of a number, a word, or a
/// list.
fn starts_field(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | ':' | '(' | '_')
}

/// Check a whole new root against the schema: the stored value, or a short
/// reason (a row's "item N: " locator dropped: the field shows where).
pub fn commit(slot: &Slot<'_>, root: &Datum) -> Result<Value, String> {
    slot.schema.check(&root.to_value()).map_err(|reason| {
        match reason.strip_prefix("item ").and_then(|rest| rest.split_once(": ")) {
            Some((index, rest)) if index.chars().all(|ch| ch.is_ascii_digit()) => rest.to_string(),
            _ => reason,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::format_lisp_source;

    fn schema(text: &str) -> Schema {
        Schema::parse(&read_value(text).expect("schema text")).expect("schema")
    }

    fn row_schema() -> Schema {
        schema(
            "(forms (word left right accent rev stac ghost swap)
                    (form trunc (num :min 1 :max 32 :step 1 :decimals 0 :default 3))
                    (form rot (num :min -16 :max 16 :step 1 :decimals 0 :default 1))
                    (form fast (num :min 1 :max 8 :step 1 :decimals 0 :default 2))
                    (form every (num :min 1 :max 16 :step 1 :decimals 0 :default 4)
                                (word left right accent rev stac ghost swap))
                    (form dotdecay (fixed (num :min 0 :max 1 :step 0.01 :decimals 2 :default 0.85))))",
        )
    }

    /// A stored value from typed text (words as strings).
    fn value(text: &str) -> Datum {
        Datum::from_value(&read_value(text).expect("value text"))
    }

    fn item(path: &[usize]) -> Stop {
        Stop::Item(path.to_vec())
    }

    /// Drive `keys` through one state against a value that follows every
    /// commit, as the host would. Returns the final value's text.
    struct Harness {
        schema: Schema,
        value: Datum,
        state: SlotState,
        commits: usize,
    }

    impl Harness {
        fn new(schema: Schema, text: &str) -> Self {
            let value = value(text);
            let mut state = SlotState::default();
            state.cursor = Slot::new(&schema, &value).top_stops()[0].clone();
            Self { schema, value, state, commits: 0 }
        }

        fn key(&mut self, code: KeyCode) -> Outcome {
            let slot = Slot::new(&self.schema, &self.value);
            let outcome = self.state.handle_key(&slot, code, KeyModifiers::NONE);
            if let Outcome::Commit(value) = &outcome {
                self.value = Datum::from_value(value);
                self.commits += 1;
            }
            outcome
        }

        fn keys(&mut self, codes: &[KeyCode]) {
            for code in codes {
                self.key(*code);
            }
        }

        fn typed(&mut self, text: &str) {
            for ch in text.chars() {
                self.key(KeyCode::Char(ch));
            }
        }

        fn text(&self) -> String {
            self.value.text()
        }
    }

    #[test]
    fn row_navigation_walks_siblings_and_nests_with_up_down() {
        let schema = row_schema();
        let value = value("((trunc (3 1)) right (every 2 (rev swap)))");
        let slot = Slot::new(&schema, &value);
        assert_eq!(slot.top_stops(), vec![item(&[0]), item(&[1]), item(&[2]), Stop::Plus(vec![])]);
        // Left/Right stay among siblings; ends fall through.
        assert_eq!(slot.step(&item(&[0]), false), None);
        assert_eq!(slot.step(&item(&[1]), true), Some(item(&[2])));
        assert_eq!(slot.step(&Stop::Plus(vec![]), true), None);
        // Down into a form lands on its first arg, not its head.
        assert_eq!(slot.down(&item(&[0])), Some(item(&[0, 1])));
        // A form arg that is a list: its `(`; down again to its elements.
        assert_eq!(slot.down(&item(&[0, 1])), Some(item(&[0, 1, 0])));
        assert_eq!(slot.children(&[0, 1]), vec![item(&[0, 1, 0]), item(&[0, 1, 1]), Stop::Plus(vec![0, 1])]);
        // Forms have no `+`: args are fixed.
        assert_eq!(slot.children(&[2]), vec![item(&[2, 1]), item(&[2, 2])]);
        // Up climbs to the enclosing `(` / head, and out of the row is focus's.
        assert_eq!(slot.up(&item(&[0, 1, 1])), Some(item(&[0, 1])));
        assert_eq!(slot.up(&Stop::Plus(vec![0, 1])), Some(item(&[0, 1])));
        assert_eq!(slot.up(&item(&[0, 1])), Some(item(&[0])));
        assert_eq!(slot.up(&item(&[0])), None);
        // Atoms have nothing below.
        assert_eq!(slot.down(&item(&[1])), None);
    }

    #[test]
    fn arrows_never_change_values_and_fall_through_at_the_ends() {
        let mut h = Harness::new(row_schema(), "((trunc 3) right)");
        assert_eq!(h.key(KeyCode::Left), Outcome::Ignored);
        assert_eq!(h.key(KeyCode::Up), Outcome::Ignored);
        assert_eq!(h.key(KeyCode::Down), Outcome::Changed);
        assert_eq!(h.state.cursor, item(&[0, 1]));
        assert_eq!(h.key(KeyCode::Right), Outcome::Ignored, "a form's last arg is the end");
        h.keys(&[KeyCode::Up, KeyCode::Right, KeyCode::Right]);
        assert_eq!(h.state.cursor, Stop::Plus(vec![]));
        assert_eq!(h.key(KeyCode::Right), Outcome::Ignored);
        assert_eq!(h.commits, 0);
        assert_eq!(h.text(), "((trunc 3) right)");
    }

    #[test]
    fn typing_a_list_on_a_number_turns_it_into_pickers_in_parens() {
        let num = schema("(num :min 1 :max 16 :step 1 :decimals 0 :default 4)");
        let mut h = Harness::new(num, "4");
        h.typed("(1 2 3 4");
        assert_eq!(h.key(KeyCode::Enter), Outcome::Commit(value("(1 2 3 4)").to_value()));
        assert_eq!(h.text(), "(1 2 3 4)");
        assert_eq!(h.state.field, None);
        // Digits over an element: clamped on Enter.
        h.keys(&[KeyCode::Down, KeyCode::Right]);
        h.typed("40");
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "(1 16 3 4)");
        assert_eq!(h.state.cursor, item(&[1]));
    }

    #[test]
    fn enter_on_a_list_edits_it_as_text_and_escape_restores() {
        let num = schema("(num :min 0 :max 10 :step 1 :decimals 0)");
        let mut h = Harness::new(num, "(1 2 (3 4))");
        h.key(KeyCode::Enter);
        assert_eq!(h.state.field.as_ref().map(|f| f.text.clone()), Some("(1 2 (3 4))".into()));
        h.typed("x");
        assert_eq!(h.key(KeyCode::Esc), Outcome::Changed);
        assert_eq!(h.state.field, None);
        assert_eq!(h.text(), "(1 2 (3 4))");
        // A second Escape is not the slot's: focus leaves.
        assert_eq!(h.key(KeyCode::Esc), Outcome::Ignored);
    }

    #[test]
    fn a_rejected_entry_keeps_the_field_open_with_a_reason() {
        let mut h = Harness::new(row_schema(), "((every 2 rev))");
        h.keys(&[KeyCode::Down, KeyCode::Right]);
        assert_eq!(h.state.cursor, item(&[0, 2]));
        h.typed("(rev bogus");
        assert_eq!(h.key(KeyCode::Enter), Outcome::Changed);
        let field = h.state.field.clone().expect("field stays open");
        assert!(field.error.as_deref().unwrap().contains("'bogus' is not one of"), "{field:?}");
        assert!(!field.error.as_deref().unwrap().starts_with("item "));
        // Fixing the text commits the word cycle.
        h.keys(&[KeyCode::Backspace; 5]);
        h.typed("swap)");
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "((every 2 (rev swap)))");
        // A parse error is refused too.
        h.key(KeyCode::Up);
        h.key(KeyCode::Enter);
        h.key(KeyCode::End);
        h.typed(")");
        h.key(KeyCode::Enter);
        assert_eq!(h.state.field.as_ref().and_then(|f| f.error.clone()), Some("unexpected )".into()));
    }

    #[test]
    fn backspace_deletes_and_one_element_lists_collapse() {
        let mut h = Harness::new(row_schema(), "((fast (1 2)) right left)");
        // Into (1 2), delete 1: the list collapses to 2.
        h.keys(&[KeyCode::Down, KeyCode::Down]);
        assert_eq!(h.state.cursor, item(&[0, 1, 0]));
        h.key(KeyCode::Backspace);
        assert_eq!(h.text(), "((fast 2) right left)");
        assert_eq!(h.state.cursor, item(&[0, 1]));
        // A form arg is fixed.
        assert_eq!(h.key(KeyCode::Backspace), Outcome::Changed);
        assert_eq!(h.commits, 1);
        // A row item goes; the cursor takes the next one.
        h.keys(&[KeyCode::Up, KeyCode::Right]);
        h.key(KeyCode::Backspace);
        assert_eq!(h.text(), "((fast 2) left)");
        assert_eq!(h.state.cursor, item(&[1]));
        h.key(KeyCode::Backspace);
        h.key(KeyCode::Backspace);
        assert_eq!(h.text(), "()");
        assert_eq!(h.state.cursor, Stop::Plus(vec![]));
    }

    #[test]
    fn deleting_a_whole_list_or_form_from_its_head() {
        let mut h = Harness::new(row_schema(), "((trunc 3) (left left right))");
        h.key(KeyCode::Right);
        assert_eq!(h.state.cursor, item(&[1]));
        h.key(KeyCode::Backspace);
        assert_eq!(h.text(), "((trunc 3))");
        h.key(KeyCode::Backspace);
        assert_eq!(h.text(), "()");
    }

    #[test]
    fn a_rest_form_has_a_plus_and_deletes_down_to_one_value() {
        let schema = schema(
            "(forms (word left) (form note (or (num :min 0 :max 12)
                                              (form seq (word :hit) (rest (num :min 0 :max 12))))))",
        );
        let two = value("((note (seq :hit 0 3)))");
        let slot = Slot::new(&schema, &two);
        // Inside (seq …): the clock, the two values, then a `+`.
        let kids = slot.children(&[0, 1]);
        assert_eq!(kids.len(), 4, "{kids:?}");
        assert!(matches!(kids[3], Stop::Plus(_)));
        let (root, _) = slot.append_copy(&[0, 1]).expect("append");
        assert_eq!(crate::vm::format_lisp_source(&commit(&slot, &root).unwrap()), r#"(("note" ("seq" ":hit" 0 3 3)))"#);
        // A value goes; the clock and the last value stay.
        let (root, _) = slot.delete(&item(&[0, 1, 2])).expect("delete 0");
        assert_eq!(crate::vm::format_lisp_source(&commit(&slot, &root).unwrap()), r#"(("note" ("seq" ":hit" 3)))"#);
        let one = value("((note (seq :hit 3)))");
        let slot = Slot::new(&schema, &one);
        assert!(slot.delete(&item(&[0, 1, 2])).is_none());
        assert!(slot.delete(&item(&[0, 1, 1])).is_none());
        // A number under the rest arg scrubs on its rails.
        assert!(slot.num_spec_at(&[0, 1, 2]).is_some());
    }

    #[test]
    fn plus_inside_a_list_appends_a_copy_of_the_last_element() {
        let mut h = Harness::new(row_schema(), "((fast (1 3)))");
        h.keys(&[KeyCode::Down, KeyCode::Down, KeyCode::Right, KeyCode::Right]);
        assert_eq!(h.state.cursor, Stop::Plus(vec![0, 1]));
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "((fast (1 3 3)))");
        assert_eq!(h.state.cursor, item(&[0, 1, 2]));
    }

    #[test]
    fn the_row_plus_is_a_field_with_completions() {
        let mut h = Harness::new(row_schema(), "(right)");
        h.key(KeyCode::Right);
        assert_eq!(h.state.cursor, Stop::Plus(vec![]));
        h.key(KeyCode::Enter);
        let slot_schema = h.schema.clone();
        let slot = Slot::new(&slot_schema, &h.value);
        assert!(h.state.field_completions(&slot).contains(&"every".to_string()));
        h.typed("ev");
        let slot = Slot::new(&slot_schema, &h.value);
        assert_eq!(h.state.field_completions(&slot), vec!["every"]);
        // Tab on a bare head inserts its default form.
        assert!(matches!(h.key(KeyCode::Tab), Outcome::Commit(_)));
        assert_eq!(h.text(), "(right (every 4 left))");
        assert_eq!(h.state.cursor, item(&[1]));
        // Typing a whole form with a word cycle, on the + by typing.
        h.key(KeyCode::Right);
        h.typed("(every 2 (rev swap))");
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "(right (every 4 left) (every 2 (rev swap)))");
    }

    #[test]
    fn enter_accepts_a_half_typed_completion() {
        let mut h = Harness::new(row_schema(), "()");
        h.typed("rig");
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "(right)");
        // Inside a form: the completer follows the cursor to every's word.
        h.key(KeyCode::Right);
        h.typed("(every 2 st");
        let schema = h.schema.clone();
        assert_eq!(h.state.field_completions(&Slot::new(&schema, &h.value)), vec!["stac"]);
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "(right (every 2 stac))");
        // A typed bare head becomes its default form.
        h.key(KeyCode::Right);
        h.typed("trunc");
        h.key(KeyCode::Enter);
        assert_eq!(h.text(), "(right (every 2 stac) (trunc 3))");
    }

    #[test]
    fn completions_see_the_enclosing_form() {
        let schema = row_schema();
        let value = value("((every 2 rev))");
        let slot = Slot::new(&schema, &value);
        assert_eq!(slot.context_prefix(&item(&[0, 2])), "(every 2 ");
        assert_eq!(slot.completions(&item(&[0, 2]), "s"), vec!["stac", "swap"]);
        // A number arg offers no words.
        assert!(slot.completions(&item(&[0, 1]), "").is_empty());
    }

    #[test]
    fn word_popups_pick_and_heads_swap_keeping_args() {
        let mut h = Harness::new(row_schema(), "((trunc 5) (every 2 rev))");
        // Enter on a form head edits the form as text...
        h.key(KeyCode::Enter);
        assert_eq!(h.state.field.as_ref().map(|f| f.text.clone()), Some("(trunc 5)".into()));
        h.key(KeyCode::Esc);
        // ...the head dropdown offers heads of the same shape.
        let schema = h.schema.clone();
        let value = h.value.clone();
        assert!(h.state.open_head_popup(&Slot::new(&schema, &value), &[0]));
        let popup = h.state.popup.clone().unwrap();
        assert_eq!(popup.options, vec!["trunc", "rot", "fast"]);
        assert_eq!(popup.highlight, 0);
        h.keys(&[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(h.text(), "((rot 5) (every 2 rev))");
        // Enter on a word opens its choices; Up/Down move the highlight.
        h.keys(&[KeyCode::Right, KeyCode::Down, KeyCode::Right]);
        assert_eq!(h.state.cursor, item(&[1, 2]));
        h.key(KeyCode::Enter);
        let popup = h.state.popup.clone().expect("word popup");
        assert_eq!(popup.options[popup.highlight], "rev");
        h.keys(&[KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(h.text(), "((rot 5) (every 2 ghost))");
        // Escape closes a popup without change.
        h.key(KeyCode::Enter);
        h.keys(&[KeyCode::Down, KeyCode::Esc]);
        assert_eq!(h.state.popup, None);
        assert_eq!(h.text(), "((rot 5) (every 2 ghost))");
    }

    #[test]
    fn a_row_word_can_become_a_form_from_its_popup() {
        let mut h = Harness::new(row_schema(), "(left)");
        h.key(KeyCode::Enter);
        let popup = h.state.popup.clone().unwrap();
        let fast = popup.options.iter().position(|o| o == "fast").unwrap();
        let schema = h.schema.clone();
        let value = h.value.clone();
        let outcome = h.state.pick(&Slot::new(&schema, &value), fast);
        assert_eq!(outcome, Outcome::Commit(super::super::read_value("((\"fast\" 2))").unwrap()));
    }

    #[test]
    fn fixed_args_refuse_lists() {
        let mut h = Harness::new(row_schema(), "((dotdecay 0.85))");
        h.key(KeyCode::Down);
        h.typed("(0.5 0.6");
        h.key(KeyCode::Enter);
        assert!(h.state.field.as_ref().and_then(|f| f.error.clone()).unwrap().contains("cannot cycle"));
        let schema = h.schema.clone();
        let slot = Slot::new(&schema, &h.value);
        assert_eq!(slot.number_text(&[0, 1], 0.85), "0.85");
    }

    #[test]
    fn cursor_survives_the_value_changing_underneath() {
        let schema = row_schema();
        let value = value("(left)");
        let slot = Slot::new(&schema, &value);
        assert_eq!(slot.normalize(&item(&[3, 1])), item(&[0]));
        assert_eq!(slot.normalize(&item(&[])), item(&[0]));
        let empty = self::value("()");
        assert_eq!(Slot::new(&schema, &empty).normalize(&item(&[0])), Stop::Plus(vec![]));
        let nested = self::value("((fast (1 2)))");
        let slot = Slot::new(&schema, &nested);
        assert_eq!(slot.normalize(&item(&[0, 1, 5])), item(&[0, 1, 1]));
    }

    #[test]
    fn schemas_at_paths_give_number_rails() {
        let schema = row_schema();
        let value = value("((fast (1 2)) (every 3 rev))");
        let slot = Slot::new(&schema, &value);
        let fast = slot.num_spec_at(&[0, 1, 1]).expect("list element of fast's arg");
        assert_eq!((fast.min, fast.max), (1.0, 8.0));
        assert_eq!(slot.num_spec_at(&[1, 1]).map(|s| s.max), Some(16.0));
        assert_eq!(slot.num_spec_at(&[1, 2]), None);
        assert_eq!(format_lisp_source(&value.to_value()), "((\"fast\" (1 2)) (\"every\" 3 \"rev\"))");
    }

    #[test]
    fn a_plain_slot_root_is_one_stop() {
        let words = schema("(word :16 :16t :8)");
        let mut h = Harness::new(words, ":16");
        assert_eq!(h.key(KeyCode::Right), Outcome::Ignored);
        assert_eq!(h.key(KeyCode::Backspace), Outcome::Changed);
        h.key(KeyCode::Enter);
        let popup = h.state.popup.clone().expect("choices");
        assert_eq!(popup.options, vec![":16", ":16t", ":8"]);
        h.keys(&[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(h.text(), ":16t");
    }
}
