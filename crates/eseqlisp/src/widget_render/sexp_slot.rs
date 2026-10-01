//! `sexp-slot`: one focusable widget that edits a guarded piece of Lisp data
//! (docs/sexp-slot-spec.md §5-§6).
//!
//! ```lisp
//! (sexp-slot :key "row-2-mods" :schema row-schema :value (get row :mods)
//!            :on-change (lambda (v) …))
//! ```
//!
//! The value shows as its structure: numbers and words as compact cells,
//! lists in real (dim) paren glyphs on a band that lightens per depth, a `+`
//! just inside every list's `)` and at the end of a row. The slot takes focus
//! as a unit; its internal cursor, the inline text field and the choice /
//! completion popups are this widget's own state, kept per widget. All of the
//! editing rules live in `crate::sexp_slot::edit` (pure, unit-tested); this
//! file lays the value out, draws it, and turns pointer input into the same
//! state changes. Every committed edit is one `:on-change` call with the new
//! stored value.
//!
//! `:lit` — a number, usually a `(bind-seq …)` so it repaints without
//! re-running Lisp — is a bitmask of top-level items to ring in `:lit-color`
//! (bit i = item i): the jaki kind lights the row items applied to the hit
//! that is sounding.
//!
//! `:lit-values` — a list, one number per top-level item (usually
//! `(bind-seq …)`s) — fills the member of item i that number addresses in
//! `:lit-color`: base-64 digits, least significant first, each an element
//! index + 1 below the item (0 = none). The jaki kind shows which value of a
//! `(seq …)` / cycle list the sounding hit played.
//!
//! `:wrap false` keeps the value on one line (it grows past its box instead
//! of wrapping between pieces). `:on-hover` (optional) is called with the top-level item under the pointer
//! (its stored value) whenever that changes, and with `nil` when the pointer
//! leaves the item or the slot. `:tint-args` — a list of `(head index)`
//! pairs — draws argument `index` (0-based) of every `(head …)` form in
//! `:tint-color`, e.g. `'(("on" 0))` marks `on`'s selector apart from its
//! word.
//!
//! `:dyn-context` (any value) is handed untouched to the host sources of the
//! schema's `(dyn SOURCE)` atoms (`crate::sexp_slot::dyn_words`): they are
//! asked only when a popup or field opens on such an atom, or to revalidate
//! when the context or the source's epoch changes, and the answer is cached
//! per widget. A name its source does not offer for the context is drawn in
//! `:error-color` (a `nil` context is not validated); the text is kept.

use std::cell::RefCell;
use std::rc::Rc;
use std::collections::HashMap;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::menu_style::{self, PANEL_PADDING_V, ROW_HEIGHT as MENU_ROW_HEIGHT};
use super::{
    CellBuffer, EventOutput, GpuPrimitive, GpuProportionalTextPrimitive, MouseEventOutcome,
    WidgetDefinition, WidgetEvent, WidgetInstance, WidgetKeyEvent, WidgetViewport, get_f32_prop,
    ndc_bounds, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, TextMeasurer, f64_to_f32,
    get_map, get_prop_num,
};
use crate::sexp_slot::dyn_words::{DynCache, DynLookup};
use crate::sexp_slot::edit::{Completions, Datum, MAX_COMPLETIONS, Outcome, Path, Slot, SlotState, Stop};
use crate::sexp_slot::schema::Schema;
use crate::theme;
use crate::vm::Value;

// ── metrics (layout rows / cells) ────────────────────────────────────────────

/// One line of the slot: the jaki row height.
const DEFAULT_LINE_HEIGHT: f32 = 1.3;
const LINE_GAP: f32 = 0.2;
/// Atom cells are a little shorter than the line.
const ATOM_HEIGHT_RATIO: f32 = 0.78;
const ATOM_PAD: f32 = 0.35;
const GAP: f32 = 0.25;
/// Parens hug what they enclose.
const PAREN_GAP: f32 = 0.06;
const MIN_FIELD_WIDTH: f32 = 3.0;
const ROW_PLUS_MIN_WIDTH: f32 = 1.6;
const MAX_POPUP_ROWS: usize = 10;
const COMMIT_TAG: &str = "sexp-commit";
const HOVER_TAG: &str = "sexp-hover";

// ── state (per widget, like the number-picker's typed edit) ──────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum StateKey {
    Stable { namespace: u64, id: u64 },
    Layout(u64),
}

/// Buffer layouts reserve disjoint 100,000-id ranges (see dropdown.rs).
const WIDGET_ID_NAMESPACE_STRIDE: u64 = 100_000;

/// Where the open popup's rows are, for pointer hit tests (set at render).
#[derive(Clone, Copy, Debug, Default)]
struct OverlayRows {
    /// Rows before the first option (the error line).
    lead: usize,
    /// Index of the first visible option.
    first: usize,
    visible: usize,
}

thread_local! {
    static STATES: RefCell<HashMap<StateKey, SlotState>> = RefCell::new(HashMap::new());
    static KEYS_BY_WIDGET_ID: RefCell<HashMap<u64, StateKey>> = RefCell::new(HashMap::new());
    static OVERLAY_ROWS: RefCell<HashMap<u64, OverlayRows>> = RefCell::new(HashMap::new());
    /// The one open field, by the slot's `:key`, so measuring (which sees
    /// only props) can grow the slot with the field's text.
    static OPEN_FIELD: RefCell<Option<(String, Stop, String)>> = const { RefCell::new(None) };
    /// Slots with an `:on-hover` whose pointer is over a top-level item: the
    /// callback (to report the leave without the node) and the item index.
    static HOVERED: RefCell<HashMap<u64, (Value, usize)>> = RefCell::new(HashMap::new());
    /// Each slot's `dyn` source answers, per (source, context, epoch).
    static DYN_CACHES: RefCell<HashMap<StateKey, std::rc::Rc<RefCell<DynCache>>>> = RefCell::new(HashMap::new());
    /// Measuring sees only props, not the widget: its `dyn-num` rails (for
    /// number widths) come from one cache per printed `:dyn-context`.
    static MEASURE_DYN_CACHES: RefCell<HashMap<String, std::rc::Rc<RefCell<DynCache>>>> = RefCell::new(HashMap::new());
}

/// The slot's `:dyn-context` and its answer cache.
fn dyn_lookup(node: &LayoutNode) -> DynLookup {
    let key = state_key(node);
    let cache = DYN_CACHES.with(|caches| caches.borrow_mut().entry(key).or_default().clone());
    DynLookup::new(node.props.get("dyn-context").cloned().unwrap_or(Value::Nil), cache)
}

/// A lookup for measuring (props only): shared per `:dyn-context`, so it
/// asks a source at most once per (context, epoch). Sources are only asked
/// for `dyn-num` positions with a non-`nil` context.
fn measure_dyn_lookup(props: &HashMap<String, Value>) -> DynLookup {
    let context = props.get("dyn-context").cloned().unwrap_or(Value::Nil);
    let key = crate::vm::format_lisp_source(&context);
    let cache = MEASURE_DYN_CACHES.with(|caches| caches.borrow_mut().entry(key).or_default().clone());
    DynLookup::new(context, cache)
}

/// The pointer now hovers `widget_id` (or nothing): every other slot that
/// still reports a hovered item gets its `:on-hover` called with `nil`.
pub fn pointer_moved_to(widget_id: Option<u64>) -> Vec<EventOutput> {
    HOVERED.with(|hovered| {
        let mut hovered = hovered.borrow_mut();
        let left: Vec<u64> = hovered.keys().copied().filter(|id| Some(*id) != widget_id).collect();
        left.into_iter()
            .filter_map(|id| hovered.remove(&id))
            .map(|(callback, _)| EventOutput { callback, args: vec![Value::Nil] })
            .collect()
    })
}

fn hover_callback(node: &LayoutNode) -> Option<Value> {
    node.props.get("on-hover").filter(|v| !matches!(v, Value::Nil | Value::Bool(false))).cloned()
}

/// Record the hovered top-level item; the event to dispatch when it changed.
fn hover_item(node: &LayoutNode, value: &Datum, item: Option<usize>) -> Option<WidgetEvent> {
    let callback = hover_callback(node)?;
    let previous = HOVERED.with(|hovered| {
        let mut hovered = hovered.borrow_mut();
        match item {
            Some(index) => hovered.insert(node.widget_id, (callback, index)).map(|(_, i)| i),
            None => hovered.remove(&node.widget_id).map(|(_, i)| i),
        }
    });
    if previous == item {
        return None;
    }
    let payload = item.and_then(|index| value.get(&[index])).map_or(Value::Nil, Datum::to_value);
    Some(WidgetEvent::Custom(Value::List(vec![
        std::rc::Rc::new(RefCell::new(Value::Keyword(HOVER_TAG.to_string()))),
        std::rc::Rc::new(RefCell::new(payload)),
    ])))
}

/// `:lit-values`: the slot paths (item index first) of the members to fill.
fn lit_value_paths(props: &HashMap<String, Value>) -> Vec<Path> {
    let Some(Value::List(values)) = props.get("lit-values") else { return Vec::new() };
    values
        .iter()
        .enumerate()
        .filter_map(|(item, cell)| {
            let code = match &*cell.borrow() {
                Value::Number(n) => *n,
                Value::ReactiveRef { slot, .. } => crate::reactive::read_float_slot(slot),
                _ => 0.0,
            };
            let mut code = if code.is_finite() && code > 0.0 { code as u64 } else { 0 };
            if code == 0 {
                return None;
            }
            let mut path = vec![item];
            while code > 0 {
                path.push((code % 64) as usize - 1);
                code /= 64;
            }
            Some(path)
        })
        .collect()
}

