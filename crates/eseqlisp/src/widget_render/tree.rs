use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    get_stable_widget_id,
};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, WidgetInstance, WidgetViewport, ndc_bounds,
};
#[cfg(target_os = "macos")]
use crate::backend::Color;
#[cfg(target_os = "macos")]
use crate::theme;

// ── Tree state ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
struct TreeState {
    expanded: HashSet<Vec<usize>>,
    cursor_row: usize,
    cursor_view_active: bool,
    external_selected_row: Option<usize>,
    synced_selection: Option<String>,
    synced_items_hash: Option<u64>,
}

thread_local! {
    static TREE_STATES: RefCell<HashMap<u64, TreeState>> = RefCell::new(HashMap::new());
}

fn get_tree_state(widget_id: u64) -> TreeState {
    TREE_STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_tree_state(widget_id: u64, state: TreeState) {
    // Always sync LAST_EXPANDED so measure() picks up the latest state
    update_last_known_expanded(widget_id, &state.expanded);
    TREE_STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

fn tree_state_key(node: &LayoutNode) -> u64 {
    node.stable_widget_id.unwrap_or(node.widget_id)
}

fn tree_state_key_from_value(node: &Value) -> Option<u64> {
    get_stable_widget_id(node)
}

// ── Flattened row ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TreeRow {
    depth: usize,
    label: String,
    detail: Option<String>,
    icon: Option<f32>,
    has_children: bool,
    expanded: bool,
    is_header: bool,
    draggable: bool,
    drop_target: bool,
    path: Vec<usize>,
    item_value: Value,
}

fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let mut truncated: String = text.chars().take(max_chars - 3).collect();
    truncated.push_str("...");
    truncated
}

/// Get a keyword-keyed value from either a Map or a keyword-value list.
/// Handles both `{:label "foo"}` (Map) and `(:label "foo")` (List with alternating kw/val).
fn get_item_field(item: &Value, key: &str) -> Option<Value> {
    match item {
        Value::Map(map) => map.get(key).map(|rc| rc.borrow().clone()),
        Value::List(list) => {
            let mut i = 0;
            while i + 1 < list.len() {
                let k = list[i].borrow();
                if matches!(&*k, Value::Keyword(k) if k == key) {
                    return Some(list[i + 1].borrow().clone());
                }
                i += 2;
            }
            None
        }
        _ => None,
    }
}

fn item_string_field(item: &Value, key: &str) -> Option<String> {
    match get_item_field(item, key) {
        Some(Value::String(value)) | Some(Value::Keyword(value)) => Some(value),
        _ => None,
    }
}

fn item_bool_field(item: &Value, key: &str, default: bool) -> bool {
    match get_item_field(item, key) {
        Some(Value::Bool(value)) => value,
        _ => default,
    }
}

fn item_icon_value(item: &Value) -> Option<f32> {
    super::button::icon_name_value(&item_string_field(item, "icon")?)
}

fn item_is_header(item: &Value) -> bool {
    matches!(item_string_field(item, "kind").as_deref(), Some("header"))
}

/// Walk nested items, producing a flat list of visible rows.
/// When `expand_all` is true, all folder nodes are treated as expanded regardless
/// of the `expanded` set — used when a search filter is active so every matching
/// path is visible.
fn flatten_items(
    items: &Value,
    depth: usize,
    parent_path: &[usize],
    expanded: &HashSet<Vec<usize>>,
    expand_all: bool,
    rows: &mut Vec<TreeRow>,
) {
    let Value::List(list) = items else { return };
    for (i, item_rc) in list.iter().enumerate() {
        let item = item_rc.borrow();
        let label = match get_item_field(&item, "label") {
            Some(Value::String(s)) => s,
            Some(other) => crate::vm::format_lisp_value(&other),
            None => match &*item {
                Value::String(s) => s.clone(),
                _ => continue,
            },
        };
        let children = get_item_field(&item, "children");
        let has_children = matches!(&children, Some(Value::List(l)) if !l.is_empty());
        let is_header = item_is_header(&item);
        let detail = match get_item_field(&item, "detail") {
            Some(Value::String(s)) if !s.is_empty() => Some(s),
            Some(other) => {
                let formatted = crate::vm::format_lisp_value(&other);
                (!formatted.is_empty()).then_some(formatted)
            }
            None => None,
        };

        let mut path = parent_path.to_vec();
        path.push(i);

        let is_expanded = has_children && (expand_all || expanded.contains(&path));
        rows.push(TreeRow {
            depth,
            label,
            detail,
            icon: item_icon_value(&item),
            has_children,
            expanded: is_expanded,
            is_header,
            draggable: item_bool_field(&item, "draggable", !is_header),
            drop_target: item_bool_field(&item, "drop-target", !is_header),
            path: path.clone(),
            item_value: item.clone(),
        });

        if is_expanded {
            if let Some(ref children_val) = children {
                flatten_items(children_val, depth + 1, &path, expanded, expand_all, rows);
            }
        }
    }
}

