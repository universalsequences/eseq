use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_f32_prop, plock_active, plock_color, resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_map,
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
}

thread_local! {
    /// Dropdown state keyed by stable widget identity when available, falling
    /// back to the current layout-local widget ID.
    static STATES: RefCell<HashMap<DropdownStateKey, DropdownState>> = RefCell::new(HashMap::new());
    /// Resolves overlay/event widget IDs to their stable state keys. A newly
    /// mounted conditional subtree can receive one numeric ID for its first
    /// interaction and another after the resulting relayout.
    static STATE_KEYS_BY_WIDGET_ID: RefCell<HashMap<u64, DropdownStateKey>> = RefCell::new(HashMap::new());
    /// Cached per-character cell widths for dropdown labels.
    /// Key: (font_size_bits, text) -> cell-widths for each character.
    static CHAR_WIDTH_CACHE: RefCell<HashMap<(u32, String), Vec<f32>>> =
        RefCell::new(HashMap::new());
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
        }
    });
}

pub fn is_dropdown_open(widget_id: u64) -> bool {
    get_state(widget_id).open
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

        let new_idx = if menu_row >= 0.0 {
            let content_row = menu_row + state.scroll_offset;
            let idx = ((content_row - MENU_PADDING_V) / MENU_ROW_HEIGHT).floor() as isize;
            // Derive option count from content_height
            let option_count =
                ((state.content_height - MENU_PADDING_V * 2.0) / MENU_ROW_HEIGHT).round() as usize;
            if idx >= 0 && (idx as usize) < option_count {
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

fn cache_text_widths(text: &str, font_size: f32, ctx: &MeasureCtx<'_>) {
    let Some(measurer) = ctx.text_measurer else {
        return;
    };
    let key = (font_size.to_bits(), text.to_string());
    CHAR_WIDTH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.entry(key).or_insert_with(|| {
            text.chars()
                .map(|ch| measurer.measure_text_px(&ch.to_string(), font_size) / ctx.cell_w)
                .collect()
        });
    });
}

fn text_width_cells(text: &str, font_size: f32) -> f32 {
    let key = (font_size.to_bits(), text.to_string());
    CHAR_WIDTH_CACHE.with(|cache| {
        cache
            .borrow()
            .get(&key)
            .map(|widths| widths.iter().sum())
            .unwrap_or_else(|| text.chars().count() as f32 * APPROX_CHAR_WIDTH)
    })
}

fn truncate_text_to_width(text: &str, max_width: f32, font_size: f32) -> String {
    if max_width <= 0.0 || text.is_empty() {
        return String::new();
    }

    let key = (font_size.to_bits(), text.to_string());
    let widths = CHAR_WIDTH_CACHE.with(|cache| cache.borrow().get(&key).cloned());

    let fits_full = widths
        .as_ref()
        .map(|w| w.iter().sum::<f32>() <= max_width)
        .unwrap_or_else(|| text_width_cells(text, font_size) <= max_width);
    if fits_full {
        return text.to_string();
    }

    let mut acc = 0.0;
    let mut out = String::new();

    match widths {
        Some(widths) => {
            for (ch, ch_width) in text.chars().zip(widths.iter().copied()) {
                if acc + ch_width > max_width {
                    break;
                }
                out.push(ch);
                acc += ch_width;
            }
        }
        None => {
            let fallback = APPROX_CHAR_WIDTH;
            for ch in text.chars() {
                if acc + fallback > max_width {
                    break;
                }
                out.push(ch);
                acc += fallback;
            }
        }
    }

    out
}

fn selected_index(options: &[String], selected: &str) -> Option<usize> {
    options.iter().position(|o| o == selected)
}

fn initial_mouse_hovered_index(
    props: &HashMap<String, Value>,
    options: &[String],
) -> Option<usize> {
    if is_action_menu(props) {
        None
    } else {
        selected_index(options, &get_selected(props))
    }
}

fn initial_keyboard_hovered_index(
    props: &HashMap<String, Value>,
    options: &[String],
) -> Option<usize> {
    if is_action_menu(props) {
        (!options.is_empty()).then_some(0)
    } else {
        selected_index(options, &get_selected(props))
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
    let content_height = option_count as f32 * MENU_ROW_HEIGHT + MENU_PADDING_V * 2.0;
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
    let content_height = option_count as f32 * MENU_ROW_HEIGHT + MENU_PADDING_V * 2.0;
    let max_scroll = (content_height - state.visible_height).max(0.0);
    let item_top = MENU_PADDING_V + idx as f32 * MENU_ROW_HEIGHT;
    let item_bottom = item_top + MENU_ROW_HEIGHT;

    if item_top < state.scroll_offset {
        state.scroll_offset = item_top;
    } else if item_bottom > state.scroll_offset + state.visible_height {
        state.scroll_offset = item_bottom - state.visible_height;
    }
    state.scroll_offset = state.scroll_offset.clamp(0.0, max_scroll);
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct DropdownWidget;
pub static DROPDOWN_WIDGET: DropdownWidget = DropdownWidget;

impl WidgetDefinition for DropdownWidget {
    fn names(&self) -> &'static [&'static str] {
        &["dropdown", "menu-button", "dropdown-chevron"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "options",
            "value",
            "value-index",
            "value-index-offset",
            "width",
            "height",
            "font-size",
            "icon",
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
            "options", "value", "value-index", "value-index-offset", "width", "height",
            "font-size", "icon", "focusable", "action-menu", "badge-color", "bg-color",
            "border-color", "border-width", "check-color", "chevron-color", "hover-bg",
            "menu-bg", "menu-border-color", "ring-color", "scrollbar-color", "text-color",
            "on-change", "plock-active", "plock-color-r", "plock-color-g", "plock-color-b",
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
        let menu_font_size = super::menu_style::menu_font_size_from_node(node);
        let props = props_from_node(node);
        let action_menu = is_action_menu(&props);
        let selected = get_selected(&props);
        let options = get_options(&props);
        if ctx.text_measurer.is_some() {
            if !selected.is_empty() {
                cache_text_widths(&selected, font_size, ctx);
            }
            for option in &options {
                cache_text_widths(&option, font_size, ctx);
                cache_text_widths(&option, menu_font_size, ctx);
            }
        }
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(if action_menu { 1.1 } else { 1.5 });
        let explicit_width = get_prop_num(node, "width").map(f64_to_f32);
        let width = explicit_width.unwrap_or(if action_menu { 2.2 } else { 10.0 });
        if action_menu {
            if ctx.text_measurer.is_some() {
                cache_text_widths(&action_menu_icon(&props), font_size, ctx);
            }
            return Some(Size { width, height });
        }
        let selected_width = if props.contains_key("value-index") {
            options
                .iter()
                .map(|option| text_width_cells(option, font_size))
                .fold(text_width_cells(&selected, font_size), f32::max)
        } else {
            text_width_cells(&selected, font_size)
        };
        let chevron_width = height * 0.48 * 1.8;
        let min_width =
            PADDING_H + selected_width + TEXT_CHEVRON_GAP + chevron_width + CHEVRON_RIGHT_PAD;
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
        let options = get_options(&node.props);

        if state.open {
            if matches!(mouse_kind, MouseEventKind::Up(MouseButton::Left))
                && state.ignore_opening_mouse_up
            {
                state.ignore_opening_mouse_up = false;
                set_state(node.widget_id, state);
                return MouseEventOutcome::Consume;
            }
            // Use the overlay rect (registered at render time) for hit-testing.
            // The overlay rect is in screen-space; local_row is in layout-space.
            let overlay_rect = super::overlay_rect_for_widget(node.widget_id);
            let menu_row = if let Some(rect) = overlay_rect {
                let r = local_row - rect.row;
                if r >= 0.0 && r < rect.height { r } else { -1.0 }
            } else {
                -1.0
            };

            if menu_row >= 0.0 {
                // Account for scroll offset: the visible window starts at scroll_offset
                let content_row = menu_row + state.scroll_offset;
                let item_idx = ((content_row - MENU_PADDING_V) / MENU_ROW_HEIGHT).floor() as usize;
                if item_idx < options.len() {
                    state.hovered_idx = Some(item_idx);
                    set_state(node.widget_id, state);
                    if matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
                        let mut state = get_state(node.widget_id);
                        state.open = false;
                        state.hovered_idx = None;
                        state.ignore_opening_mouse_up = false;
                        state.scroll_offset = 0.0;
                        set_state(node.widget_id, state);
                        super::remove_overlay(node.widget_id);
                        return MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::String(
                            options[item_idx].clone(),
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
            state.hovered_idx = initial_mouse_hovered_index(&node.props, &options);
            state.ignore_opening_mouse_up = true;
            state.scroll_offset = 0.0;
            set_state(node.widget_id, state);
            MouseEventOutcome::Consume
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let mut state = get_state_for_node(node);
        let options = get_options(&node.props);
        if options.is_empty() {
            return None;
        }

        if !state.open {
            // When closed: only Enter opens the menu.
            // Up/Down are NOT consumed — they fall through to focus navigation.
            match key.code {
                KeyCode::Enter => {
                    close_other_dropdowns(node.widget_id);
                    state.open = true;
                    state.hovered_idx = initial_keyboard_hovered_index(&node.props, &options);
                    set_state(node.widget_id, state);
                    return Some(WidgetEvent::Custom(Value::Nil));
                }
                _ => return None,
            }
        }

        // Menu is open
        match key.code {
            KeyCode::Down => {
                let next = state
                    .hovered_idx
                    .map(|i| (i + 1).min(options.len() - 1))
                    .unwrap_or(0);
                state.hovered_idx = Some(next);
                ensure_visible(&mut state, options.len());
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Up => {
                let prev = state.hovered_idx.map(|i| i.saturating_sub(1)).unwrap_or(0);
                state.hovered_idx = Some(prev);
                ensure_visible(&mut state, options.len());
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Enter => {
                if let Some(idx) = state.hovered_idx {
                    let value = options.get(idx).cloned().unwrap_or_default();
                    state.open = false;
                    state.hovered_idx = None;
                    state.scroll_offset = 0.0;
                    set_state(node.widget_id, state);
                    super::remove_overlay(node.widget_id);
                    Some(WidgetEvent::Custom(Value::String(value)))
                } else {
                    state.open = false;
                    state.scroll_offset = 0.0;
                    set_state(node.widget_id, state);
                    super::remove_overlay(node.widget_id);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
            KeyCode::Esc => {
                state.open = false;
                state.hovered_idx = None;
                state.scroll_offset = 0.0;
                set_state(node.widget_id, state);
                super::remove_overlay(node.widget_id);
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

    fn metal_fragment_shader(&self, widget_type: &str) -> Option<&'static str> {
        match widget_type {
            "dropdown" | "menu-button" => Some(super::ROUNDED_RECT_SHADER),
            "dropdown-chevron" => Some(DROPDOWN_CHEVRON_SHADER),
            _ => None,
        }
    }

    fn build_metal_primitives(
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
            emit_rounded_rect(&mut prims, ring_rect, ring_color, viewport, true, 0.0);
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
            emit_rounded_rect(&mut prims, border_rect, border_color, viewport, true, 0.0);
        }

        // ── Background ──
        emit_rounded_rect(&mut prims, node.rect, bg_color, viewport, true, 0.0);

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
                },
            ));
        } else {
            // ── Selected text ──
            let text_col = node.rect.col + PADDING_H;
            let text_clip_rect = Rect {
                row: node.rect.row + 0.08,
                col: text_col,
                width: (ch_rect.col - TEXT_CHEVRON_GAP - text_col).max(0.0),
                height: (node.rect.height - 0.16).max(0.0),
            };
            let selected_display =
                truncate_text_to_width(&selected, text_clip_rect.width, font_size);
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
        if state.open && !options.is_empty() {
            let screen_col = node.rect.col - viewport.scroll_left;
            let screen_row = node.rect.row - viewport.scroll_top;
            let viewport_rows = viewport.vp_h / viewport.cell_h.max(1.0);
            let viewport_bottom = viewport.overlay_viewport_bottom;
            let viewport_top = viewport_bottom - viewport_rows;

            let geo = compute_menu_geometry(
                screen_row,
                node.rect.height,
                options.len(),
                viewport_top,
                viewport_bottom,
            );

            // Persist geometry so key_event/scroll can operate correctly
            state.visible_height = geo.visible_height;
            state.content_height = geo.content_height;
            state.scroll_offset = state.scroll_offset.clamp(0.0, geo.max_scroll);
            // Ensure the hovered item is visible (e.g. when opening with a far-down selection)
            ensure_visible(&mut state, options.len());
            set_state(node.widget_id, state.clone());

            // Menu width: at least trigger width, expanded to fit longest option
            let needs_scrollbar = geo.content_height > geo.visible_height;
            let check_col_width = if action_menu { 0.0 } else { 1.5 }; // space for ✓ mark
            let text_left_pad = PADDING_H + check_col_width;
            let scrollbar_pad = if needs_scrollbar {
                SCROLLBAR_WIDTH + SCROLLBAR_MARGIN * 2.0
            } else {
                0.0
            };
            let max_option_width = options
                .iter()
                .map(|o| text_width_cells(o, menu_font_size))
                .fold(0.0_f32, f32::max);
            let content_width = text_left_pad + max_option_width + PADDING_H + scrollbar_pad;
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
            super::push_overlay_primitive(GpuPrimitive::PushClipRect(menu_rect));

            // Menu items — only emit those within the visible scroll window
            let sel_idx = selected_index(&options, &selected);
            let scroll_off = state.scroll_offset;
            let label_col = menu_col + PADDING_H;
            let item_text_col = label_col + check_col_width;
            let item_text_width =
                (menu_width - (item_text_col - menu_col) - PADDING_H - scrollbar_pad).max(0.0);
            for (i, option) in options.iter().enumerate() {
                // Item position in content space (relative to menu content start)
                let content_y = MENU_PADDING_V + i as f32 * MENU_ROW_HEIGHT;
                // Position in visible space (subtract scroll, add menu_top)
                let visible_y = content_y - scroll_off;

                // Skip items fully outside the visible window
                if visible_y + MENU_ROW_HEIGHT < 0.0 || visible_y >= geo.visible_height {
                    continue;
                }

                let item_y = geo.menu_top + visible_y;

                // Hover/selected highlight
                let is_hovered = state.hovered_idx == Some(i);
                if is_hovered {
                    let hl_rect = Rect {
                        row: item_y,
                        col: menu_col,
                        width: menu_width,
                        height: MENU_ROW_HEIGHT,
                    };
                    super::menu_style::emit_row_highlight(hl_rect, hover_bg, viewport);
                }

                // Check mark for selected item
                if !action_menu && sel_idx == Some(i) {
                    super::push_overlay_primitive(GpuPrimitive::ProportionalText(
                        GpuProportionalTextPrimitive {
                            row: item_y + (MENU_ROW_HEIGHT - 1.0) * 0.5,
                            col: label_col,
                            align_width: 0.0,
                            h_align: 0.0,
                            text: "✓".to_string(),
                            font_size: menu_font_size,
                            scale: 1.0,
                            fg: check_color,
                            bg: transparent,
                        },
                    ));
                }

                // Option label
                let option_display =
                    truncate_text_to_width(option, item_text_width, menu_font_size);
                if option_display.is_empty() {
                    continue;
                }
                super::push_overlay_primitive(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row: item_y + (MENU_ROW_HEIGHT - 1.0) * 0.5,
                        col: item_text_col,
                        align_width: 0.0,
                        h_align: 0.0,
                        text: option_display,
                        font_size: menu_font_size,
                        scale: 1.0,
                        fg: text_color,
                        bg: transparent,
                    },
                ));
            }
            super::push_overlay_primitive(GpuPrimitive::PopClipRect);

            // Scrollbar indicator (when content is taller than visible area)
            if needs_scrollbar {
                let track_margin = SCROLLBAR_MARGIN;
                let bar_col = menu_col + menu_width - SCROLLBAR_WIDTH - track_margin;
                let track_top = geo.menu_top + track_margin;
                let track_height = geo.visible_height - track_margin * 2.0;
                let thumb_ratio = (geo.visible_height / geo.content_height).clamp(0.05, 1.0);
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

const DROPDOWN_CHEVRON_SHADER: &str = r#"
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
"#;

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
        assert_eq!(truncate_text_to_width("-1oct", 2.0, 10.0), "-1o");
        assert!(!truncate_text_to_width("-1oct", 2.0, 10.0).contains('…'));
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
    fn open_menu_emits_a_finite_overlay_beyond_a_short_tile() {
        let widget_id = 91_337;
        let mut props = HashMap::new();
        props.insert(
            "options".to_string(),
            string_list(&["off", "1/16", "1/8", "1/4", "1/2", "1 bar"]),
        );
        props.insert("value".to_string(), Value::String("off".to_string()));
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
            initial_mouse_hovered_index(&props, &get_options(&props)),
            None,
            "mouse activation should not preselect an action"
        );
        assert_eq!(
            initial_keyboard_hovered_index(&props, &get_options(&props)),
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
