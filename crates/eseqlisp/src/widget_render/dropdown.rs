use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_f32_prop, plock_active, plock_color, resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, TextMeasurer, f64_to_f32, get_map,
    get_prop_num,
};
use crate::theme;
use crate::vm::Value;

use super::{
    GpuPrimitive, GpuProportionalTextPrimitive, WidgetInstance, WidgetViewport, ndc_bounds,
};
use crate::backend::Color;

// ── Constants ────────────────────────────────────────────────────────────────

const PADDING_H: f32 = super::menu_style::TEXT_PADDING_H;
const MENU_ROW_HEIGHT: f32 = super::menu_style::ROW_HEIGHT;
const MENU_PADDING_V: f32 = super::menu_style::PANEL_PADDING_V;
const CHEVRON_RIGHT_PAD: f32 = 0.35;
const TEXT_CHEVRON_GAP: f32 = 0.4;
/// Extra right-side padding in the menu when a scrollbar is visible.
const SCROLLBAR_WIDTH: f32 = 0.4;
const SCROLLBAR_MARGIN: f32 = 0.15;
/// Approximate cell width per character for proportional text width estimation.
const APPROX_CHAR_WIDTH: f32 = super::menu_style::APPROX_CHAR_WIDTH;
// Round action-menu glyphs sit optically below the midpoint when placed at
// the font baseline's mathematical center.
const ACTION_MENU_ICON_OPTICAL_OFFSET: f32 = -0.08;

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct DropdownOwnerIdentity {
    stable_widget_id: Option<u64>,
    subtree_root_id: Option<u64>,
    parent_subtree_root_id: Option<u64>,
    stable_key: Option<String>,
}

/// Editor buffer layouts reserve disjoint 100,000-ID ranges. Include that
/// namespace so identical stable widget paths in two buffers do not share menu
/// state.
const WIDGET_ID_NAMESPACE_STRIDE: u64 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DropdownStateKey {
    Stable {
        buffer_namespace: u64,
        stable_widget_id: u64,
    },
    Layout(u64),
}

#[derive(Clone, Debug, Default)]
struct DropdownState {
    /// Stable layout identity of the dropdown currently owning this state.
    /// Conditional subtree replacement can reuse fallback numeric IDs.
    owner: Option<DropdownOwnerIdentity>,
    open: bool,
    hovered_idx: Option<usize>,
    /// True between the trigger mouse-down that opens the menu and that same
    /// click's mouse-up.
    ignore_opening_mouse_up: bool,
    /// Scroll offset (in content-space rows) when the menu is taller than the viewport.
    scroll_offset: f32,
    /// Visible menu height (set at render time, used by key_event/scroll for clamping).
    visible_height: f32,
    /// Full content height (set at render time).
    content_height: f32,
    /// A `:filterable` menu's typed filter (cleared whenever it closes).
    filter: String,
    /// Height of the pinned filter row above the list (0 without one); set
    /// at render time for hit-testing.
    list_top: f32,
    /// Rows the open menu shows after filtering (set at render time).
    row_count: usize,
}

thread_local! {
    /// Dropdown state keyed by stable widget identity when available, falling
    /// back to the current layout-local widget ID.
    static STATES: RefCell<HashMap<DropdownStateKey, DropdownState>> = RefCell::new(HashMap::new());
    /// Resolves overlay/event widget IDs to their stable state keys. A newly
    /// mounted conditional subtree can receive one numeric ID for its first
    /// interaction and another after the resulting relayout.
    static STATE_KEYS_BY_WIDGET_ID: RefCell<HashMap<u64, DropdownStateKey>> = RefCell::new(HashMap::new());
}

fn state_key_for_widget_id(widget_id: u64) -> DropdownStateKey {
    STATE_KEYS_BY_WIDGET_ID.with(|keys| {
        keys.borrow()
            .get(&widget_id)
            .copied()
            .unwrap_or(DropdownStateKey::Layout(widget_id))
    })
}

fn get_state(widget_id: u64) -> DropdownState {
    let state_key = state_key_for_widget_id(widget_id);
    STATES.with(|s| s.borrow().get(&state_key).cloned().unwrap_or_default())
}

fn set_state(widget_id: u64, state: DropdownState) {
    let state_key = state_key_for_widget_id(widget_id);
    STATES.with(|s| s.borrow_mut().insert(state_key, state));
    super::bump_widget_state_generation();
}

fn owner_identity(node: &LayoutNode) -> Option<DropdownOwnerIdentity> {
    let identity = DropdownOwnerIdentity {
        stable_widget_id: node.stable_widget_id,
        subtree_root_id: node.subtree_root_id,
        parent_subtree_root_id: node.parent_subtree_root_id,
        stable_key: node.stable_key.clone(),
    };
    (identity.stable_widget_id.is_some()
        || identity.subtree_root_id.is_some()
        || identity.parent_subtree_root_id.is_some()
        || identity.stable_key.is_some())
    .then_some(identity)
}

/// Resolve state through the node's stable ownership identity. Numeric widget
/// IDs are layout-local and may be reused when one conditional subtree replaces
/// another; carrying open state across that replacement makes the first click
/// close a stale menu instead of opening the new dropdown.
fn get_state_for_node(node: &LayoutNode) -> DropdownState {
    let owner = owner_identity(node);
    let state_key = node
        .stable_widget_id
        .map(|stable_widget_id| DropdownStateKey::Stable {
            buffer_namespace: node.widget_id / WIDGET_ID_NAMESPACE_STRIDE,
            stable_widget_id,
        })
        .unwrap_or(DropdownStateKey::Layout(node.widget_id));
    STATE_KEYS_BY_WIDGET_ID.with(|keys| {
        keys.borrow_mut().insert(node.widget_id, state_key);
    });
    let mut replaced_owner = false;
    let state = STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.entry(state_key).or_default();
        if owner.is_some() && state.owner != owner {
            *state = DropdownState {
                owner,
                ..DropdownState::default()
            };
            replaced_owner = true;
        }
        state.clone()
    });
    if replaced_owner {
        super::remove_overlay(node.widget_id);
    }
    state
}

fn close_other_dropdowns(active_widget_id: u64) {
    let active_state_key = state_key_for_widget_id(active_widget_id);
    STATES.with(|s| {
        let mut changed = false;
        for (&state_key, state) in s.borrow_mut().iter_mut() {
            if state_key == active_state_key || !state.open {
                continue;
            }
            state.open = false;
            state.hovered_idx = None;
            state.scroll_offset = 0.0;
            state.filter.clear();
            changed = true;
        }
        if changed {
            super::bump_widget_state_generation();
        }
    });
}

/// Close the dropdown for a given widget_id (called when overlay is dismissed externally).
pub fn close_dropdown(widget_id: u64) {
    let state_key = state_key_for_widget_id(widget_id);
    STATES.with(|s| {
        if let Some(state) = s.borrow_mut().get_mut(&state_key) {
            state.open = false;
            state.hovered_idx = None;
            state.scroll_offset = 0.0;
            state.filter.clear();
        }
    });
}

pub fn is_dropdown_open(widget_id: u64) -> bool {
    get_state(widget_id).open
}

/// An open `:filterable` dropdown is typing into its filter, so the editor
/// routes every printable key (space included) to it before any keybinding.
pub fn filter_captures_text(node: &LayoutNode) -> bool {
    matches!(node.widget_type.as_str(), "dropdown")
        && is_filterable(&node.props)
        && get_state_for_node(node).open
}

/// Update hovered item based on mouse position in tile-local overlay space.
/// Returns true if the hover state changed.
pub fn hover_overlay(widget_id: u64, local_row: f32) -> bool {
    let state_key = state_key_for_widget_id(widget_id);
    STATES.with(|s| {
        let mut states = s.borrow_mut();
        let Some(state) = states.get_mut(&state_key) else {
            return false;
        };
        if !state.open {
            return false;
        }

        let overlay_rect = super::overlay_rect_for_widget(widget_id);
        let menu_row = if let Some(rect) = overlay_rect {
            let r = local_row - rect.row;
            if r >= 0.0 && r < rect.height { r } else { -1.0 }
        } else {
            return false;
        };

        let new_idx = if menu_row >= state.list_top + MENU_PADDING_V {
            let idx = list_row_index(menu_row, state.list_top, state.scroll_offset);
            if idx >= 0 && (idx as usize) < state.row_count {
                Some(idx as usize)
            } else {
                state.hovered_idx
            }
        } else {
            state.hovered_idx
        };

        if new_idx != state.hovered_idx {
            state.hovered_idx = new_idx;
            super::bump_widget_state_generation();
            true
        } else {
            false
        }
    })
}