/// Read :expand-all from a raw Value node (measure phase).
fn get_expand_all_value(node: &Value) -> bool {
    let Value::Map(map) = node else { return false };
    match map.get("expand-all") {
        Some(rc) => matches!(&*rc.borrow(), Value::Bool(true)),
        None => false,
    }
}

fn get_string_value(node: &Value, key: &str) -> Option<String> {
    let Value::Map(map) = node else { return None };
    match map.get(key) {
        Some(rc) => match &*rc.borrow() {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        },
        None => None,
    }
}

/// Read :expand-all from LayoutNode props (render/event phase).
fn get_expand_all_prop(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("expand-all"), Some(Value::Bool(true)))
}

/// Extract the :items prop from a raw Value node (measure phase).
fn get_items_value(node: &Value) -> Value {
    let Value::Map(map) = node else {
        return Value::Nil;
    };
    map.get("items")
        .map(|rc| rc.borrow().clone())
        .unwrap_or(Value::Nil)
}

/// Extract :items from a LayoutNode's props (render/event phase).
fn get_items_from_props(props: &HashMap<String, Value>) -> Value {
    props.get("items").cloned().unwrap_or(Value::Nil)
}

/// Convert a keyword-value list like (:label "foo" :path "/bar") to a Value::Map.
/// If already a map, returns as-is.
fn item_to_map(item: &Value) -> Value {
    match item {
        Value::Map(_) => item.clone(),
        Value::List(list) => {
            let mut map = HashMap::new();
            let mut i = 0;
            while i + 1 < list.len() {
                let k = list[i].borrow().clone();
                let v = list[i + 1].borrow().clone();
                if let Value::Keyword(key) = k {
                    map.insert(key, std::rc::Rc::new(RefCell::new(v)));
                }
                i += 2;
            }
            Value::Map(map)
        }
        _ => item.clone(),
    }
}

/// Toggle a tree node's expand/collapse state.
fn toggle_expand(state: &mut TreeState, path: &[usize]) {
    let path_vec = path.to_vec();
    if state.expanded.contains(&path_vec) {
        state.expanded.remove(&path_vec);
    } else {
        state.expanded.insert(path_vec);
    }
}

fn get_string_prop(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    match props.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn find_item_path_by_field(
    items: &Value,
    field: &str,
    needle: &str,
    parent_path: &[usize],
) -> Option<Vec<usize>> {
    let Value::List(list) = items else {
        return None;
    };
    for (i, item_rc) in list.iter().enumerate() {
        let item = item_rc.borrow();
        let mut path = parent_path.to_vec();
        path.push(i);

        let field_matches = match get_item_field(&item, field) {
            Some(Value::String(s)) => s == needle,
            Some(other) => crate::vm::format_lisp_value(&other) == needle,
            None if field == "label" => match &*item {
                Value::String(s) => s == needle,
                _ => false,
            },
            None => false,
        };
        if field_matches {
            return Some(path);
        }

        if let Some(children) = get_item_field(&item, "children")
            && let Some(found) = find_item_path_by_field(&children, field, needle, &path)
        {
            return Some(found);
        }
    }
    None
}

fn ancestor_paths(path: &[usize]) -> HashSet<Vec<usize>> {
    let mut expanded = HashSet::new();
    for depth in 1..path.len() {
        expanded.insert(path[..depth].to_vec());
    }
    expanded
}

fn external_selection_key(
    props: &HashMap<String, Value>,
) -> Option<(String, &'static str, String)> {
    if let Some(path) = get_string_prop(props, "selected-path") {
        return Some((format!("path:{path}"), "path", path));
    }
    if let Some(label) = get_string_prop(props, "selected-label") {
        return Some((format!("label:{label}"), "label", label));
    }
    None
}

fn external_selection_key_from_value(node: &Value) -> Option<(String, &'static str, String)> {
    if let Some(path) = get_string_value(node, "selected-path") {
        return Some((format!("path:{path}"), "path", path));
    }
    if let Some(label) = get_string_value(node, "selected-label") {
        return Some((format!("label:{label}"), "label", label));
    }
    None
}

fn hash_tree_item_value(value: &Value, state: &mut DefaultHasher) {
    match value {
        Value::Nil => {
            0_u8.hash(state);
        }
        Value::Bool(value) => {
            1_u8.hash(state);
            value.hash(state);
        }
        Value::Number(value) => {
            2_u8.hash(state);
            value.to_bits().hash(state);
        }
        Value::String(value) => {
            3_u8.hash(state);
            value.hash(state);
        }
        Value::Keyword(value) => {
            4_u8.hash(state);
            value.hash(state);
        }
        Value::Symbol(value) => {
            5_u8.hash(state);
            value.hash(state);
        }
        Value::List(values) => {
            6_u8.hash(state);
            values.len().hash(state);
            for value in values {
                hash_tree_item_value(&value.borrow(), state);
            }
        }
        Value::Map(map) => {
            7_u8.hash(state);
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                key.hash(state);
                if let Some(value) = map.get(key) {
                    hash_tree_item_value(&value.borrow(), state);
                }
            }
        }
        Value::Closure(index, _) | Value::Function(index) => {
            8_u8.hash(state);
            index.hash(state);
        }
        Value::NodeRef(index) => {
            9_u8.hash(state);
            index.hash(state);
        }
        Value::ReactiveRef {
            namespace,
            field,
            index,
            kind,
            ..
        } => {
            10_u8.hash(state);
            namespace.hash(state);
            field.hash(state);
            index.hash(state);
            kind.hash(state);
        }
        Value::NativeFunction(_) | Value::HostHandle { .. } => {
            11_u8.hash(state);
        }
    }
}

