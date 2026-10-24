//! What the user's window shows, for the agent: the `<app-context>` block a
//! message is sent with, and the `read_buffer` tool. Typing in the pane
//! makes the pane the active tile, so the tile the user was in before it is
//! remembered every frame ([`remember_focus`]) and named first.
//!
//! Views say what they show themselves: the host for the views it owns
//! ([`HostView`], the instrument and effect editors), a mode's `:describe`
//! function for views written in Lisp. Neither is guessed from a name.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use eseqlisp::buffer::Buffer;
use eseqlisp::host::BufferId;
use eseqlisp::reactive::ReactiveRegistry;
use eseqlisp::vm::{kind_name_of, InstanceId, KindKey, INSTANCE_NAMESPACE_PREFIX, KIND_KEYS_NAMESPACE_PREFIX};
use eseqlisp::vm::Value;
use eseqlisp::Editor;
use sequencer::agent::forms::top_level_forms;

use super::PANE_BUFFER;

/// The longest form the context quotes whole; a longer one is quoted as
/// the lines around the cursor.
const FORM_LINES: usize = 30;
/// Lines either side of the cursor quoted instead of a long form.
const AROUND_LINES: usize = 3;
/// The most lines one `read_buffer` call returns.
const READ_LINES: usize = 400;

/// A view the host owns, as the context names it: what its buffer shows
/// and how it stands, one fact per line, and the guide for working on it.
pub(crate) struct HostView {
    pub(crate) buffer: String,
    pub(crate) about: Vec<String>,
    pub(crate) guide: String,
}

thread_local! {
    /// The buffer of the last active tile that was not the pane's own.
    static LAST_FOCUS: Cell<Option<BufferId>> = const { Cell::new(None) };
    /// The last text buffer active: "this line" means its cursor even after
    /// a click on a toolbar or panel beside it (Eval, a knob).
    static LAST_CODE: Cell<Option<BufferId>> = const { Cell::new(None) };
}

/// Buffers the pane itself shows: never what the user means by "this".
fn is_pane_buffer(name: &str) -> bool {
    name == PANE_BUFFER || name.starts_with("*try: ")
}

/// Once per frame: note the active tile's buffer unless it is the pane's.
pub(super) fn remember_focus(editor: &Editor) {
    let buffer = editor.active_buffer();
    if !is_pane_buffer(&buffer.name) {
        LAST_FOCUS.with(|focus| focus.set(Some(buffer.id)));
        if has_text(buffer) {
            LAST_CODE.with(|code| code.set(Some(buffer.id)));
        }
    }
}

/// Whether a buffer holds text to read, rather than drawing widgets.
fn has_text(buffer: &Buffer) -> bool {
    buffer.widget_tree.is_none() && buffer.lines.iter().any(|line| !line.trim().is_empty())
}

/// One tile of the window: the buffer it shows and its tabs, as
/// (label, buffer name, selected).
struct Tile<'a> {
    buffer: &'a Buffer,
    tabs: Vec<(&'a str, &'a str, bool)>,
}

/// What views said about themselves, by buffer name: facts and guides.
#[derive(Default)]
struct Described {
    about: std::collections::HashMap<String, Vec<String>>,
    guides: Vec<String>,
}

impl Described {
    fn add(&mut self, buffer: &str, about: Vec<String>, guide: Option<String>) {
        self.about.entry(buffer.to_string()).or_default().extend(about);
        if let Some(guide) = guide.filter(|guide| !guide.is_empty() && !self.guides.contains(guide)) {
            self.guides.push(guide);
        }
    }
}

/// Ask every shown buffer's mode to describe it; `host` describes the
/// views the host owns.
fn describe(editor: &mut Editor, host: &[HostView]) -> Described {
    let mut described = Described::default();
    let shown: Vec<String> = tiles(editor).iter().map(|tile| tile.buffer.name.clone()).collect();
    for view in host.iter().filter(|view| shown.contains(&view.buffer)) {
        described.add(&view.buffer, view.about.clone(), Some(view.guide.clone()));
    }
    for name in &shown {
        match editor.describe_buffer(name) {
            Some(Ok(value)) => {
                let (about, guide) = described_value(&value);
                described.add(name, about.into_iter().collect(), guide);
            }
            Some(Err(why)) => described.add(name, vec![format!("(its :describe failed: {why})")], None),
            None => {}
        }
    }
    described
}