/// Scroll the open dropdown overlay by `delta_y` (trackpad pixel delta).
/// Returns true if scroll was consumed.
pub fn scroll_overlay(widget_id: u64, delta_y: f32) -> bool {
    let state_key = state_key_for_widget_id(widget_id);
    STATES.with(|s| {
        let mut states = s.borrow_mut();
        let Some(state) = states.get_mut(&state_key) else {
            return false;
        };
        if !state.open || state.content_height <= state.visible_height {
            return false;
        }

        let max_scroll = (state.content_height - state.visible_height).max(0.0);
        let scroll_speed = 0.05;
        state.scroll_offset = (state.scroll_offset - delta_y * scroll_speed).clamp(0.0, max_scroll);
        super::bump_widget_state_generation();
        true
    })
}

fn get_options(props: &HashMap<String, Value>) -> Vec<String> {
    match props.get("options") {
        Some(Value::List(list)) => list
            .iter()
            .map(|v| match &*v.borrow() {
                Value::String(s) => s.clone(),
                Value::Keyword(k) => k.clone(),
                other => crate::vm::format_lisp_value(other),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Option indices that are section headers (`:headers '(3)`): drawn dimmed
/// with no check mark, and never hovered, picked or announced by
/// `:on-change`, so a menu can group its options ("Library" above library
/// entries) without a header row selecting anything.
fn get_header_indices(props: &HashMap<String, Value>) -> Vec<usize> {
    match props.get("headers") {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|v| match &*v.borrow() {
                Value::Number(n) if *n >= 0.0 => Some(n.round() as usize),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// One row of the open menu: an option (by index into `:options`), a
/// section header, or the `:footer` action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuRow {
    Option(usize),
    Header(usize),
    Footer,
}

/// `:filterable true` pins a filter field above the list: typing while the
/// menu is open narrows it.
fn is_filterable(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("filterable"), Some(Value::Bool(true)))
}

fn string_prop(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    match props.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// `:footer "Extract…"`: an action row under the options; picking it calls
/// `:on-change` with the footer's own text. Never filtered away.
fn get_footer(props: &HashMap<String, Value>) -> Option<String> {
    string_prop(props, "footer")
}

/// `:details`: dim text drawn right-aligned on each option's row, parallel
/// to `:options` ("" for none).
fn get_details(props: &HashMap<String, Value>) -> Vec<String> {
    match props.get("details") {
        Some(Value::List(list)) => list
            .iter()
            .map(|v| match &*v.borrow() {
                Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Height of the filter row pinned above a `:filterable` list.
const FILTER_ROW_HEIGHT: f32 = MENU_ROW_HEIGHT + 0.35;

fn list_top(props: &HashMap<String, Value>) -> f32 {
    if is_filterable(props) { FILTER_ROW_HEIGHT } else { 0.0 }
}

/// The rows an open menu shows: every option and header, or, with a filter,
/// the options containing it (case-insensitive) and the headers that still
/// have a match under them; then the footer.
fn menu_rows(props: &HashMap<String, Value>, filter: &str) -> Vec<MenuRow> {
    let options = get_options(props);
    let headers = get_header_indices(props);
    let needle = filter.trim().to_lowercase();
    let mut rows = Vec::with_capacity(options.len() + 1);
    let mut pending_header = None;
    for (i, option) in options.iter().enumerate() {
        if headers.contains(&i) {
            if needle.is_empty() {
                rows.push(MenuRow::Header(i));
            } else {
                pending_header = Some(i);
            }
            continue;
        }
        if needle.is_empty() || option.to_lowercase().contains(&needle) {
            if let Some(header) = pending_header.take() {
                rows.push(MenuRow::Header(header));
            }
            rows.push(MenuRow::Option(i));
        }
    }
    if get_footer(props).is_some() {
        rows.push(MenuRow::Footer);
    }
    rows
}

/// What picking `row` reports through `:on-change` (headers pick nothing).
fn row_value(props: &HashMap<String, Value>, row: MenuRow) -> Option<String> {
    match row {
        MenuRow::Option(i) => get_options(props).get(i).cloned(),
        MenuRow::Footer => get_footer(props),
        MenuRow::Header(_) => None,
    }
}

/// The nearest pickable row from `from`, stepping `forward` (or back).
fn step_row(rows: &[MenuRow], from: Option<usize>, forward: bool) -> Option<usize> {
    let pickable = |i: usize| !matches!(rows[i], MenuRow::Header(_));
    match from {
        None => (0..rows.len()).find(|&i| pickable(i)),
        Some(start) => {
            let mut idx = start;
            loop {
                if forward {
                    if idx + 1 >= rows.len() {
                        return Some(start);
                    }
                    idx += 1;
                } else {
                    if idx == 0 {
                        return Some(start);
                    }
                    idx -= 1;
                }
                if pickable(idx) {
                    return Some(idx);
                }
            }
        }
    }
}

/// The list row under `menu_row` (rows from the menu's top), below the
/// pinned filter row and shifted by the list's scroll.
fn list_row_index(menu_row: f32, list_top: f32, scroll: f32) -> isize {
    ((menu_row - list_top + scroll - MENU_PADDING_V) / MENU_ROW_HEIGHT).floor() as isize
}

/// The row index of the selected option in `rows`, if it is shown.
fn selected_row(props: &HashMap<String, Value>, rows: &[MenuRow]) -> Option<usize> {
    let options = get_options(props);
    let selected = selected_index(&options, &get_selected(props))?;
    rows.iter().position(|row| *row == MenuRow::Option(selected))
}

/// The nearest selectable option from `from` stepping `forward` (or back),
/// skipping headers; `from` itself when every option past it is a header.
fn step_selectable(
    option_count: usize,
    headers: &[usize],
    from: Option<usize>,
    forward: bool,
) -> Option<usize> {
    if option_count == 0 {
        return None;
    }
    let mut idx = match from {
        None => {
            return (0..option_count).find(|i| !headers.contains(i));
        }
        Some(i) => i,
    };
    loop {
        if forward {
            if idx + 1 >= option_count {
                return from;
            }
            idx += 1;
        } else {
            if idx == 0 {
                return from;
            }
            idx -= 1;
        }
        if !headers.contains(&idx) {
            return Some(idx);
        }
    }
}

fn get_numeric_prop(props: &HashMap<String, Value>, key: &str) -> Option<f64> {
    match props.get(key) {
        Some(Value::Number(value)) => Some(*value),
        Some(Value::Bool(true)) => Some(1.0),
        Some(Value::Bool(false)) => Some(0.0),
        Some(Value::ReactiveRef { slot, .. }) => Some(crate::reactive::read_float_slot(slot)),
        _ => None,
    }
}

fn get_selected_from_index(props: &HashMap<String, Value>) -> Option<String> {
    let options = get_options(props);
    if options.is_empty() {
        return None;
    }
    let value = get_numeric_prop(props, "value-index")?;
    let offset = get_numeric_prop(props, "value-index-offset").unwrap_or(0.0);
    let idx = (value - offset).round() as isize;
    let idx = idx.clamp(0, options.len().saturating_sub(1) as isize) as usize;
    options.get(idx).cloned()
}

fn get_selected(props: &HashMap<String, Value>) -> String {
    if let Some(selected) = get_selected_from_index(props) {
        return selected;
    }
    match props.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Keyword(k)) => k.clone(),
        Some(other) => crate::vm::format_lisp_value(other),
        None => String::new(),
    }
}

fn is_action_menu(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("action-menu"), Some(Value::Bool(true)))
}

fn action_menu_icon(props: &HashMap<String, Value>) -> String {
    match props.get("icon") {
        Some(Value::String(icon)) => icon.clone(),
        Some(Value::Keyword(icon)) => icon.clone(),
        _ => "•••".to_string(),
    }
}

fn trigger_text_row(props: &HashMap<String, Value>, rect: Rect) -> f32 {
    let centered = rect.row + (rect.height - 1.0) * 0.5;
    if is_action_menu(props) {
        centered + ACTION_MENU_ICON_OPTICAL_OFFSET
    } else {
        centered
    }
}

fn props_from_node(node: &Value) -> HashMap<String, Value> {
    let Some(map) = get_map(node) else {
        return HashMap::new();
    };
    map.into_iter()
        .filter(|(key, _)| key != "type" && key != "children")
        .collect()
}

fn text_width_cells(
    text: &str, font_size: f32, cell_w: f32, measurer: Option<&dyn TextMeasurer>,
) -> f32 {
    match measurer {
        Some(measurer) if cell_w > 0.0 => measurer.measure_text_px(text, font_size) / cell_w,
        _ => text.chars().count() as f32 * APPROX_CHAR_WIDTH * (font_size / DEFAULT_FONT_SIZE),
    }
}

fn render_text_width_cells(text: &str, font_size: f32, cell_w: f32) -> f32 {
    // Fixed-size parents and retained layouts can skip intrinsic measurement.
    // Use the current render font and cell width, never measure-pass side effects
    // or cell widths cached under an earlier display scale.
    super::with_render_text_measurer(|measurer| {
        text_width_cells(text, font_size, cell_w, Some(measurer))
    }).unwrap_or_else(|| text_width_cells(text, font_size, cell_w, None))
}

fn truncate_text_to_width(text: &str, max_width: f32, font_size: f32, cell_w: f32) -> String {
    if max_width <= 0.0 || text.is_empty() {
        return String::new();
    }

    if render_text_width_cells(text, font_size, cell_w) <= max_width {
        return text.to_string();
    }

    let mut acc = 0.0;
    let mut out = String::new();

    for ch in text.chars() {
        let mut utf8 = [0; 4];
        let width = render_text_width_cells(ch.encode_utf8(&mut utf8), font_size, cell_w);
        if acc + width > max_width {
            break;
        }
        out.push(ch);
        acc += width;
    }

    out
}

fn selected_index(options: &[String], selected: &str) -> Option<usize> {
    options.iter().position(|o| o == selected)
}

fn initial_mouse_hovered_index(props: &HashMap<String, Value>) -> Option<usize> {
    if is_action_menu(props) {
        None
    } else {
        selected_row(props, &menu_rows(props, ""))
    }
}

fn initial_keyboard_hovered_index(props: &HashMap<String, Value>) -> Option<usize> {
    let rows = menu_rows(props, "");
    if is_action_menu(props) {
        step_row(&rows, None, true)
    } else {
        selected_row(props, &rows)
    }
}

/// Computed menu placement and sizing.
struct MenuGeometry {
    /// Top of the visible menu in layout-space rows.
    menu_top: f32,
    /// Full content height (all items + padding).
    content_height: f32,
    /// Visible/clamped menu height (may be smaller than content_height).
    visible_height: f32,
    /// Maximum scroll offset.
    max_scroll: f32,
}

/// Compute menu placement relative to trigger, clamping to the frame-level
/// overlay viewport.
/// When the menu doesn't fit below or above, it extends to fill the full
/// viewport height (covering the trigger), matching native macOS behavior.
fn compute_menu_geometry(
    trigger_row: f32,
    trigger_height: f32,
    option_count: usize,
    viewport_top: f32,
    viewport_bottom: f32,
) -> MenuGeometry {
    compute_menu_geometry_with_top(
        trigger_row,
        trigger_height,
        option_count,
        0.0,
        viewport_top,
        viewport_bottom,
    )
}

/// [`compute_menu_geometry`] with a pinned filter row of `list_top` rows
/// above the list.
fn compute_menu_geometry_with_top(
    trigger_row: f32,
    trigger_height: f32,
    option_count: usize,
    list_top: f32,
    viewport_top: f32,
    viewport_bottom: f32,
) -> MenuGeometry {
    let content_height =
        list_top + option_count as f32 * MENU_ROW_HEIGHT + MENU_PADDING_V * 2.0;
    let gap = 0.15;
    // Reserve space for the border so it isn't clipped by the frame edge.
    let border_inset = 0.1;
    let below_top = trigger_row + trigger_height + gap;

    let (menu_top, visible_height) = if below_top + content_height + border_inset <= viewport_bottom
    {
        // Fits below trigger
        (below_top, content_height)
    } else if trigger_row - content_height - gap >= viewport_top + border_inset {
        // Fits above trigger
        (trigger_row - content_height - gap, content_height)
    } else {
        // Doesn't fit either way — fill viewport minus border insets
        let viewport_height = (viewport_bottom - viewport_top).max(0.0);
        let h = (viewport_height - border_inset * 2.0)
            .max(0.0)
            .min(content_height);
        (viewport_top + border_inset, h)
    };

    let max_scroll = (content_height - visible_height).max(0.0);

    MenuGeometry {
        menu_top,
        content_height,
        visible_height,
        max_scroll,
    }
}

/// Ensure scroll offset keeps `hovered_idx` visible within the menu viewport.
fn ensure_visible(state: &mut DropdownState, option_count: usize) {
    let Some(idx) = state.hovered_idx else { return };
    if state.visible_height <= 0.0 {
        return;
    }
    let content_height =
        state.list_top + option_count as f32 * MENU_ROW_HEIGHT + MENU_PADDING_V * 2.0;
    let max_scroll = (content_height - state.visible_height).max(0.0);
    // In list space: the pinned filter row takes `list_top` of the window.
    let item_top = MENU_PADDING_V + idx as f32 * MENU_ROW_HEIGHT;
    let item_bottom = item_top + MENU_ROW_HEIGHT;
    let window = state.visible_height - state.list_top;

    if item_top < state.scroll_offset {
        state.scroll_offset = item_top;
    } else if item_bottom > state.scroll_offset + window {
        state.scroll_offset = item_bottom - window;
    }
    state.scroll_offset = state.scroll_offset.clamp(0.0, max_scroll);
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct DropdownWidget;
pub static DROPDOWN_WIDGET: DropdownWidget = DropdownWidget;

impl WidgetDefinition for DropdownWidget {
    fn names(&self) -> &'static [&'static str] {
        &[
            "dropdown",
            "menu-button",
            "dropdown-chevron",
            "dropdown-checkmark",
            "dropdown-magnifier",
        ]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "options",
            "headers",
            "value",
            "value-index",
            "value-index-offset",
            "width",
            "height",
            "font-size",
            "icon",
            "detail",
        ]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "value",
            "value-index",
            "value-index-offset",
            "plock-active",
            "plock-color-r",
            "plock-color-g",
            "plock-color-b",
        ]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "options", "headers", "value", "value-index", "value-index-offset", "width",
            "height", "font-size", "icon", "focusable", "action-menu", "badge-color", "bg-color",
            "border-color", "border-width", "check-color", "chevron-color", "corner-radius", "hover-bg",
            "menu-bg", "menu-border-color", "ring-color", "scrollbar-color", "text-color",
            "on-change", "plock-active", "plock-color-r", "plock-color-g", "plock-color-b",
            "detail", "details", "filterable", "filter-placeholder", "footer", "footer-color",
        ]
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let props = props_from_node(node);
        let action_menu = is_action_menu(&props);
        let selected = get_selected(&props);
        let options = get_options(&props);
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(if action_menu { 1.1 } else { 1.5 });
        let explicit_width = get_prop_num(node, "width").map(f64_to_f32);
        let width = explicit_width.unwrap_or(if action_menu { 2.2 } else { 10.0 });
        if action_menu {
            return Some(Size { width, height });
        }
        let text_width = |text: &str| text_width_cells(text, font_size, ctx.cell_w, ctx.text_measurer);
        let selected_width = if props.contains_key("value-index") {
            options
                .iter()
                .map(|option| text_width(option))
                .fold(text_width(&selected), f32::max)
        } else {
            text_width(&selected)
        };
        let chevron_width = height * 0.48 * 1.8;
        let detail_width = string_prop(&props, "detail")
            .map(|detail| text_width(&detail) + TEXT_CHEVRON_GAP)
            .unwrap_or(0.0);
        let min_width = PADDING_H
            + selected_width
            + TEXT_CHEVRON_GAP
            + detail_width
            + chevron_width
            + CHEVRON_RIGHT_PAD;
        Some(Size {
            width: explicit_width.unwrap_or_else(|| width.max(min_width)),
            height,
        })
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
        if !matches!(
            mouse_kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::Up(MouseButton::Left)
        ) {
            return MouseEventOutcome::Consume;
        }

        let mut state = get_state_for_node(node);

        if state.open {
            if matches!(mouse_kind, MouseEventKind::Up(MouseButton::Left))
                && state.ignore_opening_mouse_up
            {
                state.ignore_opening_mouse_up = false;
                set_state(node.widget_id, state);
                return MouseEventOutcome::Consume;
            }
            let rows = menu_rows(&node.props, &state.filter);
            // Use the overlay rect (registered at render time) for hit-testing.
            // The overlay rect is in screen-space; local_row is in layout-space.
            let overlay_rect = super::overlay_rect_for_widget(node.widget_id);
            let menu_row = if let Some(rect) = overlay_rect {
                let r = local_row - rect.row;
                if r >= 0.0 && r < rect.height { r } else { -1.0 }
            } else {
                -1.0
            };

            if menu_row >= 0.0 && menu_row < state.list_top + MENU_PADDING_V {
                // The pinned filter row: typing edits it, a click keeps the
                // menu open.
                return MouseEventOutcome::Consume;
            }
            if menu_row >= 0.0 {
                let item_idx = list_row_index(menu_row, state.list_top, state.scroll_offset);
                if item_idx >= 0 && (item_idx as usize) < rows.len() {
                    let item_idx = item_idx as usize;
                    let Some(value) = row_value(&node.props, rows[item_idx]) else {
                        // A section header is not an option: the menu stays open.
                        return MouseEventOutcome::Consume;
                    };
                    state.hovered_idx = Some(item_idx);
                    set_state(node.widget_id, state);
                    if matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
                        let mut state = get_state(node.widget_id);
                        state.open = false;
                        state.hovered_idx = None;
                        state.ignore_opening_mouse_up = false;
                        state.scroll_offset = 0.0;
                        state.filter.clear();
                        set_state(node.widget_id, state);
                        super::remove_overlay(node.widget_id);
                        return MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::String(
                            value,
                        )));
                    }
                    return MouseEventOutcome::Consume;
                }
            }

            if matches!(mouse_kind, MouseEventKind::Up(MouseButton::Left)) {
                state.open = false;
                state.hovered_idx = None;
                state.ignore_opening_mouse_up = false;
                state.scroll_offset = 0.0;
                state.filter.clear();
                set_state(node.widget_id, state);
                super::remove_overlay(node.widget_id);
            }
            MouseEventOutcome::Consume
        } else {
            if !matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
                return MouseEventOutcome::Consume;
            }
            // Open the dropdown
            close_other_dropdowns(node.widget_id);
            state.open = true;
            state.filter.clear();
            state.hovered_idx = initial_mouse_hovered_index(&node.props);
            state.ignore_opening_mouse_up = true;
            state.scroll_offset = 0.0;
            set_state(node.widget_id, state);
            MouseEventOutcome::Consume
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let mut state = get_state_for_node(node);
        if get_options(&node.props).is_empty() && get_footer(&node.props).is_none() {
            return None;
        }

        if !state.open {
            // When closed: only Enter opens the menu.
            // Up/Down are NOT consumed — they fall through to focus navigation.
            match key.code {
                KeyCode::Enter => {
                    close_other_dropdowns(node.widget_id);
                    state.open = true;
                    state.filter.clear();
                    state.hovered_idx = initial_keyboard_hovered_index(&node.props);
                    set_state(node.widget_id, state);
                    return Some(WidgetEvent::Custom(Value::Nil));
                }
                _ => return None,
            }
        }

        // Menu is open
        let close = |mut state: DropdownState| {
            state.open = false;
            state.hovered_idx = None;
            state.scroll_offset = 0.0;
            state.filter.clear();
            set_state(node.widget_id, state);
            super::remove_overlay(node.widget_id);
        };
        let rows = menu_rows(&node.props, &state.filter);
        let typing = is_filterable(&node.props)
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT);
        match key.code {
            // The filter: every printable key narrows the list and hovers
            // its first match, so Enter picks it.
            KeyCode::Char(ch) if typing => {
                state.filter.push(ch);
                let rows = menu_rows(&node.props, &state.filter);
                state.hovered_idx = step_row(&rows, None, true);
                state.scroll_offset = 0.0;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Backspace if typing => {
                state.filter.pop();
                let rows = menu_rows(&node.props, &state.filter);
                state.hovered_idx = step_row(&rows, None, true);
                state.scroll_offset = 0.0;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Down => {
                state.hovered_idx = step_row(&rows, state.hovered_idx, true);
                ensure_visible(&mut state, rows.len());
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Up => {
                state.hovered_idx = match state.hovered_idx {
                    None => step_row(&rows, None, true),
                    from => step_row(&rows, from, false),
                };
                ensure_visible(&mut state, rows.len());
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Enter => {
                let picked = state
                    .hovered_idx
                    .and_then(|idx| rows.get(idx).copied())
                    .map(|row| row_value(&node.props, row));
                match picked {
                    // Enter on a header (reached by the mouse) picks nothing.
                    Some(None) => Some(WidgetEvent::Custom(Value::Nil)),
                    Some(Some(value)) => {
                        close(state);
                        Some(WidgetEvent::Custom(Value::String(value)))
                    }
                    None => {
                        close(state);
                        Some(WidgetEvent::Custom(Value::Nil))
                    }
                }
            }
            KeyCode::Esc => {
                close(state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            _ => None,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(ref value) = event else {
            return None;
        };
        if matches!(value, Value::Nil) {
            return None;
        }
        let Value::String(new_value) = value else {
            return None;
        };
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput {
            callback,
            args: vec![Value::String(new_value.clone())],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let text = if is_action_menu(props) {
            action_menu_icon(props)
        } else {
            format!("{} ▾", get_selected(props))
        };
        let fg = resolve_named_color(props, "text-color", theme::DROPDOWN_FG());
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16 + 1;
        let max_col = col_start + rect.width.round() as u16;
        for (i, ch) in text.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= max_col {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    fn fragment_shader(
        &self,
        widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        match widget_type {
            "dropdown" | "menu-button" => super::ROUNDED_RECT_SHADER.source(backend),
            "dropdown-chevron" => DROPDOWN_CHEVRON_SHADER.source(backend),
            "dropdown-checkmark" => DROPDOWN_CHECKMARK_SHADER.source(backend),
            "dropdown-magnifier" => DROPDOWN_MAGNIFIER_SHADER.source(backend),
            _ => None,
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let selected = get_selected(&node.props);
        let options = get_options(&node.props);
        let action_menu = is_action_menu(&node.props);
        let mut state = get_state_for_node(node);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);

        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let menu_font_size = super::menu_style::menu_font_size_from_props(&node.props);

        let bg_color = resolve_named_color(&node.props, "bg-color", theme::DROPDOWN_BG());
        let plocked = plock_active(&node.props);
        let plock_color = plock_color(&node.props);
        let text_color = if plocked {
            plock_color
        } else {
            resolve_named_color(&node.props, "text-color", theme::DROPDOWN_FG())
        };
        let ring_color = resolve_named_color(&node.props, "ring-color", theme::DROPDOWN_RING());
        let chevron_color =
            resolve_named_color(&node.props, "chevron-color", theme::DROPDOWN_CHEVRON());
        let menu_bg = resolve_named_color(&node.props, "menu-bg", theme::DROPDOWN_MENU_BG());
        let hover_bg = resolve_named_color(&node.props, "hover-bg", theme::DROPDOWN_HOVER_BG());
        let check_color = resolve_named_color(&node.props, "check-color", theme::DROPDOWN_CHECK());

        let transparent = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
        let mut prims = Vec::new();

        // `:corner-radius` in design pixels, like `box`. Absent keeps the
        // shader's historical pill default; 0 is square.
        let corner_radius_px = match node.props.get("corner-radius") {
            Some(Value::Number(n)) => Some((*n as f32).max(0.0)),
            _ => None,
        };
        let radius_for = |rect: Rect| {
            corner_radius_px
                .map(|px| normalized_corner_radius(rect, viewport, px))
                .unwrap_or(0.0)
        };

        // ── Focus ring ──
        if is_focused && (!action_menu || !state.open) {
            let ring_v = 0.15_f32;
            let ring_h = ring_v * viewport.cell_h / viewport.cell_w;
            let ring_rect = Rect {
                row: node.rect.row - ring_v,
                col: node.rect.col - ring_h,
                width: node.rect.width + ring_h * 2.0,
                height: node.rect.height + ring_v * 2.0,
            };
            emit_rounded_rect(&mut prims, ring_rect, ring_color, viewport, true, radius_for(ring_rect));
        }

        // ── Border (only when border-color is set; default is no border) ──
        if plocked || node.props.contains_key("border-color") {
            let border_color = if plocked {
                plock_color
            } else {
                resolve_named_color(&node.props, "border-color", theme::DROPDOWN_RING())
            };
            let bw_v = get_f32_prop(
                &node.props,
                "border-width",
                if plocked { 0.10 } else { 0.08 },
            );
            let bw_h = bw_v * viewport.cell_h / viewport.cell_w;
            let border_rect = Rect {
                row: node.rect.row - bw_v,
                col: node.rect.col - bw_h,
                width: node.rect.width + bw_h * 2.0,
                height: node.rect.height + bw_v * 2.0,
            };
            emit_rounded_rect(&mut prims, border_rect, border_color, viewport, true, radius_for(border_rect));
        }

        // ── Background ──
        emit_rounded_rect(&mut prims, node.rect, bg_color, viewport, true, radius_for(node.rect));

        let ch_h = node.rect.height * 0.48;
        let ch_w = ch_h * 1.8;
        let ch_col = node.rect.col + node.rect.width - CHEVRON_RIGHT_PAD - ch_w;
        let ch_rect = Rect {
            row: node.rect.row + (node.rect.height - ch_h) * 0.5,
            col: ch_col,
            width: ch_w,
            height: ch_h,
        };

        let text_row = trigger_text_row(&node.props, node.rect);
        if action_menu {
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
                    row: text_row,
                    col: node.rect.col,
                    align_width: node.rect.width,
                    h_align: 0.5,
                    text: action_menu_icon(&node.props),
                    font_size,
                    scale: 1.0,
                    fg: text_color,
                    bg: transparent,
                    mono: false,
                },
            ));
        } else {
            // ── Detail (`:detail`): dim text just before the chevron ──
            let text_col = node.rect.col + PADDING_H;
            let mut text_right = ch_rect.col - TEXT_CHEVRON_GAP;
            if let Some(detail) = string_prop(&node.props, "detail") {
                let room = (text_right - text_col) * 0.55;
                let detail = truncate_text_to_width(&detail, room, font_size * 0.9, viewport.cell_w);
                let width = render_text_width_cells(&detail, font_size * 0.9, viewport.cell_w);
                if !detail.is_empty() {
                    prims.push(GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
                        row: text_row,
                        col: text_right - width,
                        align_width: 0.0,
                        h_align: 0.0,
                        text: detail,
                        font_size: font_size * 0.9,
                        scale: 1.0,
                        fg: theme::DIM(),
                        bg: transparent,
                        mono: false,
                    }));
                    text_right -= width + TEXT_CHEVRON_GAP;
                }
            }

            // ── Selected text ──
            let text_clip_rect = Rect {
                row: node.rect.row + 0.08,
                col: text_col,
                width: (text_right - text_col).max(0.0),
                height: (node.rect.height - 0.16).max(0.0),
            };
            let selected_display =
                truncate_text_to_width(&selected, text_clip_rect.width, font_size, viewport.cell_w);
            if !selected_display.is_empty() && text_clip_rect.width > 0.0 {
                prims.push(GpuPrimitive::PushClipRect(text_clip_rect));
                prims.push(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row: text_row,
                        col: text_col,
                        align_width: 0.0,
                        h_align: 0.0,
                        text: selected_display,
                        font_size,
                        scale: 1.0,
                        fg: text_color,
                        bg: transparent,
                        mono: false,
                    },
                ));
                prims.push(GpuPrimitive::PopClipRect);
            }

            // ── Chevron badge + arrows ──
            // Badge background behind chevrons
            let badge_color =
                resolve_named_color(&node.props, "badge-color", theme::DROPDOWN_BADGE_BG());
            let badge_pad = 0.1;
            let badge_rect = Rect {
                row: ch_rect.row - badge_pad,
                col: ch_rect.col - badge_pad * 0.5,
                width: ch_rect.width + badge_pad,
                height: ch_rect.height + badge_pad * 2.0,
            };
            emit_rounded_rect(&mut prims, badge_rect, badge_color, viewport, false, 0.4);
            let (ndc_min, ndc_max) = ndc_bounds(ch_rect, viewport);
            let px_w = ch_rect.width * viewport.cell_w;
            let px_h = ch_rect.height * viewport.cell_h;
            prims.push(GpuPrimitive::WidgetInstance {
                widget_type: "dropdown-chevron".to_string(),
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
                    color_a: [
                        chevron_color.r,
                        chevron_color.g,
                        chevron_color.b,
                        chevron_color.a,
                    ],
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: false,
            });
        }

        // ── Menu overlay (when open) ──
        let rows = menu_rows(&node.props, &state.filter);
        let filterable = is_filterable(&node.props);
        if state.open && (!rows.is_empty() || filterable) {
            let screen_col = node.rect.col - viewport.scroll_left;
            let screen_row = node.rect.row - viewport.scroll_top;
            let viewport_rows = viewport.vp_h / viewport.cell_h.max(1.0);
            let viewport_bottom = viewport.overlay_viewport_bottom;
            let viewport_top = viewport_bottom - viewport_rows;
            let list_top = list_top(&node.props);

            let geo = compute_menu_geometry_with_top(
                screen_row,
                node.rect.height,
                rows.len(),
                list_top,
                viewport_top,
                viewport_bottom,
            );

            // Persist geometry so key_event/scroll/hover can operate correctly
            state.visible_height = geo.visible_height;
            state.content_height = geo.content_height;
            state.list_top = list_top;
            state.row_count = rows.len();
            state.scroll_offset = state.scroll_offset.clamp(0.0, geo.max_scroll);
            // Ensure the hovered item is visible (e.g. when opening with a far-down selection)
            ensure_visible(&mut state, rows.len());
            set_state(node.widget_id, state.clone());

            // Menu width: at least trigger width, expanded to fit longest row
            let needs_scrollbar = geo.content_height > geo.visible_height;
            let check_col_width = if action_menu { 0.0 } else { 1.5 }; // selected-item mark
            let text_left_pad = PADDING_H + check_col_width;
            let scrollbar_pad = if needs_scrollbar {
                SCROLLBAR_WIDTH + SCROLLBAR_MARGIN * 2.0
            } else {
                0.0
            };
            let details = get_details(&node.props);
            let footer = get_footer(&node.props);
            let detail_font_size = menu_font_size * 0.88;
            let text_w = |text: &str, size: f32| render_text_width_cells(text, size, viewport.cell_w);
            let max_row_width = options
                .iter()
                .enumerate()
                .map(|(i, option)| {
                    let detail = details.get(i).map_or(0.0, |detail| {
                        if detail.is_empty() { 0.0 } else { 2.0 + text_w(detail, detail_font_size) }
                    });
                    text_w(option, menu_font_size) + detail
                })
                .chain(footer.iter().map(|footer| text_w(footer, menu_font_size)))
                .fold(0.0_f32, f32::max);
            let content_width = text_left_pad + max_row_width + PADDING_H + scrollbar_pad;
            let menu_width = content_width.max(node.rect.width);

            let viewport_cols = viewport.vp_w / viewport.cell_w.max(1.0);
            let menu_col = if action_menu {
                (screen_col + node.rect.width - menu_width)
                    .max(0.0)
                    .min((viewport_cols - menu_width).max(0.0))
            } else {
                screen_col
            };
            let menu_rect = Rect {
                row: geo.menu_top,
                col: menu_col,
                width: menu_width,
                height: geo.visible_height,
            };

            // Register overlay for hit-testing (visible rect only)
            super::set_overlay(node.widget_id, menu_rect);

            let border_color = resolve_named_color(
                &node.props,
                "menu-border-color",
                theme::DROPDOWN_MENU_BORDER(),
            );
            super::menu_style::emit_panel_chrome(
                menu_rect,
                menu_bg,
                border_color,
                viewport,
            );

            // ── Filter row (pinned above the list) ──
            if filterable {
                let field = Rect {
                    row: geo.menu_top + MENU_PADDING_V + 0.12,
                    col: menu_col + 0.35,
                    width: (menu_width - 0.7).max(0.0),
                    height: MENU_ROW_HEIGHT,
                };
                super::menu_style::emit_rounded_rect_overlay(
                    field,
                    Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 },
                    12.0,
                    viewport,
                );
                let icon_col = field.col + 0.55;
                super::push_overlay_primitive(super::menu_style::magnifier_primitive(
                    field,
                    icon_col,
                    menu_font_size,
                    theme::DIM(),
                    viewport,
                ));
                let text_col = icon_col + 1.4;
                let placeholder = string_prop(&node.props, "filter-placeholder")
                    .unwrap_or_else(|| "Filter…".to_string());
                let (text, fg) = if state.filter.is_empty() {
                    (placeholder, theme::DIM())
                } else {
                    (state.filter.clone(), theme::FG())
                };
                let text_row = field.row + (field.height - 1.0) * 0.5;
                super::push_overlay_primitive(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row: text_row,
                        col: text_col,
                        align_width: 0.0,
                        h_align: 0.0,
                        text,
                        font_size: menu_font_size,
                        scale: 1.0,
                        fg,
                        bg: transparent,
                        mono: false,
                    },
                ));
                // Caret after the typed text.
                let caret_col = text_col
                    + if state.filter.is_empty() { 0.0 } else { text_w(&state.filter, menu_font_size) }
                    + 0.08;
                super::menu_style::emit_rounded_rect_overlay(
                    Rect { row: field.row + 0.25, col: caret_col, width: 0.09, height: field.height - 0.5 },
                    theme::FG(),
                    1.0,
                    viewport,
                );
            }

            let list_rect = Rect {
                row: geo.menu_top + list_top,
                col: menu_col,
                width: menu_width,
                height: (geo.visible_height - list_top).max(0.0),
            };
            super::push_overlay_primitive(GpuPrimitive::PushClipRect(list_rect));

            // Rows — only emit those within the visible scroll window
            let sel_idx = selected_index(&options, &selected);
            let scroll_off = state.scroll_offset;
            let label_col = menu_col + PADDING_H;
            let item_text_col = label_col + check_col_width;
            let item_right = menu_col + menu_width - PADDING_H - scrollbar_pad;
            let footer_color = resolve_named_color(
                &node.props,
                "footer-color",
                Color { r: 0.45, g: 0.63, b: 1.0, a: 1.0 },
            );
            for (i, row) in rows.iter().enumerate() {
                let content_y = MENU_PADDING_V + i as f32 * MENU_ROW_HEIGHT;
                let visible_y = content_y - scroll_off;
                if visible_y + MENU_ROW_HEIGHT < 0.0 || visible_y >= list_rect.height {
                    continue;
                }
                let item_y = list_rect.row + visible_y;
                let row_rect = Rect { row: item_y, col: menu_col, width: menu_width, height: MENU_ROW_HEIGHT };
                let is_header = matches!(row, MenuRow::Header(_));
                let is_footer = matches!(row, MenuRow::Footer);
                let is_hovered = state.hovered_idx == Some(i) && !is_header;
                if is_footer {
                    // A hairline separates the action from the options.
                    super::push_overlay_primitive(GpuPrimitive::Rect(super::GpuRectPrimitive {
                        rect: Rect { row: item_y, col: menu_col + 0.35, width: menu_width - 0.7, height: 0.04 },
                        color: Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 },
                    }));
                }
                if is_hovered {
                    super::menu_style::emit_row_highlight(row_rect, hover_bg, viewport);
                }

                let (text, text_col, fg) = match *row {
                    MenuRow::Option(idx) => {
                        if !action_menu && sel_idx == Some(idx) {
                            super::push_overlay_primitive(super::menu_style::checkmark_primitive(
                                row_rect,
                                menu_font_size,
                                check_color,
                                viewport,
                            ));
                        }
                        // Detail, right-aligned and dim (white on the hover).
                        if let Some(detail) = details.get(idx).filter(|d| !d.is_empty()) {
                            super::push_overlay_primitive(GpuPrimitive::ProportionalText(
                                GpuProportionalTextPrimitive {
                                    row: item_y + (MENU_ROW_HEIGHT - 1.0) * 0.5,
                                    col: item_text_col,
                                    align_width: (item_right - item_text_col).max(0.0),
                                    h_align: 1.0,
                                    text: detail.clone(),
                                    font_size: detail_font_size,
                                    scale: 1.0,
                                    fg: if is_hovered { theme::FG() } else { theme::DIM() },
                                    bg: transparent,
                                    mono: false,
                                },
                            ));
                        }
                        (options[idx].clone(), item_text_col, theme::FG())
                    }
                    // A header starts at the check column, dimmed, so the
                    // options under it read as its members.
                    MenuRow::Header(idx) => (options[idx].clone(), label_col, theme::DIM()),
                    MenuRow::Footer => (
                        footer.clone().unwrap_or_default(),
                        item_text_col,
                        if is_hovered { theme::FG() } else { footer_color },
                    ),
                };
                let detail_room = match *row {
                    MenuRow::Option(idx) => details.get(idx).map_or(0.0, |detail| {
                        if detail.is_empty() { 0.0 } else { text_w(detail, detail_font_size) + 1.0 }
                    }),
                    _ => 0.0,
                };
                let option_display = truncate_text_to_width(
                    &text,
                    (item_right - text_col - detail_room).max(0.0),
                    menu_font_size,
                    viewport.cell_w,
                );
                if option_display.is_empty() {
                    continue;
                }
                super::push_overlay_primitive(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row: item_y + (MENU_ROW_HEIGHT - 1.0) * 0.5,
                        col: text_col,
                        align_width: 0.0,
                        h_align: 0.0,
                        text: option_display,
                        font_size: menu_font_size,
                        scale: 1.0,
                        // Popup labels are chrome, not the trigger's colored value.
                        fg,
                        bg: transparent,
                        mono: false,
                    },
                ));
            }
            super::push_overlay_primitive(GpuPrimitive::PopClipRect);

            // Scrollbar indicator (when content is taller than visible area)
            if needs_scrollbar {
                let track_margin = SCROLLBAR_MARGIN;
                let bar_col = menu_col + menu_width - SCROLLBAR_WIDTH - track_margin;
                let track_top = list_rect.row + track_margin;
                let track_height = list_rect.height - track_margin * 2.0;
                let thumb_ratio = (list_rect.height / (geo.content_height - list_top)).clamp(0.05, 1.0);
                let thumb_height = (track_height * thumb_ratio).max(1.0);
                let scroll_ratio = if geo.max_scroll > 0.0 {
                    scroll_off / geo.max_scroll
                } else {
                    0.0
                };
                let thumb_top = track_top + scroll_ratio * (track_height - thumb_height);
                let thumb_rect = Rect {
                    row: thumb_top,
                    col: bar_col,
                    width: SCROLLBAR_WIDTH,
                    height: thumb_height,
                };
                let thumb_color = resolve_named_color(
                    &node.props,
                    "scrollbar-color",
                    theme::DROPDOWN_SCROLLBAR(),
                );
                super::menu_style::emit_rounded_rect_overlay(
                    thumb_rect,
                    thumb_color,
                    3.0,
                    viewport,
                );
            }
        } else if !state.open {
            // Ensure this dropdown's overlay entry is removed when closed
            super::remove_overlay(node.widget_id);
        }

        prims
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    // The shared shader's radius is normalized to half the primitive height.
    // Converting from pixels keeps dropdown corners intentional across widths,
    // heights, and font/cell aspect ratios. A tiny positive value opts out of
    // the shader's historical pill default for intentionally square-ish rects.
    if radius_px <= 0.0 {
        return 0.001;
    }
    let radius_px = super::ui_design_px(radius_px);
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