fn tree_items_hash(items: &Value) -> u64 {
    let mut state = DefaultHasher::new();
    hash_tree_item_value(items, &mut state);
    state.finish()
}

fn sync_state_with_external_selection(
    widget_id: u64,
    items: &Value,
    props: &HashMap<String, Value>,
    expand_all: bool,
    state: &mut TreeState,
) {
    let original_state = state.clone();
    let items_hash = tree_items_hash(items);
    let Some((selection_key, field, needle)) = external_selection_key(props) else {
        state.external_selected_row = None;
        if state.synced_selection.is_some() {
            state.cursor_view_active = false;
        }
        state.synced_selection = None;
        state.synced_items_hash = Some(items_hash);
        if *state != original_state {
            set_tree_state(widget_id, state.clone());
        }
        return;
    };
    if state.synced_selection.as_deref() == Some(selection_key.as_str())
        && state.synced_items_hash == Some(items_hash)
    {
        return;
    }

    state.external_selected_row = None;
    state.cursor_view_active = false;
    if let Some(path) = find_item_path_by_field(items, field, &needle, &[]) {
        if !expand_all {
            state.expanded.extend(ancestor_paths(&path));
        }
        let mut rows = Vec::new();
        flatten_items(items, 0, &[], &state.expanded, expand_all, &mut rows);
        if let Some(row_idx) = rows.iter().position(|row| row.path == path) {
            if row_is_interactive(&rows[row_idx]) {
                state.external_selected_row = Some(row_idx);
                state.cursor_row = row_idx;
            }
        }
    }
    state.synced_selection = Some(selection_key);
    state.synced_items_hash = Some(items_hash);
    if *state != original_state {
        set_tree_state(widget_id, state.clone());
    }
}

fn row_is_interactive(row: &TreeRow) -> bool {
    !row.is_header
}

fn first_interactive_row(rows: &[TreeRow]) -> Option<usize> {
    rows.iter().position(row_is_interactive)
}

fn next_interactive_row(rows: &[TreeRow], current: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .skip(current.saturating_add(1))
        .find_map(|(idx, row)| row_is_interactive(row).then_some(idx))
}

fn previous_interactive_row(rows: &[TreeRow], current: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .take(current)
        .rev()
        .find_map(|(idx, row)| row_is_interactive(row).then_some(idx))
}

fn nearest_interactive_row(rows: &[TreeRow], current: usize) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    if let Some(row) = rows.get(current)
        && row_is_interactive(row)
    {
        return Some(current);
    }
    next_interactive_row(rows, current)
        .or_else(|| previous_interactive_row(rows, current))
        .or_else(|| first_interactive_row(rows))
}

fn normalize_state_for_visible_rows(widget_id: u64, state: &mut TreeState, rows: &[TreeRow]) {
    if rows.is_empty() {
        return;
    }

    let original_state = state.clone();
    state.cursor_row = state.cursor_row.min(rows.len() - 1);
    if matches!(state.external_selected_row, Some(row) if row >= rows.len() || !row_is_interactive(&rows[row]))
    {
        state.external_selected_row = None;
    }
    if let Some(row) = nearest_interactive_row(rows, state.cursor_row) {
        state.cursor_row = row;
    }

    if *state != original_state {
        set_tree_state(widget_id, state.clone());
    }
}

pub(crate) fn selection_view_hint(node: &LayoutNode) -> Option<(String, usize, f32)> {
    if node.widget_type != "tree" {
        return None;
    }

    let items = get_items_from_props(&node.props);
    let expand_all = get_expand_all_prop(&node.props);
    let widget_key = tree_state_key(node);
    let mut state = get_tree_state(widget_key);
    sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
    let mut rows = Vec::new();
    flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);
    if rows.is_empty() {
        return None;
    }
    normalize_state_for_visible_rows(widget_key, &mut state, &rows);
    let selection_key = state.synced_selection.clone()?;
    let selected_row = if state.cursor_view_active {
        state.cursor_row
    } else {
        state.external_selected_row?
    };
    let view_key = format!(
        "{selection_key}:items:{}:row:{}",
        state.synced_items_hash.unwrap_or(0),
        selected_row
    );
    Some((view_key, selected_row, row_height_from_props(&node.props)))
}

pub(crate) fn current_content_height(node: &LayoutNode) -> Option<f32> {
    if node.widget_type != "tree" {
        return None;
    }

    let items = get_items_from_props(&node.props);
    let expand_all = get_expand_all_prop(&node.props);
    let widget_key = tree_state_key(node);
    let mut state = get_tree_state(widget_key);
    sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
    let mut rows = Vec::new();
    flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);
    Some(rows.len() as f32 * row_height_from_props(&node.props))
}