/// A describer's result: a string, or a dict with :about and :guide.
fn described_value(value: &Value) -> (Option<String>, Option<String>) {
    let nonblank = |text: Option<String>| text.map(|text| text.trim().to_string()).filter(|text| !text.is_empty());
    match value {
        Value::Map(map) => {
            let field = |key| nonblank(crate::history_commands::map_string(map, key));
            (field("about"), field("guide"))
        }
        Value::String(text) => (nonblank(Some(text.clone())), None),
        _ => (None, None),
    }
}

fn tiles(editor: &Editor) -> Vec<Tile<'_>> {
    editor
        .tile_root
        .leaf_ids()
        .into_iter()
        .filter_map(|id| editor.tile_root.find_leaf(id))
        .filter_map(|leaf| {
            let buffer = editor.buffers.get(leaf.buffer_idx)?;
            let tabs = leaf
                .tabs
                .iter()
                .enumerate()
                .filter_map(|(index, tab)| {
                    let name = editor.buffers.get(tab.buffer_idx)?.name.as_str();
                    Some((tab.label.as_str(), name, leaf.selected_tab == Some(index)))
                })
                .collect();
            Some(Tile { buffer, tabs })
        })
        .collect()
}

/// What the window shows, for a message sent now: None when it shows only
/// the pane. Its first line sums it up (the pane shows it under the message).
pub(super) fn header(editor: &mut Editor, host: &[HostView]) -> Option<String> {
    let described = describe(editor, host);
    let editor = &*editor;
    let mut tiles = tiles(editor);
    if tiles.iter().all(|tile| is_pane_buffer(&tile.buffer.name)) {
        return None;
    }
    let shown = |id: Option<BufferId>| id.filter(|id| tiles.iter().any(|tile| tile.buffer.id == *id));
    let focus = shown(LAST_FOCUS.with(Cell::get));
    let code = shown(LAST_CODE.with(Cell::get));
    // The code the user was last in first, then the tile they were last in,
    // then the rest in window order.
    for id in [focus, code].into_iter().flatten() {
        if let Some(index) = tiles.iter().position(|tile| tile.buffer.id == id) {
            let tile = tiles.remove(index);
            tiles.insert(0, tile);
        }
    }
    let name_of = |id: BufferId| tiles.iter().find(|tile| tile.buffer.id == id).map(|tile| tile.buffer);
    let summary = match (focus.and_then(name_of), code.and_then(name_of)) {
        (Some(last), Some(text)) if last.id == text.id => format!("Sent from {}, line {}.", text.name, text.cursor.0 + 1),
        (Some(last), Some(text)) => format!(
            "Sent from {}; the code they were last in is {}, line {}.",
            last.name,
            text.name,
            text.cursor.0 + 1
        ),
        (Some(last), None) => format!("Sent from {}.", last.name),
        (None, Some(text)) => format!("Sent with {} at line {}.", text.name, text.cursor.0 + 1),
        (None, None) => "Sent with the window showing its tiles.".to_string(),
    };
    let mut lines = vec![
        summary,
        "The tiles on the user's screen, the code they were last in and the tile they were last in (before the pane) first. \"This line\" means the cursor of the code they were last in:".to_string(),
    ];
    // Views sharing a mode often say the same thing: say it once, under
    // the first tile that does.
    let mut said = std::collections::HashSet::new();
    for tile in &tiles {
        let about: Vec<&String> = described
            .about
            .get(&tile.buffer.name)
            .into_iter()
            .flatten()
            .filter(|fact| said.insert(fact.as_str()))
            .collect();
        let role = Role { last_in: Some(tile.buffer.id) == focus, last_code: Some(tile.buffer.id) == code };
        describe_tile(editor, tile, &about, role, &mut lines);
    }
    if !described.guides.is_empty() {
        lines.push(format!(
            "Guides for what is on screen (read with `guide` before explaining or changing it): {}",
            described.guides.join(", ")
        ));
    }
    Some(lines.join("\n"))
}

