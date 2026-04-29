use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
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

#[derive(Clone, Debug, Default)]
struct TreeState {
    expanded: HashSet<Vec<usize>>,
    selected_row: usize,
    synced_selection: Option<String>,
}

thread_local! {
    static TREE_STATES: RefCell<HashMap<u64, TreeState>> = RefCell::new(HashMap::new());
}

fn get_tree_state(widget_id: u64) -> TreeState {
    TREE_STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_tree_state(widget_id: u64, state: TreeState) {
    // Always sync LAST_EXPANDED so measure() picks up the latest state
    update_last_known_expanded(&state.expanded);
    TREE_STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

// ── Flattened row ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TreeRow {
    depth: usize,
    label: String,
    has_children: bool,
    expanded: bool,
    path: Vec<usize>,
    item_value: Value,
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

        let mut path = parent_path.to_vec();
        path.push(i);

        let is_expanded = has_children && (expand_all || expanded.contains(&path));
        rows.push(TreeRow {
            depth,
            label,
            has_children,
            expanded: is_expanded,
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

fn sync_state_with_external_selection(
    widget_id: u64,
    items: &Value,
    props: &HashMap<String, Value>,
    expand_all: bool,
    state: &mut TreeState,
) {
    let Some((selection_key, field, needle)) = external_selection_key(props) else {
        if state.synced_selection.is_some() {
            state.synced_selection = None;
            set_tree_state(widget_id, state.clone());
        }
        return;
    };
    if state.synced_selection.as_deref() == Some(selection_key.as_str()) {
        return;
    }

    if let Some(path) = find_item_path_by_field(items, field, &needle, &[]) {
        if !expand_all {
            state.expanded.extend(ancestor_paths(&path));
        }
        let mut rows = Vec::new();
        flatten_items(items, 0, &[], &state.expanded, expand_all, &mut rows);
        if let Some(row_idx) = rows.iter().position(|row| row.path == path) {
            state.selected_row = row_idx;
        }
    }
    state.synced_selection = Some(selection_key);
    set_tree_state(widget_id, state.clone());
}

pub(crate) fn selection_view_hint(node: &LayoutNode) -> Option<(String, usize, f32)> {
    if node.widget_type != "tree" {
        return None;
    }

    let items = get_items_from_props(&node.props);
    let expand_all = get_expand_all_prop(&node.props);
    let mut state = get_tree_state(node.widget_id);
    sync_state_with_external_selection(node.widget_id, &items, &node.props, expand_all, &mut state);
    let selection_key = state.synced_selection.clone()?;
    Some((selection_key, state.selected_row, ROW_HEIGHT))
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

// ── Widget definition ────────────────────────────────────────────────────────

pub struct TreeWidget;
pub struct TreeRowBgWidget;

pub static TREE_WIDGET: TreeWidget = TreeWidget;
pub static TREE_ROW_BG_WIDGET: TreeRowBgWidget = TreeRowBgWidget;

const INDENT_CELLS: f32 = 1.5;

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
        // widget_id isn't available during measure, so we use LAST_EXPANDED
        // (synced from set_tree_state) to get the current expanded set.
        let mut rows = Vec::new();
        let mut expanded = get_last_known_expanded(&items);
        if !expand_all
            && let Some((_, field, needle)) = external_selection_key_from_value(node)
            && let Some(path) = find_item_path_by_field(&items, field, &needle, &[])
        {
            expanded.extend(ancestor_paths(&path));
        }
        flatten_items(&items, 0, &[], &expanded, expand_all, &mut rows);

        let rh = ROW_HEIGHT;

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
        let mut expanded = get_last_known_expanded(&items);
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
            for (j, ch) in row.label.chars().enumerate() {
                let c = label_col + j as u16;
                if c >= rect.col.round() as u16 + rect.width.round() as u16 {
                    break;
                }
                buf.set(r, c, styled_cell(ch, color, None));
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
    ) -> MouseEventOutcome {
        if !matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
            return MouseEventOutcome::Consume;
        }

        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let mut state = get_tree_state(node.widget_id);
        sync_state_with_external_selection(
            node.widget_id,
            &items,
            &node.props,
            expand_all,
            &mut state,
        );
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);

        let rh = ROW_HEIGHT;

        let scroll_offset = find_parent_scroll_offset(node);
        let row_relative = local_row - node.rect.row + scroll_offset;
        let row_idx = (row_relative / rh).floor() as usize;
        if row_idx >= rows.len() {
            return MouseEventOutcome::Consume;
        }

        let row = &rows[row_idx];

        if row.has_children {
            toggle_expand(&mut state, &row.path);
            set_tree_state(node.widget_id, state);
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(make_action_value(
                "toggle",
                &row.item_value,
            )))
        } else {
            state.selected_row = row_idx;
            set_tree_state(node.widget_id, state);
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(make_action_value(
                "select",
                &row.item_value,
            )))
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let items = get_items_from_props(&node.props);
        let expand_all = get_expand_all_prop(&node.props);
        let mut state = get_tree_state(node.widget_id);
        sync_state_with_external_selection(
            node.widget_id,
            &items,
            &node.props,
            expand_all,
            &mut state,
        );
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);
        if rows.is_empty() {
            return None;
        }

        match key.code {
            KeyCode::Up => {
                state.selected_row = state.selected_row.saturating_sub(1);
                set_tree_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Down => {
                state.selected_row = (state.selected_row + 1).min(rows.len() - 1);
                set_tree_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Right => {
                let row = &rows[state.selected_row];
                if row.has_children && !row.expanded {
                    toggle_expand(&mut state, &row.path);
                    set_tree_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                } else if row.has_children && row.expanded {
                    // Move to first child
                    state.selected_row = (state.selected_row + 1).min(rows.len() - 1);
                    set_tree_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                } else {
                    None
                }
            }
            KeyCode::Left => {
                let row = &rows[state.selected_row];
                if row.has_children && row.expanded {
                    toggle_expand(&mut state, &row.path);
                    set_tree_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                } else if row.depth > 0 {
                    // Move to parent
                    let parent_path: Vec<usize> = row.path[..row.path.len() - 1].to_vec();
                    if let Some(parent_idx) = rows.iter().position(|r| r.path == parent_path) {
                        state.selected_row = parent_idx;
                        set_tree_state(node.widget_id, state);
                        Some(WidgetEvent::Custom(Value::Nil))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            KeyCode::Enter => {
                let row = &rows[state.selected_row];
                if row.has_children {
                    toggle_expand(&mut state, &row.path);
                    set_tree_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "toggle",
                        &row.item_value,
                    )))
                } else {
                    set_tree_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(make_action_value(
                        "activate",
                        &row.item_value,
                    )))
                }
            }
            _ => None,
        }
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
        let mut state = get_tree_state(node.widget_id);
        sync_state_with_external_selection(
            node.widget_id,
            &items,
            &node.props,
            expand_all,
            &mut state,
        );
        let mut rows = Vec::new();
        flatten_items(&items, 0, &[], &state.expanded, expand_all, &mut rows);

        // Update the last-known expanded state for measure() to use
        update_last_known_expanded(&state.expanded);

        let font_size = node
            .props
            .get("font-size")
            .and_then(|v| match v {
                Value::Number(n) => Some(*n as f32),
                _ => None,
            })
            .unwrap_or(DEFAULT_FONT_SIZE);

        let rh = ROW_HEIGHT;

        let bg = theme::BG();
        let default_odd = Color {
            r: bg.r + 0.04,
            g: bg.g + 0.04,
            b: bg.b + 0.04,
            a: 1.0,
        };
        let bg_odd = resolve_named_color(&node.props, "row-bg-odd", default_odd);
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

        let mut prims = Vec::new();

        for (i, row) in rows.iter().enumerate() {
            let y = node.rect.row + i as f32 * rh;

            // Row background — Finder-style: even rows transparent, odd rows
            // subtle rounded rect, selected row blue rounded rect.
            let is_selected = i == state.selected_row;
            let show_bg = is_selected || i % 2 == 1;
            if show_bg {
                let bg = if is_selected { selected_bg } else { bg_odd };
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
            let label_x = x + 2.2;
            let fg = if row.has_children { folder_fg } else { file_fg };
            let transparent = Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_y,
                    col: label_x,
                    text: row.label.clone(),
                    font_size,
                    fg,
                    bg: transparent,
                },
            ));
        }

        prims
    }
}

// ── Helpers for measure/render coordination ──────────────────────────────────

// Since measure() doesn't have widget_id, we cache the last known expanded
// set keyed by items list identity so measure can produce the correct height.

thread_local! {
    static LAST_EXPANDED: RefCell<Option<HashSet<Vec<usize>>>> =
        const { RefCell::new(None) };
}

fn get_last_known_expanded(_items: &Value) -> HashSet<Vec<usize>> {
    LAST_EXPANDED.with(|cell| cell.borrow().clone().unwrap_or_default())
}

fn update_last_known_expanded(expanded: &HashSet<Vec<usize>>) {
    LAST_EXPANDED.with(|cell| {
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

/// Fixed row height in cells. Matches what measure() uses.
const ROW_HEIGHT: f32 = 1.25;