/// Build an action map for event dispatch.
/// Converts the item to a proper map so Lisp `(get item :label)` works.
fn make_action_value(action: &str, item: &Value) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "action".to_string(),
        std::rc::Rc::new(RefCell::new(Value::String(action.to_string()))),
    );
    map.insert(
        "item".to_string(),
        std::rc::Rc::new(RefCell::new(item_to_map(item))),
    );
    Value::Map(map)
}

fn make_drag_value(drag_type: &str, item: &Value) -> Value {
    let mut map = HashMap::new();
    map.insert(
        "kind".to_string(),
        std::rc::Rc::new(RefCell::new(Value::String("widget-drag".to_string()))),
    );
    map.insert(
        "drag-type".to_string(),
        std::rc::Rc::new(RefCell::new(Value::String(drag_type.to_string()))),
    );
    map.insert(
        "payload".to_string(),
        std::rc::Rc::new(RefCell::new(item_to_map(item))),
    );
    Value::Map(map)
}

pub(crate) fn tree_drop_info(
    node: &LayoutNode,
    drag_type: &str,
    payload: Value,
    local_col: f32,
    local_row: f32,
) -> Value {
    let items = get_items_from_props(&node.props);
    let expand_all = get_expand_all_prop(&node.props);
    let widget_key = tree_state_key(node);
    let mut state = get_tree_state(widget_key);
    sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
    let mut rows = Vec::new();
    flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);

    let rh = row_height_from_props(&node.props);
    let scroll_offset = find_parent_scroll_offset(node);
    let row_relative = local_row - node.rect.row + scroll_offset;
    let target = if row_relative >= 0.0 {
        rows.get((row_relative / rh).floor() as usize)
            .filter(|row| row.drop_target)
            .map(|row| item_to_map(&row.item_value))
            .unwrap_or(Value::Nil)
    } else {
        Value::Nil
    };

    let mut info = HashMap::new();
    info.insert(
        "type".to_string(),
        std::rc::Rc::new(RefCell::new(Value::String("drop".to_string()))),
    );
    info.insert(
        "x".to_string(),
        std::rc::Rc::new(RefCell::new(Value::Number(local_col as f64))),
    );
    info.insert(
        "y".to_string(),
        std::rc::Rc::new(RefCell::new(Value::Number(local_row as f64))),
    );
    info.insert(
        "drag-type".to_string(),
        std::rc::Rc::new(RefCell::new(Value::String(drag_type.to_string()))),
    );
    info.insert(
        "payload".to_string(),
        std::rc::Rc::new(RefCell::new(payload)),
    );
    info.insert("target".to_string(), std::rc::Rc::new(RefCell::new(target)));
    Value::Map(info)
}

// ── Widget definition ────────────────────────────────────────────────────────

pub struct TreeWidget;
pub struct TreeRowBgWidget;

pub static TREE_WIDGET: TreeWidget = TreeWidget;
pub static TREE_ROW_BG_WIDGET: TreeRowBgWidget = TreeRowBgWidget;

const INDENT_CELLS: f32 = 1.5;
const LABEL_APPROX_CHAR_WIDTH: f32 = 0.62;
const DETAIL_APPROX_CHAR_WIDTH: f32 = 0.52;
const LABEL_DETAIL_GAP: f32 = 1.2;