/// What a tile was to the user just before they wrote.
#[derive(Clone, Copy)]
struct Role {
    /// The tile they were last in.
    last_in: bool,
    /// The text buffer they were last in: its cursor is "this line".
    last_code: bool,
}

fn describe_tile(editor: &Editor, tile: &Tile, about: &[&String], role: Role, lines: &mut Vec<String>) {
    let buffer = tile.buffer;
    let mut facts = vec![buffer.mode.name().to_string()];
    if is_pane_buffer(&buffer.name) {
        facts.push(if buffer.name == PANE_BUFFER { "this conversation".into() } else { "code you are trying".into() });
    }
    if let Some(path) = &buffer.path {
        facts.push(format!("file {}", path.display()));
    }
    if buffer.dirty {
        facts.push("unsaved edits".into());
    }
    if role.last_in {
        facts.push("last in".into());
    }
    if role.last_code && !role.last_in {
        facts.push("the code last in".into());
    }
    lines.push(format!("- {} ({})", buffer.name, facts.join(", ")));
    lines.extend(about.iter().map(|fact| format!("  {fact}")));
    if tile.tabs.len() > 1 {
        let tabs: Vec<String> = tile
            .tabs
            .iter()
            .map(|(label, name, selected)| {
                let selected = if *selected { ", selected" } else { "" };
                format!("{label} ({name}{selected})")
            })
            .collect();
        lines.push(format!("  tabs: {}", tabs.join(", ")));
    }
    if !has_text(buffer) {
        return;
    }
    let (row, col) = buffer.cursor;
    let line = buffer.lines.get(row).map_or("", String::as_str);
    lines.push(format!("  cursor: line {}, column {}: `{}`", row + 1, col + 1, line.trim()));
    // Other code on screen gets its cursor line; the code last in, the rest.
    if !role.last_code {
        return;
    }
    if let Some((start, end)) = editor.region_range_in_buffer(buffer.id) {
        lines.push(format!("  selected: line {} column {} to line {} column {}:", start.0 + 1, start.1 + 1, end.0 + 1, end.1 + 1));
        quote(&buffer.slice_range(start, end), lines);
    }
    let (first, last) = form_at(buffer).unwrap_or((row.saturating_sub(AROUND_LINES), row + AROUND_LINES));
    let last = last.min(buffer.lines.len().saturating_sub(1));
    lines.push(format!("  around the cursor (lines {}-{}):", first + 1, last + 1));
    quote(&numbered(buffer, first, last), lines);
}

/// The 0-based first and last lines of the top-level form holding the
/// cursor, when it parses and is short enough to quote whole.
fn form_at(buffer: &Buffer) -> Option<(usize, usize)> {
    let row = buffer.cursor.0 + 1;
    let forms = top_level_forms(&buffer.text()).ok()?;
    let form = forms.iter().find(|form| form.line <= row && row <= form.end_line)?;
    (form.end_line - form.line < FORM_LINES).then(|| (form.line - 1, form.end_line - 1))
}

fn quote(text: &str, lines: &mut Vec<String>) {
    lines.push("  ```".to_string());
    lines.extend(text.lines().map(|line| format!("  {line}")));
    lines.push("  ```".to_string());
}