fn emit_rounded_rect(
    prims: &mut Vec<GpuPrimitive>,
    rect: Rect,
    color: Color,
    viewport: WidgetViewport,
    is_background: bool,
    corner_radius: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(GpuPrimitive::WidgetInstance {
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
            corner_radius,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background,
    });
}

// ── Metal shaders ────────────────────────────────────────────────────────────

const DROPDOWN_CHEVRON_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float4 col = in.color_a;

    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Compact up chevron "^"
    float hw = 0.35 * aspect;
    float2 up_pt = float2(0.0, -0.70);
    float2 up_a  = float2(-hw, -0.22);
    float2 up_b  = float2( hw, -0.22);

    // Compact down chevron "v"
    float2 dn_pt = float2(0.0,  0.70);
    float2 dn_a  = float2(-hw,  0.22);
    float2 dn_b  = float2( hw,  0.22);

    // SDF for line segments
    float2 pa1 = p - up_a;  float2 ba1 = up_pt - up_a;
    float h1 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    float seg1 = length(pa1 - ba1 * h1);

    float2 pa2 = p - up_pt; float2 ba2 = up_b - up_pt;
    float h2 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    float seg2 = length(pa2 - ba2 * h2);

    float2 pa3 = p - dn_a;  float2 ba3 = dn_pt - dn_a;
    float h3 = clamp(dot(pa3, ba3) / dot(ba3, ba3), 0.0, 1.0);
    float seg3 = length(pa3 - ba3 * h3);

    float2 pa4 = p - dn_pt; float2 ba4 = dn_b - dn_pt;
    float h4 = clamp(dot(pa4, ba4) / dot(ba4, ba4), 0.0, 1.0);
    float seg4 = length(pa4 - ba4 * h4);

    float d = min(min(seg1, seg2), min(seg3, seg4));

    float stroke = 0.10;
    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#, super::wgsl::DROPDOWN_CHEVRON_SHADER);