impl WidgetDefinition for TreeWidget {
    fn names(&self) -> &'static [&'static str] {
        &["tree"]
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(TREE_CHEVRON_SHADER)
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "items",
            "width",
            "height",
            "font-size",
            "row-height",
            "selected-path",
            "selected-label",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let items = get_items_value(node);
        let expand_all = get_expand_all_value(node);
        // widget_id isn't available during measure, but stable widget IDs are.
        // Use the tree-local cached expansion set so sibling trees do not
        // contaminate each other's measured content height.
        let mut rows = Vec::new();
        let mut expanded = get_last_known_expanded(tree_state_key_from_value(node));
        if !expand_all
            && let Some((_, field, needle)) = external_selection_key_from_value(node)
            && let Some(path) = find_item_path_by_field(&items, field, &needle, &[])
        {
            expanded.extend(ancestor_paths(&path));
        }
        flatten_items(&items, 0, &[], &expanded, expand_all, &mut rows);

        let rh = row_height_from_value(node);

        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                if constraints.max_width < f32::MAX {
                    constraints.max_width
                } else {
                    40.0
                }
            });
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(rows.len() as f32 * rh);

        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let items = get_items_from_props(props);
        let expand_all = get_expand_all_prop(props);
        let mut expanded = get_last_known_expanded(None);
        if !expand_all
            && let Some((_, field, needle)) = external_selection_key(props)
            && let Some(path) = find_item_path_by_field(&items, field, &needle, &[])
        {
            expanded.extend(ancestor_paths(&path));
        }
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &expanded, expand_all, &mut rows);

        let fg = crate::backend::Color {
            r: 0.88,
            g: 0.88,
            b: 0.88,
            a: 1.0,
        };
        let dim = crate::backend::Color {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        };
        let detail_color = resolve_named_color(
            props,
            "detail-color",
            crate::backend::Color {
                r: 0.45,
                g: 0.70,
                b: 0.90,
                a: 1.0,
            },
        );

        for (i, row) in rows.iter().enumerate() {
            let r = rect.row.round() as u16 + i as u16;
            if r >= rect.row.round() as u16 + rect.height.round() as u16 {
                break;
            }
            let indent = row.depth as u16 * 2;
            let col_start = rect.col.round() as u16 + indent;

            // Disclosure triangle
            if row.has_children {
                let tri = if row.expanded { '▼' } else { '▶' };
                buf.set(r, col_start, styled_cell(tri, dim, None));
            }

            // Label
            let label_col = col_start + 2;
            let color = if row.has_children { fg } else { dim };
            let right_edge = rect.col.round() as u16 + rect.width.round() as u16;
            let detail_width = row
                .detail
                .as_ref()
                .map(|detail| detail.chars().count() as u16)
                .unwrap_or(0);
            let label_right_edge =
                if detail_width > 0 && detail_width + 2 < rect.width.round() as u16 {
                    right_edge.saturating_sub(detail_width + 3)
                } else {
                    right_edge
                };
            let label_chars = label_right_edge.saturating_sub(label_col) as usize;
            let label = truncate_with_ellipsis(&row.label, label_chars);
            for (j, ch) in label.chars().enumerate() {
                let c = label_col + j as u16;
                if c >= label_right_edge {
                    break;
                }
                buf.set(r, c, styled_cell(ch, color, None));
            }

            if let Some(detail) = &row.detail {
                if detail_width + 1 < rect.width.round() as u16 {
                    let detail_col = right_edge.saturating_sub(detail_width + 1);
                    for (j, ch) in detail.chars().enumerate() {
                        let c = detail_col + j as u16;
                        if c >= right_edge {
                            break;
                        }
                        buf.set(r, c, styled_cell(ch, detail_color, None));
                    }
                }
            }
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        if !matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
            return MouseEventOutcome::Consume;
        }

        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let widget_key = tree_state_key(node);
        let mut state = get_tree_state(widget_key);
        sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);

        let rh = row_height_from_props(&node.props);

        let scroll_offset = find_parent_scroll_offset(node);
        let row_relative = local_row - node.rect.row + scroll_offset;
        let row_idx = (row_relative / rh).floor() as usize;
        if row_idx >= rows.len() {
            return MouseEventOutcome::Consume;
        }

        let row = &rows[row_idx];
        if !row_is_interactive(row) {
            return MouseEventOutcome::Consume;
        }

        if row.has_children {
            toggle_expand(&mut state, &row.path);
            state.cursor_row = row_idx;
            state.cursor_view_active = true;
            set_tree_state(widget_key, state);
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(make_action_value(
                "toggle",
                &row.item_value,
            )))
        } else {
            state.cursor_row = row_idx;
            state.cursor_view_active = true;
            set_tree_state(widget_key, state);
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(make_action_value(
                "select",
                &row.item_value,
            )))
        }
    }

    fn begin_gesture(&self, node: &LayoutNode, _local_col: f32, local_row: f32) -> Option<Value> {
        let drag_type = get_string_prop(&node.props, "drag-type")?;
        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let widget_key = tree_state_key(node);
        let mut state = get_tree_state(widget_key);
        sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);

        let rh = row_height_from_props(&node.props);
        let scroll_offset = find_parent_scroll_offset(node);
        let row_relative = local_row - node.rect.row + scroll_offset;
        let row_idx = (row_relative / rh).floor() as usize;
        let row = rows.get(row_idx)?;
        if row.has_children || !row.draggable {
            return None;
        }
        Some(make_drag_value(&drag_type, &row.item_value))
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let widget_key = tree_state_key(node);
        let mut state = get_tree_state(widget_key);
        sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);
        normalize_state_for_visible_rows(widget_key, &mut state, &rows);
        if first_interactive_row(&rows).is_none() {
            return None;
        }

        match key.code {
            KeyCode::Up => {
                let Some(row_idx) = previous_interactive_row(&rows, state.cursor_row) else {
                    return None;
                };
                state.cursor_row = row_idx;
                state.cursor_view_active = true;
                let row = rows[state.cursor_row].clone();
                set_tree_state(widget_key, state);
                Some(WidgetEvent::Custom(make_action_value(
                    "cursor-change",
                    &row.item_value,
                )))
            }
            KeyCode::Down => {
                let Some(row_idx) = next_interactive_row(&rows, state.cursor_row) else {
                    return None;
                };
                state.cursor_row = row_idx;
                state.cursor_view_active = true;
                let row = rows[state.cursor_row].clone();
                set_tree_state(widget_key, state);
                Some(WidgetEvent::Custom(make_action_value(
                    "cursor-change",
                    &row.item_value,
                )))
            }
            KeyCode::Right => {
                let row = &rows[state.cursor_row.min(rows.len() - 1)];
                if row.has_children && !row.expanded {
                    toggle_expand(&mut state, &row.path);
                    state.cursor_view_active = true;
                    let row = row.clone();
                    set_tree_state(widget_key, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "cursor-change",
                        &row.item_value,
                    )))
                } else if row.has_children && row.expanded {
                    // Move to first child
                    let Some(row_idx) = next_interactive_row(&rows, state.cursor_row) else {
                        return None;
                    };
                    state.cursor_row = row_idx;
                    state.cursor_view_active = true;
                    let row = rows[state.cursor_row].clone();
                    set_tree_state(widget_key, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "cursor-change",
                        &row.item_value,
                    )))
                } else {
                    None
                }
            }
            KeyCode::Left => {
                let row = &rows[state.cursor_row.min(rows.len() - 1)];
                if row.has_children && row.expanded {
                    toggle_expand(&mut state, &row.path);
                    state.cursor_view_active = true;
                    let row = row.clone();
                    set_tree_state(widget_key, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "cursor-change",
                        &row.item_value,
                    )))
                } else if row.depth > 0 {
                    // Move to parent
                    let parent_path: Vec<usize> = row.path[..row.path.len() - 1].to_vec();
                    if let Some(parent_idx) = rows.iter().position(|r| r.path == parent_path) {
                        state.cursor_row = parent_idx;
                        state.cursor_view_active = true;
                        let row = rows[parent_idx].clone();
                        set_tree_state(widget_key, state);
                        Some(WidgetEvent::Custom(make_action_value(
                            "cursor-change",
                            &row.item_value,
                        )))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            KeyCode::Enter => {
                let row = &rows[state.cursor_row.min(rows.len() - 1)];
                if row.has_children {
                    toggle_expand(&mut state, &row.path);
                    state.cursor_view_active = true;
                    set_tree_state(widget_key, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "toggle",
                        &row.item_value,
                    )))
                } else if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SHIFT | KeyModifiers::SUPER)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    set_tree_state(widget_key, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "modified-activate",
                        &row.item_value,
                    )))
                } else {
                    set_tree_state(widget_key, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "activate",
                        &row.item_value,
                    )))
                }
            }
            _ => None,
        }
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let widget_key = tree_state_key(node);
        let mut state = get_tree_state(widget_key);
        sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);

        let rh = row_height_from_props(&node.props);
        let scroll_offset = find_parent_scroll_offset(node);
        let row_relative = local_row - node.rect.row + scroll_offset;
        let row_idx = (row_relative / rh).floor() as usize;
        let row = rows.get(row_idx)?;
        if row.has_children || !row_is_interactive(row) {
            return None;
        }
        state.cursor_row = row_idx;
        state.cursor_view_active = true;
        set_tree_state(widget_key, state);
        Some(WidgetEvent::Custom(make_action_value(
            "activate",
            &row.item_value,
        )))
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(ref action) = event else {
            return None;
        };
        // Nil means just redraw (key nav, internal toggle)
        if matches!(action, Value::Nil) {
            return None;
        }

        let Value::Map(action_map) = action else {
            return None;
        };
        let action_type = action_map
            .get("action")
            .map(|rc| {
                let v = rc.borrow();
                match &*v {
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                }
            })
            .unwrap_or_default();
        let item = action_map
            .get("item")
            .map(|rc| rc.borrow().clone())
            .unwrap_or(Value::Nil);

        let callback_key = match action_type.as_str() {
            "toggle" => "on-toggle",
            "select" => "on-select",
            "activate" => "on-activate",
            "modified-activate" => "on-modified-activate",
            "cursor-change" => "on-cursor-change",
            _ => return None,
        };

        let callback = node
            .props
            .get(callback_key)
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned();

        let cb = callback?;
        Some(EventOutput {
            callback: cb,
            args: vec![item],
        })
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let widget_key = tree_state_key(node);
        let mut state = get_tree_state(widget_key);
        sync_state_with_external_selection(widget_key, &items, &node.props, expand_all, &mut state);
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);
        normalize_state_for_visible_rows(widget_key, &mut state, &rows);

        // Update the last-known expanded state for measure() to use
        update_last_known_expanded(widget_key, &state.expanded);

        let font_size = node
            .props
            .get("font-size")
            .and_then(|v| match v {
                Value::Number(n) => Some(*n as f32),
                _ => None,
            })
            .unwrap_or(DEFAULT_FONT_SIZE);

        let rh = row_height_from_props(&node.props);

        let bg_alt = resolve_named_color(&node.props, "row-bg-alt", theme::TREE_ROW_ALT_BG());
        let selected_bg = resolve_named_color(&node.props, "selected-bg", theme::WIDGET_FOCUS_BG());
        let folder_fg = resolve_named_color(&node.props, "folder-color", theme::FG());
        let fg = theme::FG();
        let default_file = Color {
            r: fg.r * 0.72,
            g: fg.g * 0.72,
            b: fg.b * 0.72,
            a: 1.0,
        };
        let file_fg = resolve_named_color(&node.props, "file-color", default_file);
        let chevron_fg = theme::FG_MUTED();
        let default_chevron = Color {
            r: (chevron_fg.r + fg.r) * 0.5,
            g: (chevron_fg.g + fg.g) * 0.5,
            b: (chevron_fg.b + fg.b) * 0.5,
            a: 1.0,
        };
        let triangle_fg = resolve_named_color(&node.props, "chevron-color", default_chevron);
        let detail_fg = resolve_named_color(&node.props, "detail-color", theme::STATUS_ACCENT());

        let mut prims = Vec::new();

        for (i, row) in rows.iter().enumerate() {
            let y = node.rect.row + i as f32 * rh;

            // Row background — only draw the alternate stripe. The other rows
            // are left as the underlying panel background.
            let is_interactive = row_is_interactive(row);
            let raw_selected = if state.cursor_view_active {
                state.cursor_row == i
            } else {
                state.external_selected_row == Some(i)
            };
            let is_selected = is_interactive && raw_selected;
            let show_bg = is_interactive && (is_selected || i % 2 == 1);
            if show_bg {
                let bg = if is_selected { selected_bg } else { bg_alt };
                let row_inset = 0.15; // horizontal inset for rounded rect
                let row_rect = Rect {
                    row: y,
                    col: node.rect.col + row_inset,
                    width: node.rect.width - row_inset * 2.0,
                    height: rh,
                };
                let (ndc_min, ndc_max) = ndc_bounds(row_rect, _viewport);
                let px_w = row_rect.width * _viewport.cell_w;
                let px_h = row_rect.height * _viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "tree-row".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: _viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: [bg.r, bg.g, bg.b, bg.a],
                        color_b: [0.0; 4],
                        color_c: [0.0, 0.0, 1.0, 1.0],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: true,
                });
            }

            let indent = row.depth as f32 * INDENT_CELLS;
            let x = node.rect.col + indent + 0.5; // small left margin

            // Disclosure chevron (SDF rendered)
            if row.has_children {
                let chevron_h = rh * 0.85;
                let chevron_w = chevron_h * 1.8; // wider to give horizontal room
                let chevron_rect = Rect {
                    row: y + (rh - chevron_h) * 0.5,
                    col: x - 0.2,
                    width: chevron_w,
                    height: chevron_h,
                };
                let (ndc_min, ndc_max) = ndc_bounds(chevron_rect, _viewport);
                let px_w = chevron_rect.width * _viewport.cell_w;
                let px_h = chevron_rect.height * _viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "tree".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: if row.expanded { 1.0 } else { 0.0 },
                        orientation: 0.0,
                        itime: _viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: [triangle_fg.r, triangle_fg.g, triangle_fg.b, triangle_fg.a],
                        color_b: [0.0; 4],
                        color_c: [0.0, 0.0, 1.0, 1.0],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }

            // Label text (transparent bg — row Rect already handles background)
            // Offset down slightly to vertically center text in the row
            let text_y = y + (rh - 1.0) * 0.5;
            let icon_height = rh.min(1.0);
            let cell_aspect = if _viewport.cell_w > 0.0 {
                _viewport.cell_h / _viewport.cell_w
            } else {
                1.0
            };
            let icon_width = (icon_height * cell_aspect).max(icon_height);
            let icon_col = x + 1.55;
            let label_x = if row.is_header {
                x + 0.8
            } else if row.icon.is_some() {
                icon_col + icon_width + 0.55
            } else {
                x + 2.2
            };
            let fg = if row.is_header {
                theme::FG_MUTED()
            } else if row.has_children {
                folder_fg
            } else {
                file_fg
            };
            let transparent = Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };
            if let Some(icon) = row.icon {
                let icon_rect = Rect {
                    row: y + (rh - icon_height) * 0.5 - 0.08,
                    col: icon_col,
                    width: icon_width + 0.12,
                    height: icon_height + 0.12,
                };
                let (ndc_min, ndc_max) = ndc_bounds(icon_rect, _viewport);
                let px_w = icon_rect.width * _viewport.cell_w;
                let px_h = icon_rect.height * _viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "button-icon".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: icon,
                        orientation: 0.0,
                        itime: _viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: [file_fg.r, file_fg.g, file_fg.b, file_fg.a],
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }
            let detail_layout = row.detail.as_ref().map(|detail| {
                let detail_width =
                    (detail.chars().count() as f32 * DETAIL_APPROX_CHAR_WIDTH).max(1.0);
                let detail_col = node.rect.col + node.rect.width - detail_width - 0.9;
                (detail, detail_width, detail_col)
            });
            let label_available_width = detail_layout
                .as_ref()
                .map(|(_, _, detail_col)| detail_col - label_x - LABEL_DETAIL_GAP)
                .unwrap_or(node.rect.col + node.rect.width - label_x - 0.9)
                .max(0.0);
            let label_max_chars =
                (label_available_width / LABEL_APPROX_CHAR_WIDTH).floor() as usize;
            let label_text = truncate_with_ellipsis(&row.label, label_max_chars);
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_y,
                    col: label_x,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: label_text,
                    font_size: if row.is_header {
                        (font_size * 0.82).min(10.0)
                    } else {
                        font_size
                    },
                    scale: 1.0,
                    fg,
                    bg: transparent,
                },
            ));

            if let Some((detail, detail_width, detail_col)) = detail_layout {
                if detail_col > label_x {
                    prims.push(MetalPrimitive::ProportionalText(
                        MetalProportionalTextPrimitive {
                            row: text_y,
                            col: detail_col,
                            align_width: detail_width,
                            h_align: 1.0,
                            text: detail.to_string(),
                            font_size: font_size * 0.82,
                            scale: 1.0,
                            fg: detail_fg,
                            bg: transparent,
                        },
                    ));
                }
            }
        }

        prims
    }
}