/// `:tint-args` rules as (head, arg index) pairs.
fn tint_rules(props: &HashMap<String, Value>) -> Vec<(String, usize)> {
    let Some(Value::List(rules)) = props.get("tint-args") else { return Vec::new() };
    rules
        .iter()
        .filter_map(|rule| match &*rule.borrow() {
            Value::List(parts) => {
                let parts: Vec<Value> = parts.iter().map(|part| part.borrow().clone()).collect();
                match parts.as_slice() {
                    [Value::String(head) | Value::Symbol(head), Value::Number(index)] if *index >= 0.0 => {
                        Some((head.clone(), *index as usize))
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// Whether `path` lies inside a tinted argument: some ancestor-or-self sits
/// at child `index + 1` (the head is child 0) of a `(head …)` form a rule
/// names.
fn tinted(value: &Datum, path: &[usize], rules: &[(String, usize)]) -> bool {
    (1..=path.len()).any(|depth| {
        let (parent, child) = (&path[..depth - 1], path[depth - 1]);
        let Some(Datum::List(items)) = value.get(parent) else { return false };
        let Some(Datum::Word(head)) = items.first() else { return false };
        child >= 1 && rules.iter().any(|(name, index)| name == head && *index + 1 == child)
    })
}

fn state_key(node: &LayoutNode) -> StateKey {
    let key = node
        .stable_widget_id
        .map(|id| StateKey::Stable { namespace: node.widget_id / WIDGET_ID_NAMESPACE_STRIDE, id })
        .unwrap_or(StateKey::Layout(node.widget_id));
    KEYS_BY_WIDGET_ID.with(|keys| keys.borrow_mut().insert(node.widget_id, key));
    key
}

fn key_for_widget_id(widget_id: u64) -> StateKey {
    KEYS_BY_WIDGET_ID
        .with(|keys| keys.borrow().get(&widget_id).copied())
        .unwrap_or(StateKey::Layout(widget_id))
}

fn get_state(node: &LayoutNode) -> SlotState {
    let key = state_key(node);
    STATES.with(|states| states.borrow().get(&key).cloned()).unwrap_or_default()
}

fn set_state(node: &LayoutNode, state: SlotState) {
    let key = state_key(node);
    let changed = STATES.with(|states| {
        let mut states = states.borrow_mut();
        let changed = states.get(&key) != Some(&state);
        states.insert(key, state.clone());
        changed
    });
    // The open field feeds measuring, so a keystroke must re-lay out.
    OPEN_FIELD.with(|open| {
        let mut open = open.borrow_mut();
        let key = slot_key(&node.props);
        match (&state.field, key) {
            (Some(field), Some(key)) => {
                *open = Some((key, field.target.clone(), field.text.clone()));
            }
            (_, key) => {
                if open.as_ref().is_some_and(|(open_key, _, _)| Some(open_key) == key.as_ref()) {
                    *open = None;
                }
            }
        }
    });
    if changed {
        super::bump_widget_state_generation();
    }
}

fn with_state_by_id<R>(widget_id: u64, f: impl FnOnce(&mut SlotState) -> R) -> Option<R> {
    let key = key_for_widget_id(widget_id);
    STATES.with(|states| states.borrow_mut().get_mut(&key).map(f))
}

/// Close the slot's popup and field (the overlay was dismissed from outside).
pub fn close_popups(widget_id: u64) {
    let closed = with_state_by_id(widget_id, |state| {
        let open = state.has_open_editor();
        state.field = None;
        state.popup = None;
        open
    });
    if closed == Some(true) {
        OPEN_FIELD.with(|open| *open.borrow_mut() = None);
        super::bump_widget_state_generation();
    }
}

/// Whether this slot is showing a popup (its overlay entry is live).
pub fn overlay_open(widget_id: u64) -> bool {
    with_state_by_id(widget_id, |state| state.has_open_editor()).unwrap_or(false)
}

/// A focused slot owns bare typing even between edits: a letter, a digit or
/// `(` opens its field, Backspace deletes the item under its cursor.
pub fn captures_text(node: &LayoutNode) -> bool {
    node.widget_type == "sexp-slot"
}

/// The slot is typing into its field right now (Space, Cmd+V and friends
/// are text then, not transport or history).
pub fn editing_text(node: &LayoutNode) -> bool {
    node.widget_type == "sexp-slot" && get_state(node).field.is_some()
}

/// Escape closes the field or popup and keeps focus; only a second Escape
/// leaves the slot.
pub fn escape_keeps_focus(node: &LayoutNode) -> bool {
    node.widget_type == "sexp-slot" && get_state(node).has_open_editor()
}

/// Pointer hover over the open popup moves its highlight.
pub fn hover_overlay(widget_id: u64, local_row: f32) -> bool {
    let Some(rect) = super::overlay_rect_for_widget(widget_id) else { return false };
    let rows = OVERLAY_ROWS.with(|rows| rows.borrow().get(&widget_id).copied()).unwrap_or_default();
    let Some(index) = overlay_option_at(rect, rows, local_row) else { return false };
    let changed = with_state_by_id(widget_id, |state| {
        if let Some(popup) = state.popup.as_mut() {
            let changed = popup.highlight != index;
            popup.highlight = index;
            changed
        } else if let Some(field) = state.field.as_mut() {
            let changed = field.highlight != index;
            field.highlight = index;
            changed
        } else {
            false
        }
    });
    if changed == Some(true) {
        super::bump_widget_state_generation();
        return true;
    }
    false
}

fn overlay_option_at(rect: Rect, rows: OverlayRows, local_row: f32) -> Option<usize> {
    let offset = local_row - rect.row - PANEL_PADDING_V;
    if offset < 0.0 || local_row >= rect.row + rect.height {
        return None;
    }
    let row = (offset / MENU_ROW_HEIGHT).floor() as usize;
    let option = row.checked_sub(rows.lead)?;
    (option < rows.visible).then_some(rows.first + option)
}

// ── props ───────────────────────────────────────────────────────────────────

fn props_from_node(node: &Value) -> HashMap<String, Value> {
    get_map(node)
        .map(|map| map.into_iter().filter(|(key, _)| key != "type" && key != "children").collect())
        .unwrap_or_default()
}

fn slot_key(props: &HashMap<String, Value>) -> Option<String> {
    match props.get("key") {
        Some(Value::String(key)) => Some(key.clone()),
        Some(Value::Keyword(key)) => Some(key.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// The slot's parsed schema. A schema is plain data a host passes on every
/// render (the jaki rule body's is ~130 KB of nested lists), and measure,
/// paint, mouse and key handling each need it, so parsing it from scratch
/// every call made a panel of slots cost (slots × schema size) several times
/// per frame. Parsed schemas are cached by a hash of the value's content:
/// walking it allocates nothing and is far cheaper than parsing, and a
/// content key stays right whether the host shares or copies the value.
fn parse_schema(props: &HashMap<String, Value>) -> Result<Rc<Schema>, String> {
    let value = props.get("schema").ok_or("sexp-slot needs :schema")?;
    // Fast path: a list clone shares its element cells, so the same Lisp
    // schema value passed again has the same element pointers. The entry
    // keeps a clone of the value, so those cells (and their addresses) stay
    // alive and cannot be reused by another value.
    let identity = schema_identity(value);
    if let Some(id) = identity {
        if let Some(schema) = SCHEMA_IDENTITY.with(|cache| cache.borrow().get(&id).map(|(_, s)| Rc::clone(s))) {
            return Ok(schema);
        }
    }
    let schema = parse_schema_by_content(value)?;
    if let Some(id) = identity {
        SCHEMA_IDENTITY.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= 64 {
                cache.clear();
            }
            cache.insert(id, (value.clone(), Rc::clone(&schema)));
        });
    }
    Ok(schema)
}

/// (length, first cell, last cell) of a list value, or None.
fn schema_identity(value: &Value) -> Option<(usize, usize, usize)> {
    match value {
        Value::List(items) if !items.is_empty() => Some((
            items.len(),
            Rc::as_ptr(&items[0]) as usize,
            Rc::as_ptr(&items[items.len() - 1]) as usize,
        )),
        _ => None,
    }
}

fn parse_schema_by_content(value: &Value) -> Result<Rc<Schema>, String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_schema_value(value, &mut hasher);
    let key = std::hash::Hasher::finish(&hasher);
    SCHEMA_CACHE.with(|cache| {
        if let Some(schema) = cache.borrow().get(&key) {
            return Ok(Rc::clone(schema));
        }
        let schema = Rc::new(Schema::parse(value)?);
        let mut cache = cache.borrow_mut();
        // a handful of distinct schemas per session; bound it anyway
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, Rc::clone(&schema));
        Ok(schema)
    })
}

thread_local! {
    static SCHEMA_CACHE: std::cell::RefCell<HashMap<u64, Rc<Schema>>> =
        std::cell::RefCell::new(HashMap::new());
    static SCHEMA_IDENTITY: std::cell::RefCell<HashMap<(usize, usize, usize), (Value, Rc<Schema>)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Hash the parts of a value a schema can contain (lists, maps, atoms).
fn hash_schema_value(value: &Value, hasher: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    match value {
        Value::List(items) => {
            0u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_schema_value(&item.borrow(), hasher);
            }
        }
        Value::Map(map) => {
            1u8.hash(hasher);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                key.hash(hasher);
                hash_schema_value(&map[key].borrow(), hasher);
            }
        }
        Value::String(s) => { 2u8.hash(hasher); s.hash(hasher); }
        Value::Symbol(s) => { 3u8.hash(hasher); s.hash(hasher); }
        Value::Keyword(s) => { 4u8.hash(hasher); s.hash(hasher); }
        Value::Number(n) => { 5u8.hash(hasher); n.to_bits().hash(hasher); }
        Value::Bool(b) => { 6u8.hash(hasher); b.hash(hasher); }
        _ => 7u8.hash(hasher),
    }
}

fn value_datum(props: &HashMap<String, Value>, schema: &Schema) -> Datum {
    match props.get("value") {
        Some(Value::Nil) | None => Datum::from_value(&schema.default_value()),
        Some(value) => Datum::from_value(value),
    }
}

/// `:wrap false`: one line, never wrapped.
fn wraps(props: &HashMap<String, Value>) -> bool {
    !matches!(props.get("wrap"), Some(Value::Bool(false)))
}

fn line_height(props: &HashMap<String, Value>) -> f32 {
    get_f32_prop(props, "height", DEFAULT_LINE_HEIGHT).max(0.5)
}

// ── layout ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Piece {
    Num { path: Path, text: String },
    Word { path: Path, text: String },
    /// A form's head: the form's stop.
    Head { path: Path, text: String },
    Open { path: Path },
    Close { path: Path },
    Plus { container: Path, row: bool },
    Field { text: String },
}

#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub piece: Piece,
    /// Band depth of the list the piece sits in (0 = top level).
    pub depth: usize,
    pub line: usize,
    pub col: f32,
    pub width: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct Band {
    pub path: Path,
    pub depth: usize,
    pub line: usize,
    pub col: f32,
    pub width: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SlotLayout {
    pub placed: Vec<Placed>,
    pub bands: Vec<Band>,
    pub lines: usize,
    pub width: f32,
}

/// The value as a flat run of pieces, depth-first, with the open field (if
/// any) standing in for its target.
fn pieces(slot: &Slot<'_>, field: Option<(&Stop, &str)>) -> Vec<(Piece, usize)> {
    fn item(slot: &Slot<'_>, field: Option<(&Stop, &str)>, path: Path, depth: usize, out: &mut Vec<(Piece, usize)>) {
        if let Some((Stop::Item(target), text)) = field
            && *target == path
        {
            out.push((Piece::Field { text: text.to_string() }, depth));
            return;
        }
        match slot.value.get(&path) {
            Some(Datum::Num(number)) => {
                let text = slot.number_text(&path, *number);
                out.push((Piece::Num { path, text }, depth));
            }
            Some(Datum::Word(word)) => out.push((Piece::Word { path, text: word.clone() }, depth)),
            Some(Datum::List(_)) => {
                let inner = depth + 1;
                out.push((Piece::Open { path: path.clone() }, inner));
                if let Some(head) = slot.form_head_at(&path) {
                    out.push((Piece::Head { path: path.clone(), text: head.to_string() }, inner));
                }
                for stop in slot.children(&path) {
                    match stop {
                        Stop::Item(child) => item(slot, field, child, inner, out),
                        Stop::Plus(container) => plus(field, container, false, inner, out),
                    }
                }
                out.push((Piece::Close { path }, inner));
            }
            None => {}
        }
    }
    fn plus(field: Option<(&Stop, &str)>, container: Path, row: bool, depth: usize, out: &mut Vec<(Piece, usize)>) {
        match field {
            Some((Stop::Plus(target), text)) if *target == container => {
                out.push((Piece::Field { text: text.to_string() }, depth));
            }
            _ => out.push((Piece::Plus { container, row }, depth)),
        }
    }
    let mut out = Vec::new();
    for stop in slot.top_stops() {
        match stop {
            Stop::Item(path) => item(slot, field, path, 0, &mut out),
            Stop::Plus(container) => plus(field, container, true, 0, &mut out),
        }
    }
    out
}

fn piece_width(piece: &Piece, text_width: &dyn Fn(&str) -> f32) -> f32 {
    match piece {
        Piece::Num { text, .. } | Piece::Word { text, .. } | Piece::Head { text, .. } => {
            text_width(text) + ATOM_PAD * 2.0
        }
        Piece::Open { .. } => text_width("("),
        Piece::Close { .. } => text_width(")"),
        Piece::Plus { row: true, .. } => (text_width("+") + 0.8).max(ROW_PLUS_MIN_WIDTH),
        Piece::Plus { row: false, .. } => text_width("+") + 0.3,
        Piece::Field { text } => (text_width(text) + 0.9).max(MIN_FIELD_WIDTH),
    }
}

/// Flow the pieces left to right, wrapping between pieces at `wrap_width`.
pub(crate) fn layout_slot(
    slot: &Slot<'_>,
    field: Option<(&Stop, &str)>,
    text_width: &dyn Fn(&str) -> f32,
    wrap_width: f32,
) -> SlotLayout {
    let mut placed: Vec<Placed> = Vec::new();
    let (mut line, mut col) = (0usize, 0.0f32);
    let mut previous: Option<&Piece> = None;
    let run = pieces(slot, field);
    for (piece, depth) in &run {
        let width = piece_width(piece, text_width);
        if let Some(prev) = previous {
            let gap = if matches!(prev, Piece::Open { .. }) || matches!(piece, Piece::Close { .. }) {
                PAREN_GAP
            } else {
                GAP
            };
            col += gap;
            if col + width > wrap_width && !matches!(piece, Piece::Close { .. }) {
                line += 1;
                col = 0.0;
            }
        }
        placed.push(Placed { piece: piece.clone(), depth: *depth, line, col, width });
        col += width;
        previous = Some(piece);
    }
    // One band per list per line, from its `(` to its `)`.
    let mut bands = Vec::new();
    for (open_index, open) in placed.iter().enumerate() {
        let Piece::Open { path } = &open.piece else { continue };
        let close_index = placed[open_index..]
            .iter()
            .position(|p| matches!(&p.piece, Piece::Close { path: close } if close == path))
            .map_or(placed.len() - 1, |offset| open_index + offset);
        let span = &placed[open_index..=close_index];
        for line in open.line..=span.last().map_or(open.line, |p| p.line) {
            let on_line: Vec<&Placed> = span.iter().filter(|p| p.line == line).collect();
            let (Some(first), Some(last)) = (on_line.first(), on_line.last()) else { continue };
            bands.push(Band {
                path: path.clone(),
                depth: open.depth,
                line,
                col: first.col - PAREN_GAP,
                width: last.col + last.width - first.col + PAREN_GAP * 2.0,
            });
        }
    }
    let width = placed.iter().map(|p| p.col + p.width).fold(0.0, f32::max);
    SlotLayout { lines: placed.last().map_or(1, |p| p.line + 1), placed, bands, width }
}

impl SlotLayout {
    pub fn height(&self, line_h: f32) -> f32 {
        self.lines as f32 * line_h + self.lines.saturating_sub(1) as f32 * LINE_GAP
    }

    fn line_row(line: usize, line_h: f32) -> f32 {
        line as f32 * (line_h + LINE_GAP)
    }

    /// The piece's rect relative to the slot's top-left.
    fn piece_rect(placed: &Placed, line_h: f32) -> Rect {
        let atom = matches!(
            placed.piece,
            Piece::Num { .. } | Piece::Word { .. } | Piece::Head { .. } | Piece::Plus { .. } | Piece::Field { .. }
        );
        let height = if atom { line_h * ATOM_HEIGHT_RATIO } else { line_h };
        Rect {
            row: Self::line_row(placed.line, line_h) + (line_h - height) * 0.5,
            col: placed.col,
            width: placed.width,
            height,
        }
    }

    /// The stop under a point relative to the slot's top-left.
    fn stop_at(&self, col: f32, row: f32, line_h: f32) -> Option<(Stop, &Placed)> {
        let line = (row / (line_h + LINE_GAP)).floor().max(0.0) as usize;
        self.placed
            .iter()
            .filter(|p| p.line == line)
            .find(|p| col >= p.col - GAP * 0.5 && col < p.col + p.width + GAP * 0.5)
            .and_then(|p| {
                let stop = match &p.piece {
                    Piece::Num { path, .. }
                    | Piece::Word { path, .. }
                    | Piece::Head { path, .. }
                    | Piece::Open { path }
                    | Piece::Close { path } => Stop::Item(path.clone()),
                    Piece::Plus { container, .. } => Stop::Plus(container.clone()),
                    Piece::Field { .. } => return None,
                };
                Some((stop, p))
            })
    }

    /// Rects the cursor ring surrounds: a list's band, a form's head, an
    /// atom, a `+`.
    fn cursor_rects(&self, slot: &Slot<'_>, cursor: &Stop, line_h: f32) -> Vec<Rect> {
        let plain_list = |path: &Path| {
            slot.value.get(path).is_some_and(|datum| matches!(datum, Datum::List(_))) && !slot.is_form_at(path)
        };
        match cursor {
            Stop::Item(path) if plain_list(path) => self
                .bands
                .iter()
                .filter(|band| band.path == *path)
                .map(|band| Rect {
                    row: Self::line_row(band.line, line_h),
                    col: band.col,
                    width: band.width,
                    height: line_h,
                })
                .collect(),
            _ => self
                .placed
                .iter()
                .filter(|p| match (&p.piece, cursor) {
                    (Piece::Head { path, .. }, Stop::Item(target))
                    | (Piece::Num { path, .. }, Stop::Item(target))
                    | (Piece::Word { path, .. }, Stop::Item(target)) => path == target,
                    (Piece::Plus { container, .. }, Stop::Plus(target)) => container == target,
                    _ => false,
                })
                .map(|p| Self::piece_rect(p, line_h))
                .collect(),
        }
    }

    /// One rect per line spanned by top-level item `index`, from its first
    /// piece to its last.
    fn item_rects(&self, index: usize, line_h: f32) -> Vec<Rect> {
        let pieces: Vec<&Placed> = self
            .placed
            .iter()
            .filter(|p| match &p.piece {
                Piece::Num { path, .. }
                | Piece::Word { path, .. }
                | Piece::Head { path, .. }
                | Piece::Open { path }
                | Piece::Close { path } => path.first() == Some(&index),
                Piece::Plus { container, .. } => container.first() == Some(&index),
                Piece::Field { .. } => false,
            })
            .collect();
        let mut lines: Vec<usize> = pieces.iter().map(|p| p.line).collect();
        lines.dedup();
        lines
            .into_iter()
            .filter_map(|line| {
                let on_line: Vec<&&Placed> = pieces.iter().filter(|p| p.line == line).collect();
                let (first, last) = (on_line.first()?, on_line.last()?);
                Some(Rect {
                    row: Self::line_row(line, line_h),
                    col: first.col - PAREN_GAP,
                    width: last.col + last.width - first.col + PAREN_GAP * 2.0,
                    height: line_h,
                })
            })
            .collect()
    }

    /// Where a popup anchors: under the cursor's item or the field.
    fn anchor(&self, stop: Option<&Stop>, line_h: f32) -> Option<Rect> {
        let placed = self.placed.iter().find(|p| match (&p.piece, stop) {
            (Piece::Field { .. }, None) => true,
            (Piece::Head { path, .. } | Piece::Word { path, .. } | Piece::Num { path, .. }, Some(Stop::Item(target))) => {
                path == target
            }
            _ => false,
        })?;
        Some(Self::piece_rect(placed, line_h))
    }
}

fn measure_text(text: &str, font_size: f32, cell_w: f32, measurer: Option<&dyn TextMeasurer>) -> f32 {
    match measurer {
        Some(measurer) if cell_w > 0.0 => measurer.measure_text_px(text, font_size) / cell_w,
        _ => text.chars().count() as f32 * menu_style::APPROX_CHAR_WIDTH * (font_size / DEFAULT_FONT_SIZE),
    }
}

fn render_text_width(text: &str, font_size: f32, cell_w: f32) -> f32 {
    super::with_render_text_measurer(|measurer| measure_text(text, font_size, cell_w, Some(measurer)))
        .unwrap_or_else(|| measure_text(text, font_size, cell_w, None))
}

/// The open field for this slot, from widget state (render / events) or the
/// measuring side table (measure).
fn open_field_for_key(props: &HashMap<String, Value>) -> Option<(Stop, String)> {
    let key = slot_key(props)?;
    OPEN_FIELD.with(|open| {
        open.borrow()
            .as_ref()
            .filter(|(open_key, _, _)| *open_key == key)
            .map(|(_, target, text)| (target.clone(), text.clone()))
    })
}

/// Lay the node out at its render width.
fn node_layout(node: &LayoutNode, schema: &Schema, value: &Datum, state: &SlotState, cell_w: f32) -> SlotLayout {
    let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
    let lookup = dyn_lookup(node);
    let slot = Slot::new(schema, value).with_dyn(&lookup);
    let field = state.field.as_ref().map(|field| (&field.target, field.text.as_str()));
    let text_width = |text: &str| render_text_width(text, font_size, cell_w);
    // A hair of slack so the render measurer never wraps what measuring fit.
    let wrap = if wraps(&node.props) { node.rect.width + 0.05 } else { f32::INFINITY };
    layout_slot(&slot, field, &text_width, wrap)
}

// ── drawing helpers ─────────────────────────────────────────────────────────

fn rounded_rect(rect: Rect, color: Color, radius_px: f32, viewport: WidgetViewport, background: bool) -> GpuPrimitive {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    GpuPrimitive::WidgetInstance {
        widget_type: "dropdown".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: [color.r, color.g, color.b, color.a],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: ((super::ui_design_px(radius_px) * 2.0) / px_h).clamp(0.001, 0.5),
            pixel_aspect: px_w / px_h,
        },
        is_background: background,
    }
}

fn text(row: f32, col: f32, text: String, font_size: f32, fg: Color) -> GpuPrimitive {
    GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
        row,
        col,
        align_width: 0.0,
        h_align: 0.0,
        text,
        font_size,
        scale: 1.0,
        fg,
        bg: Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
        mono: false,
    })
}

fn offset(rect: Rect, origin: Rect) -> Rect {
    Rect { row: rect.row + origin.row, col: rect.col + origin.col, ..rect }
}

fn grow(rect: Rect, rows: f32, viewport: WidgetViewport) -> Rect {
    let cols = rows * viewport.cell_h / viewport.cell_w.max(1.0);
    Rect {
        row: rect.row - rows,
        col: rect.col - cols,
        width: rect.width + cols * 2.0,
        height: rect.height + rows * 2.0,
    }
}

fn focus_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "focus-color", Color { r: 1.0, g: 0.95, b: 0.25, a: 1.0 })
}