/// Lines `first..=last` (0-based) of `buffer`, numbered from 1.
fn numbered(buffer: &Buffer, first: usize, last: usize) -> String {
    let width = (last + 1).to_string().len();
    buffer.lines[first..=last]
        .iter()
        .enumerate()
        .map(|(offset, line)| format!("{:>width$}  {line}", first + offset + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `read_buffer`: lines `from..=to` (1-based) of the open buffer `name`.
pub(super) fn read_buffer(editor: &Editor, name: &str, from: Option<usize>, to: Option<usize>) -> Result<String, String> {
    let Some(buffer) = editor.buffers.iter().find(|buffer| buffer.name == name) else {
        let mut open: Vec<&str> = editor.buffers.iter().map(|buffer| buffer.name.as_str()).collect();
        open.sort_unstable();
        return Err(format!("No buffer is named {name}; open buffers: {}", open.join(", ")));
    };
    if !has_text(buffer) {
        return Err(format!("{name} has no text: it draws widgets. `look` at it instead."));
    }
    let count = buffer.lines.len();
    let first = from.unwrap_or(1).max(1);
    if first > count {
        return Err(format!("{name} has {count} lines"));
    }
    let last = to.unwrap_or(count).min(count).min(first + READ_LINES - 1);
    if last < first {
        return Err("`to` is before `from`".to_string());
    }
    let mut facts = vec![buffer.mode.name().to_string()];
    if let Some(path) = &buffer.path {
        facts.push(format!("file {}", path.display()));
    }
    if buffer.dirty {
        facts.push("unsaved edits".into());
    }
    let more = if last < count { format!("; read on with from {}", last + 1) } else { String::new() };
    Ok(format!(
        "{name} ({}): lines {first}-{last} of {count}{more}\n{}",
        facts.join(", "),
        numbered(buffer, first - 1, last - 1)
    ))
}

/// The most fields `view_state` lists before folding the rest away.
const STATE_FIELDS: usize = 120;
/// How many instances of a kind one `view_state` row shows when they all
/// differ (track colours), and how many it lists that differ from the
/// common value (the steps that are on).
const INSTANCES_SHOWN: usize = 4;
const EXCEPTIONS_SHOWN: usize = 32;
/// The longest value `view_state` prints whole.
const VALUE_CHARS: usize = 160;

/// `view_state`: the reactive fields the buffer's drawn view reads, with
/// their values now: those its widgets bind (`#'kind.field` props, read by
/// index or whole) and those its rebuilt subtrees read. Fields that differ
/// only by a number (one per track, per cell) fold into one row.
pub(crate) fn view_state(editor: &Editor, name: &str) -> Result<String, String> {
    let Some(buffer) = editor.buffers.iter().find(|buffer| buffer.name == name) else {
        return Err(format!("No buffer is named {name}"));
    };
    let Some(layout) = editor.layout_for_buffer_named(name) else {
        return Err(if has_text(buffer) {
            format!("{name} is text, not a drawn view: read it with read_buffer.")
        } else {
            format!("{name} is not drawn now (a hidden tab or a closed tile), so it reads no state.")
        });
    };
    // (namespace, field) -> the indexes widgets bind, empty for whole reads.
    let mut fields: BTreeMap<(String, String), BTreeSet<usize>> = BTreeMap::new();
    let mut bound = Vec::new();
    ReactiveRegistry::collect_widget_binding_entries(&layout, &mut bound);
    for (key, _) in bound {
        let indexes = fields.entry((key.field.namespace, key.field.field)).or_default();
        indexes.extend(key.index);
    }
    let active = (editor.active_buffer().id == buffer.id).then(|| editor.runtime().current_committed_ui_snapshot()).flatten();
    if let Some(snapshot) = active.as_ref().or(buffer.committed_ui_snapshot.as_ref()) {
        for key in snapshot.reactive_fields() {
            fields.entry((key.namespace.clone(), key.field.clone())).or_default();
        }
    }
    if fields.is_empty() {
        return Ok(format!("{name} reads no reactive state: what it shows is fixed when it is built."));
    }
    let runtime = editor.runtime();
    let print = |value: &Value| clip_value(&runtime.format_value(value), value);
    // A whole value, or the one element a widget binds.
    let shown = |value: &Value, indexes: &BTreeSet<usize>| match (value, indexes.len()) {
        (Value::List(items), 1) => {
            let index = *indexes.first().expect("one index");
            items.get(index).map_or(format!("[{index}] (out of range)"), |item| format!("[{index}] {}", print(&item.borrow())))
        }
        _ => print(value),
    };
    // Kind instance fields group by kind and field (track.muted across
    // tracks); other namespaces by field shape (digits as #).
    let mut kinds: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    let mut others: Vec<(&str, String, Vec<(&str, String)>)> = Vec::new();
    for ((namespace, field), indexes) in &fields {
        if namespace.starts_with(KIND_KEYS_NAMESPACE_PREFIX) {
            continue;
        }
        if let Some(id) = namespace.strip_prefix(INSTANCE_NAMESPACE_PREFIX).and_then(|id| id.parse::<InstanceId>().ok()) {
            let Some(kind) = runtime.instance_kind(id) else { continue };
            let kind = kind.to_string();
            let key = instance_label(runtime, id);
            let value = runtime.instance_field(id, field).map_or_else(|why| format!("({why:?})"), |value| shown(&value, indexes));
            kinds.entry((kind, field.clone())).or_default().push((key, value));
            continue;
        }
        let value = runtime.reactive_field_value(namespace, field).map_or("(unset)".to_string(), |value| shown(value, indexes));
        let shape = field_shape(field);
        match others.last_mut() {
            Some((ns, last, members)) if ns == namespace && *last == shape => members.push((field, value)),
            _ => others.push((namespace, shape, vec![(field, value)])),
        }
    }
    let doc = |kind: &str, field: &str| {
        runtime
            .instance_kind_schema(kind)
            .and_then(|schema| schema.host.iter().find(|host| host.field.name == field))
            .and_then(|host| host.doc.as_deref())
            .map_or(String::new(), |doc| format!("  ; {}", sequencer::agent::harness::clip(doc, 100)))
    };
    let mut lines = vec![format!("{name} draws from {} reactive fields, as they are now.", fields.len())];
    if !kinds.is_empty() {
        lines.push("Kind fields (kind[key].field = value; a singleton has no key):".to_string());
    }
    for ((kind, field), members) in kinds.iter().take(STATE_FIELDS) {
        match members.as_slice() {
            [(key, value)] => lines.push(format!("  {key}.{field} = {value}{}", doc(kind, field))),
            _ => {
                let name = kind_name_of(kind);
                lines.push(format!("  {name}.{field} across {} {name} instances: {}{}", members.len(), spread(members), doc(kind, field)));
            }
        }
    }
    if !others.is_empty() {
        lines.push("Other fields (namespace.field = value):".to_string());
    }
    for (namespace, shape, members) in others.iter().take(STATE_FIELDS) {
        match members.as_slice() {
            [(field, value)] => lines.push(format!("  {namespace}.{field} = {value}")),
            [(first, value), ..] => {
                lines.push(format!("  {namespace}.{shape}: {} fields, e.g. {first} = {value}", members.len()))
            }
            [] => {}
        }
    }
    let rows = kinds.len() + others.len();
    if rows > 2 * STATE_FIELDS {
        lines.push(format!("  … more fields than shown ({rows} rows)"));
    }
    Ok(lines.join("\n"))
}

/// The sample rate DGenLisp values are worked out at: the app does not
/// tell Lisp its own, and 48 kHz is the usual one.
const DGEN_SAMPLERATE: f64 = 48_000.0;

/// What the DGenLisp defs `names` in `source` come to with `knobs` (params
/// missing from it take their defaults), `origin` saying where the knobs
/// came from.
pub(crate) fn dgen_values(source: &str, knobs: &HashMap<String, f64>, origin: &str, names: &[String]) -> Result<String, String> {
    use sequencer::agent::dgen_eval;
    if names.is_empty() {
        return Err("Name the defs to work out, e.g. `B, freqs`.".to_string());
    }
    let params = dgen_eval::params(source)?;
    let knob_line: Vec<String> = params
        .iter()
        .map(|(name, default)| match knobs.get(name) {
            Some(value) => format!("{name} {}", dgen_eval::describe(&dgen_eval::DgenValue::Scalar(*value))),
            None => format!("{name} {} (default)", dgen_eval::describe(&dgen_eval::DgenValue::Scalar(*default))),
        })
        .collect();
    let mut lines = vec![
        format!("With {origin}, at {} Hz:", DGEN_SAMPLERATE),
        format!("  params: {}", knob_line.join(", ")),
    ];
    for (name, value) in dgen_eval::evaluate(source, knobs, DGEN_SAMPLERATE, names)? {
        lines.push(format!("  {name} = {}", dgen_eval::describe(&value)));
    }
    Ok(lines.join("\n"))
}

/// `dgen_values_live`: [`dgen_values`] over the user's open buffer, with the
/// knobs of the instrument on `track` (1-based; the current track when None).
pub(super) fn dgen_values_live(
    editor: &mut Editor,
    buffer: &str,
    names: &[String],
    track: Option<usize>,
    inputs: &[(String, f64)],
) -> Result<String, String> {
    let Some(source) = editor.buffers.iter().find(|open| open.name == buffer).map(Buffer::text) else {
        return Err(format!("No buffer is named {buffer}"));
    };
    let track_expr = match track {
        Some(track) if track >= 1 => format!("(nth (eseq.kinds/tracks) {})", track - 1),
        Some(_) => return Err("`track` counts from 1".to_string()),
        None => "(let ((selection eseq.kinds/selection)) selection.track)".to_string(),
    };
    // Each param of the track's instrument: name, the value its knob shows,
    // and whether that value is 100 x what the DSP gets.
    let code = format!(
        "(let ((track {track_expr}))
           (if track
             (let ((device (first track.devices)))
               (list (+ track.index 1) track.name
                 (if device (map (lambda (p) (list p.name p.value p.percent)) device.params) (list))))
             nil))"
    );
    let value = editor.runtime_mut().eval_str(&code).map_err(|error| format!("could not read the track's knobs: {error:?}"))?;
    let Some(Value::List(parts)) = value else {
        return Err("No track is selected; pass `track`.".to_string());
    };
    let part = |index: usize| parts.get(index).map(|part| part.borrow().clone());
    let (Some(Value::Number(number)), Some(Value::String(name)), Some(Value::List(rows))) = (part(0), part(1), part(2)) else {
        return Err("could not read the track's knobs".to_string());
    };
    let mut knobs = HashMap::new();
    for row in rows {
        let row = row.borrow();
        let Value::List(cells) = &*row else { continue };
        let cell = |index: usize| cells.get(index).map(|cell| cell.borrow().clone());
        if let (Some(Value::String(param)), Some(Value::Number(value))) = (cell(0), cell(1)) {
            let percent = matches!(cell(2), Some(Value::Bool(true)));
            knobs.insert(param, if percent { value / 100.0 } else { value });
        }
    }
    knobs.extend(inputs.iter().cloned());
    let given: Vec<String> = inputs.iter().map(|(name, value)| format!("{name} {value}")).collect();
    let origin = match given.as_slice() {
        [] => format!("the knobs of track {number} ({name}) as they show now"),
        _ => format!("the knobs of track {number} ({name}) as they show now and {}", given.join(", ")),
    };
    dgen_values(&source, &knobs, &origin, names)
}

/// How an instance reads: `track[2]`, `step[track[0] 5]`, or the kind's
/// name for a singleton.
fn instance_label(runtime: &eseqlisp::Runtime, id: InstanceId) -> String {
    let kind_id = runtime.instance_kind(id).unwrap_or("?");
    let kind = kind_name_of(kind_id);
    // Only a parent-keyed kind's first key part is an instance (its
    // parent's id); every other part is a plain index, even one that
    // happens to equal some instance's id.
    let parent_keyed = runtime
        .instance_kind_schema(kind_id)
        .is_some_and(|schema| matches!(schema.key, KindKey::Under { .. }));
    let key = runtime.instance_key(id).unwrap_or_default();
    key_label(kind, key, parent_keyed, |parent| match parent != id {
        true => instance_label(runtime, parent),
        false => parent.to_string(),
    })
}

/// `kind[key …]`, naming the first key part with `parent` when the kind is
/// parent-keyed; the kind alone for an instance with no key.
fn key_label(kind: &str, key: &[u64], parent_keyed: bool, parent: impl Fn(u64) -> String) -> String {
    if key.is_empty() {
        return kind.to_string();
    }
    let parts: Vec<String> = key
        .iter()
        .enumerate()
        .map(|(index, part)| if index == 0 && parent_keyed { parent(*part) } else { part.to_string() })
        .collect();
    format!("{kind}[{}]", parts.join(" "))
}

/// One field across many instances: the value most share, then the
/// instances that differ (`false, except track[0] true`).
fn spread(members: &[(String, String)]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, value) in members {
        *counts.entry(value).or_default() += 1;
    }
    let (common, count) = counts.iter().max_by_key(|(_, count)| **count).map(|(value, count)| (*value, *count)).unwrap_or(("", 0));
    if count == members.len() {
        return format!("all {common}");
    }
    if count == 1 {
        let shown: Vec<String> =
            members.iter().take(INSTANCES_SHOWN).map(|(key, value)| format!("{key} {value}")).collect();
        let more = if members.len() > INSTANCES_SHOWN { ", …" } else { "" };
        return format!("{}{more}", shown.join(", "));
    }
    let differ: Vec<String> = members
        .iter()
        .filter(|(_, value)| value != common)
        .take(EXCEPTIONS_SHOWN)
        .map(|(key, value)| format!("{key} {value}"))
        .collect();
    let more = if members.len() - count > EXCEPTIONS_SHOWN { ", …" } else { "" };
    format!("{common}, except {}{more}", differ.join(", "))
}

/// A field name with each run of digits as `#`: `track-3-pattern` and
/// `track-12-pattern` share `track-#-pattern`.
fn field_shape(field: &str) -> String {
    let mut shape = String::with_capacity(field.len());
    for c in field.chars() {
        if c.is_ascii_digit() {
            if !shape.ends_with('#') {
                shape.push('#');
            }
        } else {
            shape.push(c);
        }
    }
    shape
}

/// `printed` with long decimals cut to 3 places: f32 values print as
/// 0.9090909361839294.
fn rounded(printed: &str) -> String {
    let mut out = String::with_capacity(printed.len());
    let mut decimals: Option<usize> = None;
    for c in printed.chars() {
        match (c, decimals) {
            ('.', None) if out.ends_with(|c: char| c.is_ascii_digit()) => {
                decimals = Some(0);
                out.push(c);
            }
            (c, Some(n)) if c.is_ascii_digit() => {
                if n < 3 {
                    out.push(c);
                }
                decimals = Some(n + 1);
            }
            (c, _) => {
                decimals = None;
                out.push(c);
            }
        }
    }
    out
}

/// A printed value cut to [`VALUE_CHARS`], with a list's length.
fn clip_value(printed: &str, value: &Value) -> String {
    let count = match value {
        Value::List(items) => Some(items.len()),
        _ => None,
    };
    let printed = rounded(&printed.replace('\n', " "));
    let clipped = sequencer::agent::harness::clip(&printed, VALUE_CHARS);
    match count {
        Some(count) if clipped != printed => format!("({count} items) {clipped}"),
        _ => clipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::mode::BufferMode;
    use eseqlisp::{EditorConfig, Runtime};

    fn editor_with_code() -> (Editor, BufferId) {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let source = "(def a 1)\n; the stiffness\n(def b\n  (* a 2))\n(def c 3)\n";
        let id = editor.open_scratch_buffer_with_mode("*code*", source, BufferMode::DGenLisp);
        (editor, id)
    }

    #[test]
    fn the_header_names_the_tile_last_in_its_cursor_and_the_form_under_it() {
        let (mut editor, id) = editor_with_code();
        let buffer = editor.buffers.iter_mut().find(|buffer| buffer.id == id).unwrap();
        buffer.cursor = (3, 4);
        remember_focus(&editor);
        let header = header(&mut editor, &[]).expect("a tile shows *code*");
        assert!(header.starts_with("Sent from *code*, line 4.\n"), "{header}");
        assert!(header.contains("- *code* (dgenlisp-mode, last in)"), "{header}");
        assert!(header.contains("cursor: line 4, column 5: `(* a 2))`"), "{header}");
        assert!(header.contains("around the cursor (lines 3-4):\n  ```\n  3  (def b\n  4    (* a 2))\n  ```"), "{header}");
    }

    #[test]
    fn views_describe_themselves_by_mode_or_host_and_name_their_guides() {
        let (mut editor, _) = editor_with_code();
        editor
            .runtime_mut()
            .eval_str(
                "(def demo-describe (buffer) (dict :about (str \"a demo of \" buffer) :guide \"effect\"))
                 (define-mode \"demo-mode\" :describe \"demo-describe\")
                 (define-mode \"child-mode\" :inherit \"demo-mode\")
                 (def broken (buffer) (undefined-thing))
                 (define-mode \"broken-mode\" :describe \"broken\")",
            )
            .unwrap();
        editor.refresh_runtime_side_effects();
        editor.open_scratch_buffer_with_mode("*demo*", "", BufferMode::Named("child-mode".into()));
        // One tile: *demo* now shows where *code* did.
        let host = [HostView {
            buffer: "*demo*".into(),
            about: vec!["the DGenLisp code of instrument pluck".into()],
            guide: "instrument".into(),
        }];
        let header = header(&mut editor, &host).unwrap();
        // The host's facts, then the mode's: an inheriting mode uses its
        // parent's describer.
        assert!(header.contains("- *demo* (child-mode)\n  the DGenLisp code of instrument pluck\n  a demo of *demo*"), "{header}");
        assert!(header.ends_with("(read with `guide` before explaining or changing it): instrument, effect"), "{header}");

        // A describer that throws is reported, not fatal.
        editor.open_scratch_buffer_with_mode("*broken*", "", BufferMode::Named("broken-mode".into()));
        let header = super::header(&mut editor, &[]).unwrap();
        assert!(header.contains("- *broken* (broken-mode)\n  (its :describe failed:"), "{header}");
    }

    #[test]
    fn many_instances_read_as_the_common_value_and_the_ones_that_differ() {
        let members = |values: &[&str]| -> Vec<(String, String)> {
            values.iter().enumerate().map(|(i, value)| (format!("track[{i}]"), value.to_string())).collect()
        };
        assert_eq!(spread(&members(&["false", "false", "false"])), "all false");
        assert_eq!(spread(&members(&["false", "true", "false"])), "false, except track[1] true");
        assert_eq!(spread(&members(&["1", "2", "3"])), "track[0] 1, track[1] 2, track[2] 3");
        assert_eq!(rounded("('rgb 0.9599999785 0.28 1) 3.0 v1.2345"), "('rgb 0.959 0.28 1) 3.0 v1.234");
        assert_eq!(field_shape("mod-in-level-12-3"), "mod-in-level-#-#");
    }

    #[test]
    fn an_instance_label_resolves_only_the_parent_slot_of_its_key() {
        // Step 3 of the track with id 41, where 3 is also some created
        // instance's id: only the parent slot is looked up.
        let parent = |id: u64| if id == 41 { "track[0]".to_string() } else { format!("chordchain#{id}") };
        assert_eq!(key_label("step", &[41, 3], true, parent), "step[track[0] 3]");
        assert_eq!(key_label("track", &[3], false, parent), "track[3]");
        assert_eq!(key_label("selection", &[], false, parent), "selection");
    }

    #[test]
    fn read_buffer_numbers_the_lines_it_reads_and_names_open_buffers_when_missing() {
        let (editor, _) = editor_with_code();
        let read = read_buffer(&editor, "*code*", Some(3), Some(4)).unwrap();
        assert_eq!(read, "*code* (dgenlisp-mode): lines 3-4 of 6; read on with from 5\n3  (def b\n4    (* a 2))");
        let missing = read_buffer(&editor, "*nope*", None, None).unwrap_err();
        assert!(missing.contains("*code*"), "{missing}");
    }
}