// ── Helpers for measure/render coordination ──────────────────────────────────

// Since measure() doesn't have the transient widget_id, cache expansion by the
// stable widget ID embedded in the widget value. A fallback is kept for callers
// without stable IDs (for example simple TUI tests).

thread_local! {
    static LAST_EXPANDED: RefCell<HashMap<u64, HashSet<Vec<usize>>>> =
        RefCell::new(HashMap::new());
    static LAST_EXPANDED_FALLBACK: RefCell<Option<HashSet<Vec<usize>>>> =
        const { RefCell::new(None) };
}

fn get_last_known_expanded(widget_key: Option<u64>) -> HashSet<Vec<usize>> {
    if let Some(widget_key) = widget_key {
        return LAST_EXPANDED
            .with(|cell| cell.borrow().get(&widget_key).cloned().unwrap_or_default());
    }
    LAST_EXPANDED_FALLBACK.with(|cell| cell.borrow().clone().unwrap_or_default())
}

fn update_last_known_expanded(widget_key: u64, expanded: &HashSet<Vec<usize>>) {
    LAST_EXPANDED.with(|cell| {
        cell.borrow_mut().insert(widget_key, expanded.clone());
    });
    LAST_EXPANDED_FALLBACK.with(|cell| {
        *cell.borrow_mut() = Some(expanded.clone());
    });
}