// ── the widget ──────────────────────────────────────────────────────────────

pub struct SexpSlotWidget;
pub static SEXP_SLOT_WIDGET: SexpSlotWidget = SexpSlotWidget;

fn commit_event(value: Value) -> WidgetEvent {
    WidgetEvent::Custom(Value::List(vec![
        std::rc::Rc::new(RefCell::new(Value::Keyword(COMMIT_TAG.to_string()))),
        std::rc::Rc::new(RefCell::new(value)),
    ]))
}

fn outcome_event(node: &LayoutNode, state: SlotState, outcome: Outcome) -> Option<WidgetEvent> {
    match outcome {
        Outcome::Ignored => None,
        Outcome::Changed => {
            set_state(node, state);
            Some(WidgetEvent::Custom(Value::Nil))
        }
        Outcome::Commit(value) => {
            set_state(node, state);
            Some(commit_event(value))
        }
    }
}

impl WidgetDefinition for SexpSlotWidget {
    fn names(&self) -> &'static [&'static str] {
        &["sexp-slot"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["value", "schema", "width", "max-width", "height", "font-size", "wrap"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "schema", "value", "on-change", "width", "max-width", "height", "font-size",
            "focusable", "atom-bg", "text-color", "head-color", "paren-color", "band-color",
            "focus-color", "error-color", "on-hover", "tint-args", "tint-color", "wrap",
            "lit", "lit-color", "lit-values", "dyn-context",
        ]
    }

    /// `:lit` is usually a `(bind-seq …)`: a hit repaints the slot without
    /// re-running Lisp.
    fn bindable_props(&self) -> &'static [&'static str] {
        &["lit"]
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let props = props_from_node(node);
        let line_h = get_prop_num(node, "height").map(f64_to_f32).unwrap_or(DEFAULT_LINE_HEIGHT);
        let min_width = get_prop_num(node, "width").map(f64_to_f32).unwrap_or(0.0);
        let Ok(schema) = parse_schema(&props) else {
            return Some(Size { width: min_width.max(12.0), height: line_h });
        };
        let value = value_datum(&props, &schema);
        let font_size = get_prop_num(node, "font-size").map(f64_to_f32).unwrap_or(ctx.inherited_font_size);
        let text_width = |text: &str| measure_text(text, font_size, ctx.cell_w, ctx.text_measurer);
        let wrap = if !wraps(&props) {
            f32::INFINITY
        } else {
            get_prop_num(node, "max-width")
            .map(f64_to_f32)
            .unwrap_or(f32::INFINITY)
            .min(if constraints.max_width > 0.0 { constraints.max_width } else { f32::INFINITY })
        };
        let field = open_field_for_key(&props);
        let lookup = measure_dyn_lookup(&props);
        let layout = layout_slot(
            &Slot::new(&schema, &value).with_dyn(&lookup),
            field.as_ref().map(|(target, text)| (target, text.as_str())),
            &text_width,
            wrap,
        );
        Some(Size { width: layout.width.max(min_width), height: layout.height(line_h) })
    }

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32, _modifiers: KeyModifiers) -> Option<Value> {
        let state = get_state(node);
        if state.has_open_editor() {
            return None;
        }
        let schema = parse_schema(&node.props).ok()?;
        let value = value_datum(&node.props, &schema);
        let layout = node_layout(node, &schema, &value, &state, last_cell_w());
        let (stop, _) = layout.stop_at(local_col - node.rect.col, local_row - node.rect.row, line_height(&node.props))?;
        let Stop::Item(path) = stop else { return None };
        let Some(Datum::Num(start)) = value.get(&path) else { return None };
        let cell = |v: Value| std::rc::Rc::new(RefCell::new(v));
        let mut gesture = vec![cell(Value::Number(*start)), cell(Value::Number(local_row as f64))];
        gesture.extend(path.iter().map(|&index| cell(Value::Number(index as f64))));
        Some(Value::List(gesture))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        remember_cell_w(cell_w);
        let Ok(schema) = parse_schema(&node.props) else { return MouseEventOutcome::Consume };
        let value = value_datum(&node.props, &schema);
        let lookup = dyn_lookup(node);
        let slot = Slot::new(&schema, &value).with_dyn(&lookup);
        let mut state = get_state(node);
        state.cursor = slot.normalize(&state.cursor);

        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // An open popup or field is an overlay: every click lands here.
                if state.has_open_editor() {
                    let rows = OVERLAY_ROWS.with(|rows| rows.borrow().get(&node.widget_id).copied()).unwrap_or_default();
                    let hit = super::overlay_rect_for_widget(node.widget_id)
                        .and_then(|rect| overlay_option_at(rect, rows, local_row));
                    let outcome = match hit {
                        Some(index) if state.popup.is_some() => state.pick(&slot, index),
                        Some(index) => {
                            if let Some(field) = state.field.as_mut() {
                                field.highlight = index;
                            }
                            state.handle_key(&slot, crossterm::event::KeyCode::Tab, KeyModifiers::NONE)
                        }
                        None => {
                            state.field = None;
                            state.popup = None;
                            Outcome::Changed
                        }
                    };
                    super::remove_overlay(node.widget_id);
                    return match outcome_event(node, state, outcome) {
                        Some(event) => MouseEventOutcome::Dispatch(event),
                        None => MouseEventOutcome::Consume,
                    };
                }
                let layout = node_layout(node, &schema, &value, &state, cell_w);
                let line_h = line_height(&node.props);
                let Some((stop, placed)) = layout.stop_at(local_col - node.rect.col, local_row - node.rect.row, line_h) else {
                    return MouseEventOutcome::Consume;
                };
                state.place(&slot, stop.clone());
                let outcome = match (&placed.piece, &stop) {
                    (Piece::Word { text, .. }, Stop::Item(path)) => {
                        // A dyn name with nothing to pick (only a hint) opens
                        // as text, so the hint shows and the name stays editable.
                        if !state.open_word_popup(&slot, path) && slot.dyn_field(&stop, text) {
                            state.open_field(stop.clone(), text.clone());
                        }
                        Outcome::Changed
                    }
                    (Piece::Head { .. }, Stop::Item(path)) => {
                        state.open_head_popup(&slot, path);
                        Outcome::Changed
                    }
                    (Piece::Plus { .. }, _) => state.activate(&slot),
                    _ => Outcome::Changed,
                };
                match outcome_event(node, state, outcome) {
                    Some(event) => MouseEventOutcome::Dispatch(event),
                    None => MouseEventOutcome::Consume,
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(Value::List(gesture)) = gesture else { return MouseEventOutcome::Consume };
                let numbers: Vec<f64> = gesture
                    .iter()
                    .filter_map(|cell| match &*cell.borrow() {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .collect();
                let [start, start_row, path @ ..] = numbers.as_slice() else {
                    return MouseEventOutcome::Consume;
                };
                let path: Path = path.iter().map(|&index| index as usize).collect();
                let Some(spec) = slot.num_spec_at(&path) else { return MouseEventOutcome::Consume };
                let per_row = scrub_per_row(spec.min, spec.max, spec.step);
                let scrubbed = spec.snap(start + (start_row - local_row as f64) * per_row);
                if value.get(&path) == Some(&Datum::Num(scrubbed)) {
                    return MouseEventOutcome::Consume;
                }
                let root = slot.replace(&path, Datum::Num(scrubbed));
                match crate::sexp_slot::edit::commit(&slot, &root) {
                    Ok(stored) => {
                        state.place(&slot, Stop::Item(path));
                        set_state(node, state);
                        MouseEventOutcome::Dispatch(commit_event(stored))
                    }
                    Err(_) => MouseEventOutcome::Consume,
                }
            }
            MouseEventKind::Moved if hover_callback(node).is_some() => {
                let layout = node_layout(node, &schema, &value, &state, cell_w);
                let item = layout
                    .stop_at(local_col - node.rect.col, local_row - node.rect.row, line_height(&node.props))
                    .and_then(|(stop, _)| match stop {
                        Stop::Item(path) => path.first().copied(),
                        Stop::Plus(_) => None,
                    });
                match hover_item(node, &value, item) {
                    Some(event) => MouseEventOutcome::Dispatch(event),
                    None => MouseEventOutcome::Consume,
                }
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let schema = parse_schema(&node.props).ok()?;
        let value = value_datum(&node.props, &schema);
        let lookup = dyn_lookup(node);
        let slot = Slot::new(&schema, &value).with_dyn(&lookup);
        let mut state = get_state(node);
        let outcome = state.handle_key(&slot, key.code, key.modifiers);
        if !state.has_open_editor() {
            super::remove_overlay(node.widget_id);
        }
        outcome_event(node, state, outcome)
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(Value::List(parts)) = event else { return None };
        let [tag, value] = parts.as_slice() else { return None };
        if matches!(&*tag.borrow(), Value::Keyword(tag) if tag == HOVER_TAG) {
            let callback = hover_callback(node)?;
            return Some(EventOutput { callback, args: vec![value.borrow().clone()] });
        }
        if !matches!(&*tag.borrow(), Value::Keyword(tag) if tag == COMMIT_TAG) {
            return None;
        }
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput { callback, args: vec![value.borrow().clone()] })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let text = match parse_schema(props) {
            Ok(schema) => {
                let value = value_datum(props, &schema);
                match (&*schema, &value) {
                    (Schema::Forms(_), Datum::List(items)) => {
                        items.iter().map(Datum::text).collect::<Vec<_>>().join(" ") + " +"
                    }
                    _ => value.text(),
                }
            }
            Err(reason) => reason,
        };
        let fg = resolve_named_color(props, "text-color", theme::FG());
        let row = rect.row.round() as u16;
        let start = rect.col.round() as u16;
        let end = start + rect.width.round() as u16;
        for (i, ch) in text.chars().enumerate() {
            let col = start + i as u16;
            if col >= end {
                break;
            }
            buf.set(row, col, styled_cell(ch, fg, None));
        }
    }

    fn build_primitives(&self, _widget_type: &str, node: &LayoutNode, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
        remember_cell_w(viewport.cell_w);
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let line_h = line_height(&node.props);
        let mut prims = Vec::new();
        let schema = match parse_schema(&node.props) {
            Ok(schema) => schema,
            Err(reason) => {
                prims.push(text(node.rect.row, node.rect.col, reason, font_size, theme::RED()));
                return prims;
            }
        };
        let value = value_datum(&node.props, &schema);
        let lookup = dyn_lookup(node);
        let slot = Slot::new(&schema, &value).with_dyn(&lookup);
        let focused = viewport.focused_widget_id == Some(node.widget_id);
        let mut state = get_state(node);
        let normalized = slot.normalize(&state.cursor);
        if !focused && state.has_open_editor() || normalized != state.cursor {
            // Focus moved away (or the value changed under the cursor): an
            // open field or popup does not outlive the slot's focus.
            if !focused {
                state.field = None;
                state.popup = None;
            }
            state.cursor = normalized;
            set_state(node, state.clone());
        }
        let layout = node_layout(node, &schema, &value, &state, viewport.cell_w);
        let origin = node.rect;
        let at = |rect: Rect| offset(rect, origin);

        let text_color = resolve_named_color(&node.props, "text-color", theme::FG());
        let head_color = resolve_named_color(&node.props, "head-color", theme::SYN_KEYWORD());
        let paren_color = resolve_named_color(&node.props, "paren-color", theme::DIM());
        let atom_bg = resolve_named_color(&node.props, "atom-bg", theme::DROPDOWN_BG());
        let band_color = resolve_named_color(&node.props, "band-color", Color { r: 1.0, g: 1.0, b: 1.0, a: 0.05 });
        let focus = focus_color(&node.props);
        let rules = tint_rules(&node.props);
        let tint = resolve_named_color(&node.props, "tint-color", theme::SYN_STRING());
        let tint_band = Color { a: 0.16, ..tint };
        let tint_atom = Color { a: 0.28, ..tint };
        let is_tinted = |path: &Path| !rules.is_empty() && tinted(&value, path, &rules);
        let played = lit_value_paths(&node.props);
        let error_color = resolve_named_color(&node.props, "error-color", theme::RED());

        // Lit items: a ring around each item the sounding hit applied.
        let lit = get_f32_prop(&node.props, "lit", 0.0).max(0.0) as u64;
        if lit != 0 {
            let lit_color = resolve_named_color(&node.props, "lit-color", focus);
            let count = match &value {
                Datum::List(items) => items.len(),
                _ => 0,
            };
            for index in 0..count.min(64) {
                if lit & (1u64 << index) == 0 {
                    continue;
                }
                for rect in layout.item_rects(index, line_h) {
                    prims.push(rounded_rect(grow(at(rect), 0.1, viewport), lit_color, 10.0, viewport, true));
                    prims.push(rounded_rect(at(rect), theme::BG(), 8.0, viewport, true));
                }
            }
        }
        // Cursor ring first: every cell and band draws over it.
        if focused && state.field.is_none() {
            for rect in layout.cursor_rects(&slot, &state.cursor, line_h) {
                prims.push(rounded_rect(grow(at(rect), 0.12, viewport), focus, 10.0, viewport, true));
                prims.push(rounded_rect(at(rect), theme::BG(), 8.0, viewport, true));
            }
        }
        // Depth bands, outermost first: each nesting level a little lighter.
        let mut bands: Vec<&Band> = layout.bands.iter().collect();
        bands.sort_by_key(|band| band.depth);
        for band in bands {
            let rect = Rect {
                row: SlotLayout::line_row(band.line, line_h),
                col: band.col,
                width: band.width,
                height: line_h,
            };
            let color = if is_tinted(&band.path) { tint_band } else { band_color };
            prims.push(rounded_rect(at(rect), color, 8.0, viewport, true));
        }
        for placed in &layout.placed {
            let rect = at(SlotLayout::piece_rect(placed, line_h));
            let text_row = rect.row + (rect.height - 1.0) * 0.5;
            match &placed.piece {
                Piece::Num { text: t, path } | Piece::Word { text: t, path } => {
                    // the member the sounding hit played fills with the lit color
                    let playing = played.contains(path);
                    // A dyn name its source does not offer here: error band.
                    let invalid = matches!(placed.piece, Piece::Word { .. }) && slot.dyn_problem(path).is_some();
                    let bg = if playing {
                        resolve_named_color(&node.props, "lit-color", focus)
                    } else if invalid {
                        Color { a: 0.22, ..error_color }
                    } else if is_tinted(path) {
                        tint_atom
                    } else {
                        atom_bg
                    };
                    prims.push(rounded_rect(rect, bg, 8.0, viewport, true));
                    let fg = if playing {
                        theme::BG()
                    } else if invalid {
                        error_color
                    } else {
                        text_color
                    };
                    prims.push(text(text_row, rect.col + ATOM_PAD, t.clone(), font_size, fg));
                }
                Piece::Head { text: t, .. } => {
                    prims.push(text(text_row, rect.col + ATOM_PAD, t.clone(), font_size, head_color));
                }
                Piece::Open { .. } | Piece::Close { .. } => {
                    let glyph = if matches!(placed.piece, Piece::Open { .. }) { "(" } else { ")" };
                    prims.push(text(text_row, rect.col, glyph.to_string(), font_size, paren_color));
                }
                Piece::Plus { row, .. } => {
                    if *row {
                        prims.push(rounded_rect(rect, Color { a: band_color.a * 1.6, ..band_color }, 8.0, viewport, true));
                    }
                    let width = render_text_width("+", font_size, viewport.cell_w);
                    prims.push(text(text_row, rect.col + (rect.width - width) * 0.5, "+".to_string(), font_size, paren_color));
                }
                Piece::Field { text: t } => {
                    prims.push(rounded_rect(grow(rect, 0.08, viewport), focus, 8.0, viewport, true));
                    prims.push(rounded_rect(rect, theme::TEXT_INPUT_BG(), 8.0, viewport, true));
                    let text_col = rect.col + 0.45;
                    prims.push(text(text_row, text_col, t.clone(), font_size, text_color));
                    if let Some(field) = &state.field {
                        let before = field.before_caret();
                        let caret_col = text_col + render_text_width(&before, font_size, viewport.cell_w);
                        prims.push(GpuPrimitive::ForegroundRect(super::GpuRectPrimitive {
                            rect: Rect { row: rect.row + 0.12, col: caret_col, width: 0.08, height: rect.height - 0.24 },
                            color: focus,
                        }));
                    }
                }
            }
        }

        if focused {
            draw_overlay(node, &slot, &state, &layout, viewport, line_h);
        } else {
            super::remove_overlay(node.widget_id);
        }
        prims
    }
}

/// The popup under the cursor's word / head, or the field's error and
/// completions, drawn as a frame overlay (the dropdown menu's look) and
/// registered for pointer hit tests.
fn draw_overlay(
    node: &LayoutNode,
    slot: &Slot<'_>,
    state: &SlotState,
    layout: &SlotLayout,
    viewport: WidgetViewport,
    line_h: f32,
) {
    let (anchor, rows, highlight, error) = if let Some(popup) = &state.popup {
        let target = Stop::Item(popup.path.clone());
        // A dyn word's popup: its details and hints come from the same rows.
        let rows = match slot.value.get(&popup.path) {
            Some(Datum::Word(word)) if !popup.head && slot.dyn_field(&target, word) => {
                let rows = slot.completion_rows(&target, "");
                Completions { words: popup.options.clone(), ..rows }
            }
            _ => Completions { words: popup.options.clone(), ..Completions::default() },
        };
        (layout.anchor(Some(&target), line_h), rows, popup.highlight, None)
    } else if let Some(field) = &state.field {
        (layout.anchor(None, line_h), state.field_completion_rows(slot), field.highlight, field.error.clone())
    } else {
        super::remove_overlay(node.widget_id);
        return;
    };
    let Some(anchor) = anchor else {
        super::remove_overlay(node.widget_id);
        return;
    };
    let options = &rows.words;
    if options.is_empty() && error.is_none() && rows.hints.is_empty() {
        super::remove_overlay(node.widget_id);
        return;
    }
    let menu_font = menu_style::MENU_FONT_SIZE;
    // Lead rows (not selectable): the error, then a source's hints.
    let lead = usize::from(error.is_some()) + rows.hints.len();
    // Below the anchor if it fits, else whichever side has more room, the
    // window shrunk to fit (it scrolls with the highlight).
    let screen_row = node.rect.row + anchor.row - viewport.scroll_top;
    let screen_col = node.rect.col + anchor.col - viewport.scroll_left;
    let viewport_bottom = viewport.overlay_viewport_bottom;
    let viewport_top = viewport_bottom - viewport.vp_h / viewport.cell_h.max(1.0);
    let below = screen_row + anchor.height + 0.15;
    let rows_in = |room: f32| ((room - PANEL_PADDING_V * 2.0 - 0.1) / MENU_ROW_HEIGHT).floor().max(1.0) as usize;
    let (room_below, room_above) = (rows_in(viewport_bottom - below), rows_in(screen_row - 0.15 - viewport_top));
    let window = if state.popup.is_some() { MAX_POPUP_ROWS } else { MAX_COMPLETIONS };
    let wanted = lead + options.len().min(window);
    let place_below = room_below >= wanted || room_below >= room_above;
    let fit = if place_below { room_below } else { room_above };
    let visible = options.len().min(window).min(fit.saturating_sub(lead).max(1));
    let first = highlight.saturating_sub(visible.saturating_sub(1)).min(options.len().saturating_sub(visible));
    let text_w = |t: &str| render_text_width(t, menu_font, viewport.cell_w);
    let detail_gap = 1.2;
    let detail = |index: usize| rows.details.get(index).filter(|detail| !detail.is_empty());
    let content_width = options
        .iter()
        .enumerate()
        .map(|(index, option)| text_w(option) + detail(index).map_or(0.0, |d| detail_gap + text_w(d)))
        .chain(error.iter().map(|reason| text_w(reason)))
        .chain(rows.hints.iter().map(|hint| text_w(hint)))
        .fold(0.0_f32, f32::max);
    let width = (content_width + menu_style::TEXT_PADDING_H * 2.0).max(anchor.width).max(6.0);
    let height = (lead + visible) as f32 * MENU_ROW_HEIGHT + PANEL_PADDING_V * 2.0;
    let top = if place_below { below } else { screen_row - height - 0.15 };
    let panel = Rect { row: top, col: screen_col, width, height };

    super::set_overlay(node.widget_id, panel);
    OVERLAY_ROWS.with(|rows| rows.borrow_mut().insert(node.widget_id, OverlayRows { lead, first, visible }));
    menu_style::emit_panel_chrome(panel, theme::DROPDOWN_MENU_BG(), theme::DROPDOWN_MENU_BORDER(), viewport);
    let row_rect = |index: usize| Rect {
        row: panel.row + PANEL_PADDING_V + index as f32 * MENU_ROW_HEIGHT,
        col: panel.col,
        width: panel.width,
        height: MENU_ROW_HEIGHT,
    };
    let text_row = |rect: Rect| rect.row + (MENU_ROW_HEIGHT - 1.0) * 0.5;
    if let Some(reason) = error {
        let rect = row_rect(0);
        let color = resolve_named_color(&node.props, "error-color", theme::RED());
        super::push_overlay_primitive(text(text_row(rect), rect.col + menu_style::TEXT_PADDING_H, reason, menu_font, color));
    }
    for (index, hint) in rows.hints.iter().enumerate() {
        let rect = row_rect(lead - rows.hints.len() + index);
        super::push_overlay_primitive(text(text_row(rect), rect.col + menu_style::TEXT_PADDING_H, hint.clone(), menu_font, theme::DIM()));
    }
    for (offset, option) in options.iter().skip(first).take(visible).enumerate() {
        let rect = row_rect(lead + offset);
        if first + offset == highlight {
            menu_style::emit_row_highlight(rect, theme::DROPDOWN_HOVER_BG(), viewport);
        }
        let fg = if slot.schema.is_form_head(option) { theme::SYN_KEYWORD() } else { theme::FG() };
        super::push_overlay_primitive(text(text_row(rect), rect.col + menu_style::TEXT_PADDING_H, option.clone(), menu_font, fg));
        if let Some(detail) = detail(first + offset) {
            let col = rect.col + rect.width - menu_style::TEXT_PADDING_H - text_w(detail);
            super::push_overlay_primitive(text(text_row(rect), col, detail.clone(), menu_font, theme::DIM()));
        }
    }
}

/// Scrub rate: a full range in ~24 rows of travel, never finer than :step.
fn scrub_per_row(min: f64, max: f64, step: f64) -> f64 {
    let range = if min.is_finite() && max.is_finite() { (max - min) / 24.0 } else { 0.0 };
    range.max(step).max(if step > 0.0 { step } else { 0.01 })
}

thread_local! {
    static LAST_CELL_W: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Gestures begin without the cell width; the last one drawn or clicked is it.
fn remember_cell_w(cell_w: f32) {
    if cell_w > 0.0 {
        LAST_CELL_W.with(|last| last.set(cell_w));
    }
}

fn last_cell_w() -> f32 {
    LAST_CELL_W.with(|last| last.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sexp_slot::read_value;

    fn row_schema() -> Schema {
        Schema::parse(
            &read_value(
                "(forms (word left right accent rev stac ghost swap)
                        (form trunc (num :min 1 :max 32 :step 1 :decimals 0 :default 3))
                        (form fast (num :min 1 :max 8 :step 1 :decimals 0 :default 2))
                        (form every (num :min 1 :max 16 :step 1 :decimals 0 :default 4)
                                    (word left right accent rev stac ghost swap)))",
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn datum(text: &str) -> Datum {
        Datum::from_value(&read_value(text).unwrap())
    }

    /// One cell per char: easy arithmetic.
    fn chars(text: &str) -> f32 {
        text.chars().count() as f32
    }

    fn kinds(layout: &SlotLayout) -> Vec<String> {
        layout
            .placed
            .iter()
            .map(|p| match &p.piece {
                Piece::Num { text, .. } | Piece::Word { text, .. } => text.clone(),
                Piece::Head { text, .. } => format!("{text}:"),
                Piece::Open { .. } => "(".into(),
                Piece::Close { .. } => ")".into(),
                Piece::Plus { row: true, .. } => "[+]".into(),
                Piece::Plus { row: false, .. } => "+".into(),
                Piece::Field { text } => format!("[{text}]"),
            })
            .collect()
    }

    #[test]
    fn a_row_lays_out_as_forms_words_and_a_trailing_plus() {
        let schema = row_schema();
        let value = datum("((trunc (3 1)) right (every 2 (rev swap)))");
        let layout = layout_slot(&Slot::new(&schema, &value), None, &chars, f32::INFINITY);
        assert_eq!(
            kinds(&layout),
            vec![
                "(", "trunc:", "(", "3", "1", "+", ")", ")", "right", "(", "every:", "2", "(", "rev",
                "swap", "+", ")", ")", "[+]"
            ]
        );
        assert_eq!(layout.lines, 1);
        // Bands: the trunc form, its list, the every form, its word cycle.
        let depths: Vec<usize> = layout.bands.iter().map(|b| b.depth).collect();
        assert_eq!(depths, vec![1, 2, 1, 2]);
        // The nested band sits inside its form's band.
        let (outer, inner) = (&layout.bands[0], &layout.bands[1]);
        assert!(inner.col > outer.col && inner.col + inner.width < outer.col + outer.width);
    }

    #[test]
    fn a_narrow_slot_wraps_between_pieces_and_grows_taller() {
        let schema = row_schema();
        let value = datum("((fast (1 2 3 4 5 6 7 8)) right left)");
        let wide = layout_slot(&Slot::new(&schema, &value), None, &chars, f32::INFINITY);
        let narrow = layout_slot(&Slot::new(&schema, &value), None, &chars, 14.0);
        assert!(narrow.lines > 1);
        assert!(narrow.width <= 14.0 + 0.001, "{}", narrow.width);
        assert!(narrow.height(1.3) > wide.height(1.3));
        // A list split over lines gets one band segment per line.
        let list_bands = narrow.bands.iter().filter(|b| b.path == vec![0, 1]).count();
        assert!(list_bands >= 2, "{list_bands}");
    }

    #[test]
    fn wrap_false_keeps_a_narrow_slot_on_one_line() {
        let mut node = LayoutNode {
            widget_id: 11,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "sexp-slot".to_string(),
            rect: Rect { row: 0.0, col: 0.0, width: 6.0, height: 1.3 },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let schema = row_schema();
        let value = datum("((fast (1 2 3 4 5 6 7 8)) right left)");
        let lines = |node: &LayoutNode| node_layout(node, &schema, &value, &SlotState::default(), 8.0).lines;
        assert!(lines(&node) > 1);
        node.props.insert("wrap".into(), Value::Bool(false));
        assert_eq!(lines(&node), 1);
    }

    #[test]
    fn the_open_field_replaces_its_target_and_grows_with_its_text() {
        let schema = row_schema();
        let value = datum("(right)");
        let slot = Slot::new(&schema, &value);
        let plus = Stop::Plus(vec![]);
        let short = layout_slot(&slot, Some((&plus, "ev")), &chars, f32::INFINITY);
        assert_eq!(kinds(&short), vec!["right", "[ev]"]);
        let long = layout_slot(&slot, Some((&plus, "(every 2 (rev swap))")), &chars, f32::INFINITY);
        assert!(long.width > short.width);
        let replaced = layout_slot(&slot, Some((&Stop::Item(vec![0]), "(le")), &chars, f32::INFINITY);
        assert_eq!(kinds(&replaced), vec!["[(le]", "[+]"]);
    }

    #[test]
    fn clicks_find_the_stop_under_the_pointer() {
        let schema = row_schema();
        let value = datum("((every 2 rev) left)");
        let slot = Slot::new(&schema, &value);
        let layout = layout_slot(&slot, None, &chars, f32::INFINITY);
        let find = |label: &str| {
            let index = kinds(&layout).iter().position(|k| k == label).unwrap();
            let p = &layout.placed[index];
            layout.stop_at(p.col + p.width * 0.5, 0.5, 1.3).map(|(stop, _)| stop)
        };
        assert_eq!(find("every:"), Some(Stop::Item(vec![0])));
        assert_eq!(find("2"), Some(Stop::Item(vec![0, 1])));
        assert_eq!(find("rev"), Some(Stop::Item(vec![0, 2])));
        assert_eq!(find("left"), Some(Stop::Item(vec![1])));
        assert_eq!(find("[+]"), Some(Stop::Plus(vec![])));
        // The cursor ring of a form is its head; of a plain list, its band.
        assert_eq!(layout.cursor_rects(&slot, &Stop::Item(vec![0]), 1.3).len(), 1);
        let list = datum("((fast (1 2)))");
        let list_slot = Slot::new(&schema, &list);
        let list_layout = layout_slot(&list_slot, None, &chars, f32::INFINITY);
        let ring = list_layout.cursor_rects(&list_slot, &Stop::Item(vec![0, 1]), 1.3);
        assert_eq!(ring.len(), 1);
        assert!(ring[0].width > 4.0, "the whole ( 1 2 + ) band");
    }

    #[test]
    fn scrubbing_spans_a_range_and_respects_the_step() {
        assert_eq!(scrub_per_row(1.0, 25.0, 1.0), 1.0);
        assert!((scrub_per_row(0.0, 1.0, 0.01) - 1.0 / 24.0).abs() < 1e-9);
        assert_eq!(scrub_per_row(f64::NEG_INFINITY, f64::INFINITY, 0.0), 0.01);
    }

    #[test]
    fn commits_reach_on_change_with_the_stored_value() {
        let mut node = LayoutNode {
            widget_id: 7,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "sexp-slot".to_string(),
            rect: Rect { row: 0.0, col: 0.0, width: 30.0, height: 1.3 },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let mut runtime = crate::Runtime::new();
        let schema = runtime
            .eval_str("'(forms (word left right) (form trunc (num :min 1 :max 32 :step 1 :default 3)))")
            .unwrap()
            .unwrap();
        node.props.insert("schema".into(), schema);
        node.props.insert("value".into(), read_value("(left)").unwrap());
        node.props.insert("on-change".into(), Value::Keyword("cb".into()));
        let key = |code| WidgetKeyEvent { code, modifiers: KeyModifiers::NONE };
        use crossterm::event::KeyCode;
        // Right to the +, type a form, Enter: one commit.
        assert!(matches!(SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Right)), Some(WidgetEvent::Custom(Value::Nil))));
        for ch in "(trunc 40".chars() {
            SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Char(ch)));
        }
        assert!(editing_text(&node));
        assert!(escape_keeps_focus(&node));
        let event = SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Enter)).expect("commit");
        let output = SEXP_SLOT_WIDGET.handle_event(&node, event).expect("on-change");
        assert_eq!(output.args, vec![read_value("(\"left\" (\"trunc\" 32))").unwrap()]);
        assert!(!editing_text(&node));
        // Arrow keys past the ends are not the slot's.
        assert!(SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Up)).is_none());
        // Space outside a field falls through (transport).
        assert!(SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Char(' '))).is_none());
    }

    #[test]
    fn dyn_context_reaches_the_source_and_its_words_commit() {
        use crate::sexp_slot::dyn_words::{DynGroup, DynItem, DynWords, register_dyn_word_source};
        let seen: std::rc::Rc<RefCell<Vec<Value>>> = Default::default();
        let log = seen.clone();
        register_dyn_word_source(
            "widget-param",
            Box::new(move |context| {
                log.borrow_mut().push(context.clone());
                DynWords {
                    epoch: 0,
                    groups: vec![DynGroup {
                        group: "Instrument".into(),
                        items: vec![DynItem::new("instrument:cutoff", "Cutoff")],
                    }],
                    aliases: Vec::new(),
                }
            }),
        );
        let mut node = LayoutNode {
            widget_id: 13,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "sexp-slot".to_string(),
            rect: Rect { row: 0.0, col: 0.0, width: 40.0, height: 1.3 },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let mut runtime = crate::Runtime::new();
        let schema = runtime
            .eval_str("(list \"forms\" (list \"form\" \"plock\" (list \"fixed\" (list \"dyn\" \"widget-param\")) (list \"num\")))")
            .unwrap()
            .unwrap();
        node.props.insert("schema".into(), schema);
        node.props.insert("value".into(), read_value("((plock \"\" 0))").unwrap());
        node.props.insert("on-change".into(), Value::Keyword("cb".into()));
        node.props.insert("dyn-context".into(), Value::Number(3.0));
        let key = |code| WidgetKeyEvent { code, modifiers: KeyModifiers::NONE };
        use crossterm::event::KeyCode;
        // Onto the form, down to its name, type a fuzzy query, Enter.
        SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Down));
        for ch in "cut".chars() {
            SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Char(ch)));
        }
        let event = SEXP_SLOT_WIDGET.key_event(&node, key(KeyCode::Enter)).expect("commit");
        let output = SEXP_SLOT_WIDGET.handle_event(&node, event).expect("on-change");
        assert_eq!(output.args, vec![read_value("((\"plock\" \"instrument:cutoff\" 0))").unwrap()]);
        // Asked once, with the context untouched, across every keystroke.
        assert_eq!(*seen.borrow(), vec![Value::Number(3.0)]);
    }

    #[test]
    fn dyn_num_scrubs_and_formats_in_the_named_words_rails() {
        use crate::sexp_slot::dyn_words::{DynGroup, DynItem, DynWords, register_dyn_word_source};
        use crate::sexp_slot::schema::NumSpec;
        register_dyn_word_source(
            "widget-rails",
            Box::new(|_| DynWords {
                groups: vec![DynGroup {
                    group: "Instrument".into(),
                    items: vec![DynItem::new("instrument:cutoff", "Cutoff").with_num(NumSpec {
                        min: 20.0,
                        max: 20000.0,
                        step: 1.0,
                        decimals: 0,
                    })],
                }],
                ..DynWords::default()
            }),
        );
        let make_node = |widget_id: u64| LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "sexp-slot".to_string(),
            rect: Rect { row: 0.0, col: 0.0, width: 60.0, height: 1.3 },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let mut node = make_node(17);
        node.props.insert("schema".into(), read_value(
            "(forms (form plock (fixed (dyn widget-rails))
                       (dyn-num widget-rails (num :min -100000 :max 100000 :step 0.01 :decimals 2))))",
        ).unwrap());
        node.props.insert("value".into(), read_value("((plock \"instrument:cutoff\" 440))").unwrap());
        node.props.insert("on-change".into(), Value::Keyword("cb".into()));
        node.props.insert("dyn-context".into(), Value::Number(0.0));
        let schema = parse_schema(&node.props).unwrap();
        let value = value_datum(&node.props, &schema);
        let layout = node_layout(&node, &schema, &value, &SlotState::default(), 8.0);
        let num = layout.placed.iter().find(|p| matches!(p.piece, Piece::Num { .. })).unwrap();
        assert_eq!(num.piece, Piece::Num { path: vec![0, 2], text: "440".into() });
        // Unrouted: the fallback's decimals.
        let mut unrouted = make_node(18);
        unrouted.props = node.props.clone();
        unrouted.props.insert("dyn-context".into(), Value::Nil);
        let layout = node_layout(&unrouted, &schema, &value, &SlotState::default(), 8.0);
        assert!(layout.placed.iter().any(|p| p.piece == Piece::Num { path: vec![0, 2], text: "440.00".into() }));

        let (col, row) = (num.col + num.width * 0.5, 0.6);
        let gesture = SEXP_SLOT_WIDGET.begin_gesture(&node, col, row, KeyModifiers::NONE).expect("a number");
        let drag = |to_row: f32| {
            match SEXP_SLOT_WIDGET.mouse_event(
                &node, MouseEventKind::Drag(MouseButton::Left), col, to_row, None, Some(&gesture),
                KeyModifiers::NONE, 8.0, 16.0,
            ) {
                MouseEventOutcome::Dispatch(event) => {
                    let args = SEXP_SLOT_WIDGET.handle_event(&node, event).expect("on-change").args;
                    Datum::from_value(&args[0]).get(&[0, 2]).cloned()
                }
                _ => None,
            }
        };
        // One row up: a 24th of cutoff's range, snapped to its whole-Hz step.
        let Some(Datum::Num(up)) = drag(row - 1.0) else { panic!("scrub") };
        assert_eq!(up.fract(), 0.0);
        assert!((up - (440.0 + 19980.0 / 24.0)).abs() <= 1.0, "{up}");
        // Far up / down: clamped to the cutoff's rails, not the fallback's.
        assert_eq!(drag(row - 100.0), Some(Datum::Num(20000.0)));
        assert_eq!(drag(row + 100.0), Some(Datum::Num(20.0)));
    }

    #[test]
    fn lit_values_decode_to_member_paths_per_item() {
        let mut props = HashMap::new();
        let cell = |v: f64| std::rc::Rc::new(RefCell::new(Value::Number(v)));
        // item 0: none; item 1: [1, 3, 0] = digits 2, 4, 1.
        props.insert("lit-values".to_string(),
            Value::List(vec![cell(0.0), cell((2 + 4 * 64 + 64 * 64) as f64)]));
        assert_eq!(lit_value_paths(&props), vec![vec![1, 1, 3, 0]]);
    }

    #[test]
    fn tint_args_mark_a_forms_argument_subtree_only() {
        let value = datum("((on (and (fig 3) tail) (vel* 0.6)) (on left stac) left)");
        let rules = vec![("on".to_string(), 0)];
        // The selector and everything inside it.
        assert!(tinted(&value, &[0, 1], &rules));
        assert!(tinted(&value, &[0, 1, 1, 2], &rules));
        assert!(tinted(&value, &[1, 1], &rules));
        // Not the head, the word, or a bare top-level word.
        assert!(!tinted(&value, &[0, 0], &rules));
        assert!(!tinted(&value, &[0, 2], &rules));
        assert!(!tinted(&value, &[0, 2, 1], &rules));
        assert!(!tinted(&value, &[2], &rules));
        assert!(!tinted(&value, &[0, 1], &[]));
    }

    #[test]
    fn hover_reports_the_top_level_item_and_nil_on_leave() {
        let mut node = LayoutNode {
            widget_id: 9,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "sexp-slot".to_string(),
            rect: Rect { row: 0.0, col: 0.0, width: 60.0, height: 1.3 },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        node.props.insert("schema".into(), read_value(
            "(forms (word left right) (form trunc (num :min 1 :max 32 :step 1 :default 3)))",
        ).unwrap());
        node.props.insert("value".into(), read_value("(left (trunc 3))").unwrap());
        node.props.insert("on-hover".into(), Value::Keyword("hover".into()));
        let moved = |col: f32| {
            SEXP_SLOT_WIDGET.mouse_event(
                &node, MouseEventKind::Moved, col, 0.6, None, None, KeyModifiers::NONE, 8.0, 16.0,
            )
        };
        let reported = |outcome: MouseEventOutcome| match outcome {
            MouseEventOutcome::Dispatch(event) => {
                Some(SEXP_SLOT_WIDGET.handle_event(&node, event).expect("on-hover").args)
            }
            _ => None,
        };
        // Over `left`: item 0; staying on it reports nothing new.
        assert_eq!(reported(moved(0.5)), Some(vec![read_value("\"left\"").unwrap()]));
        assert_eq!(reported(moved(0.6)), None);
        // Over `(trunc 3)`'s number: item 1, as its stored value.
        let layout = node_layout(&node, &parse_schema(&node.props).unwrap(),
            &value_datum(&node.props, &parse_schema(&node.props).unwrap()), &SlotState::default(), 8.0);
        let num = layout.placed.iter().find(|p| matches!(p.piece, Piece::Num { .. })).unwrap();
        assert_eq!(
            reported(moved(num.col + num.width * 0.5)),
            Some(vec![read_value("(\"trunc\" 3)").unwrap()])
        );
        // The pointer leaving for another widget reports nil once.
        let left = pointer_moved_to(Some(1234));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].args, vec![Value::Nil]);
        assert!(pointer_moved_to(None).is_empty());
    }
}