const DROPDOWN_CHECKMARK_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = in.aspect;
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);

    float2 start = float2(-0.72 * aspect, -0.02);
    float2 joint = float2(-0.25 * aspect,  0.48);
    float2 end   = float2( 0.75 * aspect, -0.52);

    float2 pa1 = p - start; float2 ba1 = joint - start;
    float h1 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    float seg1 = length(pa1 - ba1 * h1);

    float2 pa2 = p - joint; float2 ba2 = end - joint;
    float h2 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    float seg2 = length(pa2 - ba2 * h2);

    float d = min(seg1, seg2);
    float stroke = 0.10;
    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard_fragment(); }
    return float4(in.color_a.rgb, in.color_a.a * mask);
}
"#, super::wgsl::DROPDOWN_CHECKMARK_SHADER);

/// The filter row's search glyph: a ring with a handle to the lower right
/// (uv y grows downward).
const DROPDOWN_MAGNIFIER_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = in.aspect;
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);

    float2 c = float2(-0.18, -0.18);
    float r = 0.52;
    float ring = abs(length(p - c) - r);

    float2 a = c + float2(0.707, 0.707) * r;
    float2 b = float2(0.78, 0.78);
    float2 pa = p - a; float2 ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    float grip = length(pa - ba * h);

    float d = min(ring, grip);
    float stroke = 0.11;
    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard_fragment(); }
    return float4(in.color_a.rgb, in.color_a.a * mask);
}
"#, super::wgsl::DROPDOWN_MAGNIFIER_SHADER);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::BindingKind;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn string_list(values: &[&str]) -> Value {
        Value::List(
            values
                .iter()
                .map(|value| Rc::new(RefCell::new(Value::String((*value).to_string()))))
                .collect(),
        )
    }

    #[test]
    fn selected_label_can_follow_index_value() {
        let mut props = HashMap::new();
        props.insert("options".to_string(), string_list(&["saw", "pulse", "tri"]));
        props.insert("value-index".to_string(), Value::Number(2.0));

        assert_eq!(get_selected(&props), "tri");
    }

    #[test]
    fn open_state_follows_a_stable_dropdown_across_layout_id_churn() {
        fn node(widget_id: u64) -> LayoutNode {
            let mut props = HashMap::new();
            props.insert("options".to_string(), string_list(&["off", "lfo", "env"]));
            props.insert("value".to_string(), Value::String("off".to_string()));
            LayoutNode {
                widget_id,
                stable_widget_id: Some(8_811_337),
                subtree_root_id: Some(8_811_000),
                parent_subtree_root_id: Some(8_811_000),
                stable_key: None,
                widget_type: "dropdown".to_string(),
                rect: Rect {
                    row: 1.0,
                    col: 2.0,
                    width: 5.0,
                    height: 1.0,
                },
                props,
                children: Vec::new(),
                focusable: true,
                animation: Default::default(),
            }
        }

        let provisional = node(81_001);
        let settled = node(81_019);
        close_dropdown(provisional.widget_id);
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            let outcome = DROPDOWN_WIDGET.mouse_event(
                &provisional,
                kind,
                provisional.rect.col,
                provisional.rect.row,
                None,
                None,
                KeyModifiers::NONE,
                10.0,
                20.0,
            );
            assert!(matches!(outcome, MouseEventOutcome::Consume));
        }
        assert!(is_dropdown_open(provisional.widget_id));

        let settled_state = get_state_for_node(&settled);
        assert!(
            settled_state.open,
            "the relayout must not lose the menu opened by the first click"
        );
        assert!(!settled_state.ignore_opening_mouse_up);
        assert!(is_dropdown_open(settled.widget_id));

        close_dropdown(settled.widget_id);
    }

    #[test]
    fn selected_label_can_follow_reactive_index_value() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let slot = slots.slot("TEST_DROPDOWN", "wave");
        slots.write_float("TEST_DROPDOWN", "wave", 1.0);

        let mut props = HashMap::new();
        props.insert("options".to_string(), string_list(&["saw", "pulse", "tri"]));
        props.insert(
            "value-index".to_string(),
            Value::ReactiveRef {
                namespace: "TEST_DROPDOWN".to_string(),
                field: "wave".to_string(),
                index: None,
                kind: BindingKind::Float,
                slot,
            },
        );

        assert_eq!(get_selected(&props), "pulse");
        slots.write_float("TEST_DROPDOWN", "wave", 2.0);
        assert_eq!(get_selected(&props), "tri");
    }

    #[test]
    fn selected_label_applies_index_offset() {
        let mut props = HashMap::new();
        props.insert("options".to_string(), string_list(&["svf", "ladder"]));
        props.insert("value-index".to_string(), Value::Number(2.0));
        props.insert("value-index-offset".to_string(), Value::Number(1.0));

        assert_eq!(get_selected(&props), "ladder");
    }

    #[test]
    fn truncation_does_not_spend_width_on_ellipsis() {
        assert_eq!(truncate_text_to_width("-1oct", 2.0, DEFAULT_FONT_SIZE, 10.0), "-1o");
        assert!(!truncate_text_to_width("-1oct", 2.0, DEFAULT_FONT_SIZE, 10.0).contains('…'));
    }

    #[test]
    fn menu_geometry_uses_space_below_short_originating_tile() {
        // The trigger lives in a two-row transport tile, but the frame-level
        // overlay viewport continues for another eighteen rows.
        let geometry = compute_menu_geometry(0.25, 1.0, 5, 0.0, 20.0);

        assert_eq!(geometry.visible_height, geometry.content_height);
        assert!(geometry.menu_top > 1.0);
        assert!(geometry.menu_top + geometry.visible_height > 2.0);
        assert!(geometry.menu_top + geometry.visible_height <= 20.0);
    }

    #[test]
    fn menu_geometry_can_open_above_its_originating_tile() {
        // Negative local rows represent frame space above a lower tile.
        let geometry = compute_menu_geometry(1.0, 1.0, 5, -12.0, 8.0);

        assert_eq!(geometry.visible_height, geometry.content_height);
        assert!(geometry.menu_top < 0.0);
        assert!(geometry.menu_top >= -12.0);
    }

    #[test]
    fn oversized_menu_scrolls_within_the_frame_overlay_viewport() {
        let geometry = compute_menu_geometry(2.0, 1.0, 100, -3.0, 7.0);

        assert!(geometry.visible_height <= 10.0);
        assert_eq!(geometry.menu_top, -2.9);
        assert!(geometry.max_scroll > 0.0);
    }

    #[test]
    fn fixed_size_parent_dropdown_keeps_menu_text_inside_padding() {
        struct Font(f32);
        impl crate::layout::TextMeasurer for Font {
            fn measure_text_px(&self, text: &str, font_size: f32) -> f32 {
                text.chars().count() as f32 * font_size * self.0
            }
            fn line_height_px(&self, font_size: f32) -> f32 { font_size * self.0 }
        }

        let mut runtime = crate::Runtime::new();
        let tree = runtime.eval_str(r#"
            (box :width 4.2 :height 0.8
              (dropdown :width 4.2 :height 0.5 :font-size 10
                :value "A" :options '("A" "B" "Wide menu option")))
        "#).unwrap().unwrap();
        // A stretched child of a fixed-size box never needs intrinsic
        // measurement. Rendering must still use the current font/cell metrics.
        for (scale, cell_w) in [(1.2, 10.0), (2.4, 12.0)] {
            let font = Rc::new(Font(scale));
            super::super::set_render_text_measurer(font.clone());
            let engine = crate::layout::LayoutEngine::with_text_measurer(
                100, 30, 1.0, font.as_ref(), cell_w, 20.0);
            let layout = engine.layout(&tree).unwrap();
            let node = &layout.children[0];
            assert!(node.rect.width.is_finite() && node.rect.width > 0.0);
            assert!(node.rect.height.is_finite() && node.rect.height > 0.0);
            let viewport = WidgetViewport {
                cell_w, cell_h: 20.0, vp_w: 1000.0, vp_h: 600.0,
                time_seconds: 0.0, focused_widget_id: None, focused_branch: false,
                overlay_viewport_bottom: 30.0, scroll_top: 0.0, scroll_left: 0.0,
                inherited_hover: false,
            };
            super::super::clear_overlay();
            close_dropdown(node.widget_id);
            DROPDOWN_WIDGET.mouse_event(node, MouseEventKind::Down(MouseButton::Left),
                node.rect.col, node.rect.row, None, None, KeyModifiers::NONE, cell_w, 20.0);
            let (_, overlays) = super::super::collect_gpu_primitives(node, viewport, 0.0, 30);
            let panel = super::super::get_overlay_rect().expect("open menu bounds");
            let labels: Vec<_> = overlays.iter().filter_map(|primitive| match primitive {
                GpuPrimitive::ProportionalText(text) => Some(text),
                _ => None,
            }).collect();
            assert_eq!(labels.len(), 3);
            assert_eq!(labels[2].text, "Wide menu option", "the menu must fit its complete options");
            for label in labels {
                let text_width = crate::layout::TextMeasurer::measure_text_px(
                    font.as_ref(), &label.text, label.font_size) / cell_w;
                assert!(label.col + text_width + PADDING_H <= panel.col + panel.width + 0.001,
                    "menu text must leave right padding: text={:?}, panel={panel:?}", label.text);
            }
            close_dropdown(node.widget_id);
            super::super::clear_overlay();
        }
    }

    #[test]
    fn open_menu_emits_a_finite_overlay_with_a_vector_selected_mark() {
        let widget_id = 91_337;
        let mut props = HashMap::new();
        props.insert(
            "options".to_string(),
            string_list(&["off", "1/16", "1/8", "1/4", "1/2", "1 bar"]),
        );
        props.insert("value".to_string(), Value::String("off".to_string()));
        let mut node = LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "dropdown".to_string(),
            rect: Rect {
                row: 0.25,
                col: 8.0,
                width: 6.0,
                height: 1.0,
            },
            props,
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 800.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };

        super::super::clear_overlay();
        set_state(widget_id, DropdownState::default());
        let outcome = DROPDOWN_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            node.rect.col,
            node.rect.row,
            None,
            None,
            KeyModifiers::NONE,
            viewport.cell_w,
            viewport.cell_h,
        );
        assert!(matches!(outcome, MouseEventOutcome::Consume));

        let (_tile_primitives, overlay_primitives) =
            crate::widget_render::collect_gpu_primitives(&node, viewport, 0.0, 2);
        let overlay_rect =
            super::super::get_overlay_rect().expect("open dropdown should register hit bounds");

        assert!(!overlay_primitives.is_empty());
        assert!(overlay_rect.width.is_finite() && overlay_rect.width > 0.0);
        assert!(overlay_rect.height.is_finite() && overlay_rect.height > 0.0);
        assert!(overlay_rect.row + overlay_rect.height > 2.0);
        assert!(overlay_primitives.iter().any(|primitive| matches!(
            primitive,
            GpuPrimitive::WidgetInstance { widget_type, .. }
                if widget_type == "dropdown-checkmark"
        )));
        assert!(!overlay_primitives.iter().any(|primitive| matches!(
            primitive,
            GpuPrimitive::ProportionalText(text) if text.text == "\u{2713}"
        )));

        set_state(widget_id, DropdownState::default());
        super::super::clear_overlay();

        node.props
            .insert("action-menu".to_string(), Value::Bool(true));
        let outcome = DROPDOWN_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            node.rect.col,
            node.rect.row,
            None,
            None,
            KeyModifiers::NONE,
            viewport.cell_w,
            viewport.cell_h,
        );
        assert!(matches!(outcome, MouseEventOutcome::Consume));
        let (_tile_primitives, overlay_primitives) =
            crate::widget_render::collect_gpu_primitives(&node, viewport, 0.0, 2);
        assert!(!overlay_primitives.iter().any(|primitive| matches!(
            primitive,
            GpuPrimitive::WidgetInstance { widget_type, .. }
                if widget_type == "dropdown-checkmark"
        )));

        set_state(widget_id, DropdownState::default());
        super::super::clear_overlay();
    }

    /// `:headers` marks section-header options: the keyboard steps over
    /// them, a click on one keeps the menu open and picks nothing, and the
    /// header renders dimmed with no check mark while the options pick.
    #[test]
    fn filter_keeps_matching_options_their_headers_and_the_footer() {
        let props = HashMap::from([
            (
                "options".to_string(),
                string_list(&["No groove", "Project", "Take", "Factory", "MPC 16 Swing 54%", "MPC 8 Swing 66%"]),
            ),
            (
                "headers".to_string(),
                Value::List(vec![
                    Rc::new(RefCell::new(Value::Number(1.0))),
                    Rc::new(RefCell::new(Value::Number(3.0))),
                ]),
            ),
            ("footer".to_string(), Value::String("Extract…".to_string())),
        ]);
        assert_eq!(menu_rows(&props, "").len(), 7, "everything, then the footer");
        assert_eq!(
            menu_rows(&props, "swing 66"),
            vec![MenuRow::Header(3), MenuRow::Option(5), MenuRow::Footer],
            "a header survives only over a match"
        );
        let rows = menu_rows(&props, "TAKE");
        assert_eq!(rows, vec![MenuRow::Header(1), MenuRow::Option(2), MenuRow::Footer]);
        assert_eq!(step_row(&rows, None, true), Some(1), "the first pickable row");
        assert_eq!(row_value(&props, MenuRow::Footer).as_deref(), Some("Extract…"));
        assert_eq!(row_value(&props, MenuRow::Header(1)), None);
    }

    #[test]
    fn header_options_are_drawn_but_never_picked() {
        let widget_id = 91_338;
        let mut props = HashMap::new();
        props.insert(
            "options".to_string(),
            string_list(&["Take", "Library", "Swing 58", "Off"]),
        );
        props.insert(
            "headers".to_string(),
            Value::List(vec![Rc::new(RefCell::new(Value::Number(1.0)))]),
        );
        props.insert("value".to_string(), Value::String("Take".to_string()));
        let node = LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "dropdown".to_string(),
            rect: Rect {
                row: 0.25,
                col: 8.0,
                width: 6.0,
                height: 1.0,
            },
            props,
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 800.0,
            vp_h: 400.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let key = |code| WidgetKeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
        };
        super::super::clear_overlay();
        set_state(widget_id, DropdownState::default());

        // Keyboard: open on "Take", Down skips the header to "Swing 58",
        // Up skips it back to "Take".
        DROPDOWN_WIDGET.key_event(&node, key(KeyCode::Enter));
        assert_eq!(get_state(widget_id).hovered_idx, Some(0));
        DROPDOWN_WIDGET.key_event(&node, key(KeyCode::Down));
        assert_eq!(get_state(widget_id).hovered_idx, Some(2));
        DROPDOWN_WIDGET.key_event(&node, key(KeyCode::Up));
        assert_eq!(get_state(widget_id).hovered_idx, Some(0));
        DROPDOWN_WIDGET.key_event(&node, key(KeyCode::Down));

        // Render: the header row is dimmed, the options are not.
        let (_tile, overlay) =
            crate::widget_render::collect_gpu_primitives(&node, viewport, 0.0, 2);
        let text_color = |label: &str| {
            overlay
                .iter()
                .find_map(|primitive| match primitive {
                    GpuPrimitive::ProportionalText(text) if text.text == label => Some(text.fg),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{label} renders"))
        };
        assert_eq!(text_color("Library"), theme::DIM());
        assert_eq!(text_color("Swing 58"), theme::FG());

        // A header on a hovered Enter (the mouse can rest on it) picks
        // nothing and keeps the menu open.
        let mut state = get_state(widget_id);
        state.hovered_idx = Some(1);
        set_state(widget_id, state);
        assert!(matches!(
            DROPDOWN_WIDGET.key_event(&node, key(KeyCode::Enter)),
            Some(WidgetEvent::Custom(Value::Nil))
        ));
        assert!(get_state(widget_id).open);

        // Mouse: a click on the header row is consumed, the menu stays open;
        // a click on the option under it picks that option.
        let overlay_rect = super::super::get_overlay_rect().expect("open menu bounds");
        let row_of =
            |idx: usize| overlay_rect.row + MENU_PADDING_V + (idx as f32 + 0.5) * MENU_ROW_HEIGHT;
        let click = |row: f32| {
            DROPDOWN_WIDGET.mouse_event(
                &node,
                MouseEventKind::Down(MouseButton::Left),
                overlay_rect.col + 1.0,
                row,
                None,
                None,
                KeyModifiers::NONE,
                viewport.cell_w,
                viewport.cell_h,
            )
        };
        assert!(matches!(click(row_of(1)), MouseEventOutcome::Consume));
        assert!(
            get_state(widget_id).open,
            "a header click keeps the menu open"
        );
        match click(row_of(2)) {
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::String(picked))) => {
                assert_eq!(picked, "Swing 58")
            }
            _ => panic!("the option under the header picks"),
        }

        set_state(widget_id, DropdownState::default());
        super::super::clear_overlay();
    }

    #[test]
    fn value_index_accepts_reactive_binding_at_widget_construction() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let slot = slots.slot("TEST_DROPDOWN", "wave_construct");
        let widget = crate::widgets::build_widget(
            "dropdown",
            vec![
                Value::Keyword("value-index".to_string()),
                Value::ReactiveRef {
                    namespace: "TEST_DROPDOWN".to_string(),
                    field: "wave_construct".to_string(),
                    index: None,
                    kind: BindingKind::Float,
                    slot,
                },
                Value::Keyword("options".to_string()),
                string_list(&["saw", "pulse"]),
            ],
        );

        let Value::Map(props) = widget else {
            panic!("dropdown with bound value-index should construct a widget map");
        };
        assert!(props.contains_key("type"));
        assert!(props.contains_key("value-index"));
    }

    #[test]
    fn menu_button_constructor_marks_an_icon_only_focusable_action_menu() {
        let widget = crate::widgets::build_widget(
            "menu-button",
            vec![
                Value::Keyword("options".to_string()),
                string_list(&["Copy current values to all scenes"]),
            ],
        );
        let Value::Map(map) = widget else {
            panic!("menu-button should construct a widget map");
        };
        assert!(matches!(
            map.get("type").map(|value| value.borrow().clone()),
            Some(Value::Keyword(kind)) if kind == "menu-button"
        ));
        assert!(matches!(
            map.get("action-menu").map(|value| value.borrow().clone()),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            map.get("focusable").map(|value| value.borrow().clone()),
            Some(Value::Bool(true))
        ));
        let props = props_from_node(&Value::Map(map));
        assert!(is_action_menu(&props));
        assert_eq!(
            initial_mouse_hovered_index(&props),
            None,
            "mouse activation should not preselect an action"
        );
        assert_eq!(
            initial_keyboard_hovered_index(&props),
            Some(0),
            "keyboard activation should focus the first action immediately"
        );
        let rect = Rect {
            row: 2.0,
            col: 0.0,
            width: 2.25,
            height: 0.7,
        };
        assert!((trigger_text_row(&props, rect) - 1.77).abs() < f32::EPSILON);
    }
}