// ── Row background widget ────────────────────────────────────────────────────

impl WidgetDefinition for TreeRowBgWidget {
    fn names(&self) -> &'static [&'static str] {
        &["tree-row"]
    }

    fn measure(
        &self,
        _node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        None
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(super::ROUNDED_RECT_SHADER)
    }
}

// ── Metal shaders ────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
const TREE_CHEVRON_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float expanded = in.value_t;
    float4 col = in.color_a;

    // Aspect-corrected coordinates: x in [-a, a], y in [-1, 1]
    float a = in.aspect;
    float2 p = float2((uv.x - 0.5) * 2.0 * a, (uv.y - 0.5) * 2.0);

    // Right chevron ">" — endpoints in aspect-corrected space
    float2 r_pt = float2(0.25 * a, 0.0);
    float2 r_a = float2(-0.25 * a, -0.5);
    float2 r_b = float2(-0.25 * a, 0.5);

    // Down chevron "v" — endpoints in aspect-corrected space
    float2 d_pt = float2(0.0, 0.3);
    float2 d_a = float2(-0.55 * a, -0.3);
    float2 d_b = float2(0.55 * a, -0.3);

    // Interpolate between right and down chevron
    float s = expanded;
    float2 pt = r_pt * (1.0 - s) + d_pt * s;
    float2 arm_a = r_a * (1.0 - s) + d_a * s;
    float2 arm_b = r_b * (1.0 - s) + d_b * s;

    // SDF for two line segments (arm_a -> pt, pt -> arm_b)
    float2 pa1 = p - arm_a;
    float2 ba1 = pt - arm_a;
    float h1 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    float seg1 = length(pa1 - ba1 * h1);

    float2 pa2 = p - pt;
    float2 ba2 = arm_b - pt;
    float h2 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    float seg2 = length(pa2 - ba2 * h2);

    float d = min(seg1, seg2);

    // Stroke width + anti-aliasing
    float stroke = 0.18;
    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard_fragment(); }

    return float4(col.rgb, col.a * mask);
}
"#;

/// Find the scroll offset from the nearest parent scroll container.
fn find_parent_scroll_offset(_node: &LayoutNode) -> f32 {
    super::scroll::current_event_scroll_offset()
}

fn row_height_from_value(node: &Value) -> f32 {
    get_prop_num(node, "row-height")
        .map(f64_to_f32)
        .unwrap_or(DEFAULT_ROW_HEIGHT)
        .max(MIN_ROW_HEIGHT)
}

fn row_height_from_props(props: &HashMap<String, Value>) -> f32 {
    match props.get("row-height") {
        Some(Value::Number(n)) => (*n as f32).max(MIN_ROW_HEIGHT),
        _ => DEFAULT_ROW_HEIGHT,
    }
}

const DEFAULT_ROW_HEIGHT: f32 = 1.25;
const MIN_ROW_HEIGHT: f32 = 1.0;
