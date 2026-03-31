use std::collections::HashMap;

use crate::vm::{Value, format_lisp_value};
use crate::widget_render;

/// Default font size (in points) used when no explicit font-size is specified.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub row: f32,
    pub col: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy)]
pub struct Constraints {
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
    pub aspect: f32,
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub widget_id: u64,
    pub widget_type: String,
    pub rect: Rect,
    pub props: HashMap<String, Value>,
    pub children: Vec<LayoutNode>,
    pub focusable: bool,
}

/// Trait for measuring proportional text width in pixels.
/// Implemented by the Metal backend wrapping `ProportionalGlyphAtlas`.
/// `None` in TUI mode — labels fall back to monospace char-count measurement.
pub trait TextMeasurer {
    fn measure_text_px(&self, text: &str, font_size: f32) -> f32;
    fn line_height_px(&self, font_size: f32) -> f32;
}

/// Context passed to `WidgetDefinition::measure()` for proportional text support.
pub struct MeasureCtx<'a> {
    pub text_measurer: Option<&'a dyn TextMeasurer>,
    /// Monospace cell width in pixels (for converting proportional px → cell units).
    pub cell_w: f32,
    /// Monospace cell height in pixels.
    pub cell_h: f32,
    /// Font size inherited from ancestor containers (logical points).
    /// Labels use this as their default when no explicit `:font-size` is set.
    pub inherited_font_size: f32,
}

pub struct LayoutEngine<'a> {
    pub terminal_cols: u16,
    pub terminal_rows: u16,
    pub aspect: f32,
    pub text_measurer: Option<&'a dyn TextMeasurer>,
    pub cell_w: f32,
    pub cell_h: f32,
}

impl<'a> LayoutEngine<'a> {
    pub fn new(cols: u16, rows: u16, aspect: f32) -> Self {
        Self {
            terminal_cols: cols,
            terminal_rows: rows,
            aspect,
            text_measurer: None,
            cell_w: 1.0,
            cell_h: 1.0,
        }
    }

    pub fn with_text_measurer(
        cols: u16,
        rows: u16,
        aspect: f32,
        text_measurer: &'a dyn TextMeasurer,
        cell_w: f32,
        cell_h: f32,
    ) -> Self {
        Self {
            terminal_cols: cols,
            terminal_rows: rows,
            aspect,
            text_measurer: Some(text_measurer),
            cell_w,
            cell_h,
        }
    }

    pub fn layout(&self, tree: &Value) -> Option<LayoutNode> {
        let size = self.measure(
            tree,
            Constraints {
                min_width: 0.0,
                max_width: self.terminal_cols as f32,
                min_height: 0.0,
                max_height: f32::MAX,
                aspect: self.aspect,
            },
            DEFAULT_FONT_SIZE,
        )?;
        // If any direct child has :flex, use viewport height so flex children
        // can fill remaining space (e.g. a scroll container with :flex 1).
        // Otherwise use measured content height to preserve existing behavior.
        let has_flex_children = get_children(tree)
            .iter()
            .any(|child| get_prop_num(child, "flex").is_some_and(|f| f > 0.0));
        let root_height = if has_flex_children {
            (self.terminal_rows as f32).max(size.height)
        } else {
            size.height
        };
        let mut layout = self.build_layout_node(
            tree,
            Rect {
                row: 0.0,
                col: 0.0,
                width: size.width,
                height: root_height,
            },
            DEFAULT_FONT_SIZE,
        );
        let mut next_widget_id = 1;
        assign_widget_ids(&mut layout, &mut next_widget_id);
        Some(layout)
    }

    /// Measure the natural (unconstrained) content width of a widget tree.
    /// Used for horizontal scroll bounds — if this exceeds the viewport, scrolling is needed.
    pub fn natural_content_width(&self, tree: &Value) -> f32 {
        self.measure(
            tree,
            Constraints {
                min_width: 0.0,
                max_width: f32::MAX,
                min_height: 0.0,
                max_height: f32::MAX,
                aspect: self.aspect,
            },
            DEFAULT_FONT_SIZE,
        )
        .map(|s| s.width)
        .unwrap_or(0.0)
    }

    fn measure(
        &self,
        node: &Value,
        constraints: Constraints,
        inherited_font_size: f32,
    ) -> Option<Size> {
        let widget_type = get_widget_type(node)?;
        let children = get_children(node);

        // If this node sets :font-size, children inherit it.
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(inherited_font_size);

        let ctx = MeasureCtx {
            text_measurer: self.text_measurer,
            cell_w: self.cell_w,
            cell_h: self.cell_h,
            inherited_font_size: font_size,
        };

        let size = if let Some(definition) = widget_render::widget_definition(&widget_type) {
            definition.measure(
                node,
                &children,
                constraints,
                &ctx,
                &mut |child, child_constraints| self.measure(child, child_constraints, font_size),
            )?
        } else if let Some(sdf_size) = widget_render::sdf_widget::sdf_widget_measure(
            &widget_type,
            node,
            &children,
            constraints,
            &ctx,
        ) {
            sdf_size
        } else {
            measure_builtin_leaf(node, &widget_type, constraints.aspect)
        };

        Some(clamp_size_for_node(node, size, constraints))
    }

    fn build_layout_node(&self, node: &Value, rect: Rect, inherited_font_size: f32) -> LayoutNode {
        let widget_type = get_widget_type(node).unwrap_or_default();
        let children_values = get_children(node);

        // Resolve font-size: explicit on this node, or inherited from parent.
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(inherited_font_size);

        let children = self.layout_children_with_font(node, rect, &children_values, font_size);
        let mut props = collect_props(node);

        // Inject inherited font-size into props so the rendering path can use it.
        if !props.contains_key("font-size")
            && (inherited_font_size - DEFAULT_FONT_SIZE).abs() > 0.01
        {
            props.insert(
                "font-size".to_string(),
                Value::Number(inherited_font_size as f64),
            );
        }

        // For scroll containers, inject content/viewport dimensions so the
        // scroll event handler and renderer can compute scroll bounds.
        if widget_type == "scroll" {
            let content_height = children
                .first()
                .map(|c| c.rect.height)
                .unwrap_or(0.0);
            props.insert(
                "_content_height".to_string(),
                Value::Number(content_height as f64),
            );
            props.insert(
                "_viewport_height".to_string(),
                Value::Number(rect.height as f64),
            );
        }

        let focusable = matches!(props.get("focusable"), Some(Value::Bool(true)));
        LayoutNode {
            widget_id: 0,
            widget_type,
            rect,
            props,
            children,
            focusable,
        }
    }

    fn layout_children_with_font(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        inherited_font_size: f32,
    ) -> Vec<LayoutNode> {
        let Some(widget_type) = get_widget_type(node) else {
            return vec![];
        };

        // If this container sets :font-size, children inherit it.
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(inherited_font_size);

        widget_render::widget_definition(&widget_type)
            .map(|definition| {
                definition.layout_children(
                    node,
                    area,
                    children,
                    self.aspect,
                    &mut |child, child_constraints| {
                        let mut cc = child_constraints;
                        cc.aspect = self.aspect;
                        self.measure(child, cc, font_size)
                    },
                    &mut |child, rect| self.build_layout_node(child, rect, font_size),
                )
            })
            .unwrap_or_default()
    }
}

fn node_has_event_handler(node: &LayoutNode) -> bool {
    node.props.contains_key("on-click")
        || node.props.contains_key("on-drag")
        || node.props.contains_key("on-change")
        || node.props.contains_key("on-mouse-down")
}

pub fn hit_test_layout(node: &LayoutNode, row: f32, col: f32) -> Option<&LayoutNode> {
    // Scroll containers: only hit-test within viewport rect, and adjust
    // coordinates by scroll offset before recursing into children.
    if node.widget_type == "scroll" {
        if !rect_contains(node.rect, row, col) {
            return None;
        }
        let state = widget_render::scroll::get_scroll_state(node.widget_id);
        let adjusted_row = row + state.offset_y;
        for child in node.children.iter().rev() {
            if let Some(hit) = hit_test_layout(child, adjusted_row, col) {
                return Some(hit);
            }
        }
        // The scroll container itself is hittable (for scroll gestures)
        return Some(node);
    }

    // Container nodes: always recurse into children — their rects may be
    // clamped to the viewport while children extend beyond (scroll).
    for child in node.children.iter().rev() {
        if let Some(hit) = hit_test_layout(child, row, col) {
            // If the child doesn't handle events but this node does, bubble up
            if !node_has_event_handler(hit) && node_has_event_handler(node) {
                return Some(node);
            }
            return Some(hit);
        }
    }

    // Check if point is within this widget's rect
    if rect_contains(node.rect, row, col) {
        // Leaf widgets are always hittable.
        // Containers are hittable only if they handle mouse events
        // (e.g. tabs has a clickable header area).
        if !widget_render::is_layout_widget_type(&node.widget_type) {
            return Some(node);
        }
        if node.props.contains_key("on-change") || node.props.contains_key("bind")
            || node.props.contains_key("on-click")
        {
            return Some(node);
        }
    }

    None
}

fn reuse_layout_node_impl(
    existing: &LayoutNode,
    tree: &Value,
    dirty_widget_ids: &mut Vec<u64>,
    path: &mut Vec<String>,
) -> Result<LayoutNode, String> {
    let format_reason = |reason: String, path: &[String]| {
        if path.is_empty() {
            reason
        } else {
            format!("{reason}@{}", path.join(">"))
        }
    };
    let widget_type = get_widget_type(tree).ok_or_else(|| "not-widget".to_string())?;
    if widget_type != existing.widget_type {
        return Err(format_reason(format!(
            "widget-type:{}->{}",
            existing.widget_type, widget_type
        ), path));
    }
    // Tree widgets manage internal expand/collapse state that changes their
    // height without changing props. Always force full relayout.
    if widget_type == "tree" {
        return Err(format_reason("tree-widget".to_string(), path));
    }

    let children_values = get_children(tree);
    if children_values.len() != existing.children.len() {
        return Err(format_reason(format!(
            "children-len:{}:{}->{}",
            widget_type,
            existing.children.len(),
            children_values.len()
        ), path));
    }

    let new_props = collect_props(tree);
    if !size_affecting_props_equal(&widget_type, &existing.props, &new_props) {
        return Err(format_reason(format!("size-props:{widget_type}"), path));
    }

    if existing.props != new_props {
        dirty_widget_ids.push(existing.widget_id);
    }

    let children = existing
        .children
        .iter()
        .zip(children_values.iter())
        .enumerate()
        .map(|(idx, (child_layout, child_tree))| {
            path.push(format!("{widget_type}[{idx}]"));
            let result = reuse_layout_node_impl(child_layout, child_tree, dirty_widget_ids, path);
            path.pop();
            result
        })
        .collect::<Result<Vec<_>, _>>()?;

    let focusable = matches!(new_props.get("focusable"), Some(Value::Bool(true)));
    Ok(LayoutNode {
        widget_id: existing.widget_id,
        widget_type,
        rect: existing.rect,
        props: new_props,
        children,
        focusable,
    })
}

pub fn reuse_layout_node(
    existing: &LayoutNode,
    tree: &Value,
    dirty_widget_ids: &mut Vec<u64>,
) -> Option<LayoutNode> {
    let mut path = Vec::new();
    reuse_layout_node_impl(existing, tree, dirty_widget_ids, &mut path).ok()
}

pub fn reuse_layout_failure_reason(existing: &LayoutNode, tree: &Value) -> Option<String> {
    let mut dirty_widget_ids = Vec::new();
    let mut path = Vec::new();
    reuse_layout_node_impl(existing, tree, &mut dirty_widget_ids, &mut path).err()
}

pub fn same_layout_geometry(left: &LayoutNode, right: &LayoutNode) -> bool {
    left.widget_type == right.widget_type
        && left.rect == right.rect
        && left.children.len() == right.children.len()
        && left
            .children
            .iter()
            .zip(right.children.iter())
            .all(|(left_child, right_child)| same_layout_geometry(left_child, right_child))
}

fn rect_contains(rect: Rect, row: f32, col: f32) -> bool {
    row >= rect.row
        && col >= rect.col
        && row < rect.row + rect.height
        && col < rect.col + rect.width
}

fn assign_widget_ids(node: &mut LayoutNode, next_widget_id: &mut u64) {
    node.widget_id = *next_widget_id;
    *next_widget_id = next_widget_id.wrapping_add(1);
    for child in &mut node.children {
        assign_widget_ids(child, next_widget_id);
    }
}

fn size_affecting_props_equal(
    widget_type: &str,
    old_props: &HashMap<String, Value>,
    new_props: &HashMap<String, Value>,
) -> bool {
    if widget_type == "label" {
        let width_equal = value_option_eq(old_props.get("width"), new_props.get("width"));
        let width_locked = old_props.contains_key("width") || new_props.contains_key("width");
        return width_equal
            && (width_locked || value_option_eq(old_props.get("text"), new_props.get("text")));
    }

    let keys: &[&str] = if let Some(definition) = widget_render::widget_definition(widget_type) {
        definition.size_affecting_props()
    } else if widget_render::sdf_widget::sdf_widget_def(widget_type).is_some() {
        &[]
    } else {
        match widget_type {
            "knob" => &[],
            "meter" => &[],
            "text-input" => &["width"],
            "select" => &["options"],
            _ => return false,
        }
    };

    keys.iter()
        .all(|key| value_option_eq(old_props.get(*key), new_props.get(*key)))
}

fn value_option_eq(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => value_eq(left, right),
        _ => false,
    }
}

fn value_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Nil, Value::Nil) => true,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Symbol(a), Value::Symbol(b)) => a == b,
        (Value::Keyword(a), Value::Keyword(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| value_eq(&x.borrow(), &y.borrow()))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, left_value)| match b.get(key) {
                    Some(right_value) => value_eq(&left_value.borrow(), &right_value.borrow()),
                    None => false,
                })
        }
        (Value::Closure(a, _), Value::Closure(b, _)) => a == b,
        (Value::Function(a), Value::Function(b)) => a == b,
        (Value::NodeRef(a), Value::NodeRef(b)) => a == b,
        _ => false,
    }
}

pub fn print_layout_tree(node: &LayoutNode, indent: usize) {
    for line in format_layout_tree_lines(node, indent) {
        println!("{line}");
    }
}

pub(crate) fn format_layout_tree_lines(node: &LayoutNode, indent: usize) -> Vec<String> {
    let mut lines = vec![format_layout_line(node, indent)];
    for child in &node.children {
        lines.extend(format_layout_tree_lines(child, indent + 1));
    }
    lines
}

fn format_layout_line(node: &LayoutNode, indent: usize) -> String {
    let fmt = |v: f32| -> String {
        if v.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.2}")
        }
    };
    let mut line = format!(
        "{}:{}  row={} col={} w={} h={}",
        "  ".repeat(indent),
        node.widget_type,
        fmt(node.rect.row),
        fmt(node.rect.col),
        fmt(node.rect.width),
        fmt(node.rect.height)
    );

    for key in ["text", "value", "min", "max"] {
        if let Some(value) = node.props.get(key) {
            line.push_str("  ");
            line.push_str(key);
            line.push('=');
            match (key, value) {
                ("text", Value::String(text)) => {
                    line.push('"');
                    line.push_str(text);
                    line.push('"');
                }
                _ => line.push_str(&format_compact_value(value)),
            }
        }
    }

    line
}

fn format_compact_value(value: &Value) -> String {
    match value {
        Value::Number(n) if n.fract() == 0.0 => format!("{n:.0}"),
        Value::Number(n) => format!("{n}"),
        _ => format_lisp_value(value),
    }
}

fn clamp_size(size: Size, constraints: Constraints) -> Size {
    Size {
        width: size
            .width
            .clamp(constraints.min_width, constraints.max_width),
        height: size
            .height
            .clamp(constraints.min_height, constraints.max_height),
    }
}

fn clamp_size_for_node(node: &Value, size: Size, constraints: Constraints) -> Size {
    let unclamped_width = get_map(node)
        .and_then(|map| map.get("no-clamp-width").cloned())
        .is_some_and(|value| matches!(value, Value::Bool(true)));
    Size {
        width: if unclamped_width {
            size.width.max(constraints.min_width)
        } else {
            size.width.clamp(constraints.min_width, constraints.max_width)
        },
        height: size
            .height
            .clamp(constraints.min_height, constraints.max_height),
    }
}

/// Shrink constraints by separate x and y padding (for aspect-corrected padding).
pub(crate) fn shrink_constraints_xy(constraints: Constraints, pad_x: f32, pad_y: f32) -> Constraints {
    Constraints {
        min_width: 0.0,
        max_width: (constraints.max_width - pad_x * 2.0).max(0.0),
        min_height: 0.0,
        max_height: (constraints.max_height - pad_y * 2.0).max(0.0),
        aspect: constraints.aspect,
    }
}

fn collect_props(v: &Value) -> HashMap<String, Value> {
    let Some(map) = get_map(v) else {
        return HashMap::new();
    };

    map.iter()
        .filter(|(key, _)| key.as_str() != "type" && key.as_str() != "children")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub(crate) fn get_map(v: &Value) -> Option<HashMap<String, Value>> {
    match v {
        Value::Map(map) => Some(
            map.iter()
                .map(|(key, value)| (key.clone(), value.borrow().clone()))
                .collect(),
        ),
        _ => None,
    }
}

pub(crate) fn get_widget_type(v: &Value) -> Option<String> {
    let map = get_map(v)?;
    match map.get("type") {
        Some(Value::Keyword(widget_type)) => Some(widget_type.clone()),
        Some(Value::String(widget_type)) => Some(widget_type.clone()),
        _ => None,
    }
}

pub(crate) fn get_children(v: &Value) -> Vec<Value> {
    let Some(map) = get_map(v) else {
        return vec![];
    };

    match map.get("children") {
        Some(Value::List(children)) => children
            .iter()
            .map(|child| child.borrow().clone())
            .collect(),
        _ => vec![],
    }
}

pub(crate) fn get_prop_num(v: &Value, key: &str) -> Option<f64> {
    let map = get_map(v)?;
    match map.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

pub(crate) fn get_prop_str(v: &Value, key: &str) -> Option<String> {
    let map = get_map(v)?;
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn f64_to_f32(n: f64) -> f32 {
    if !n.is_finite() || n <= 0.0 {
        0.0
    } else {
        n as f32
    }
}

pub(crate) fn usize_to_f32(value: usize) -> f32 {
    value as f32
}

fn measure_builtin_leaf(node: &Value, widget_type: &str, aspect: f32) -> Size {
    match widget_type {
        "knob" => Size {
            width: 5.0,
            height: 5.0,
        },
        "meter" => Size {
            width: 2.0,
            height: 8.0,
        },
        "text-input" => Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(20.0),
            height: aspect,
        },
        "select" => {
            let width = match get_map(node).and_then(|map| map.get("options").cloned()) {
                Some(Value::List(items)) => items
                    .iter()
                    .filter_map(|item| match &*item.borrow() {
                        Value::String(s) => Some(s.chars().count()),
                        Value::Keyword(s) => Some(s.chars().count() + 1),
                        Value::Symbol(s) => Some(s.chars().count()),
                        _ => None,
                    })
                    .max()
                    .map(usize_to_f32)
                    .unwrap_or(8.0),
                _ => 8.0,
            };
            Size {
                width,
                height: aspect,
            }
        }
        _ => Size {
            width: 0.0,
            height: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::build_widget;
    use crate::vm::Value;

    /// Helper: keyword value
    fn kw(s: &str) -> Value {
        Value::Keyword(s.to_string())
    }

    /// Helper: number value
    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    /// Helper: string value
    fn s(text: &str) -> Value {
        Value::String(text.to_string())
    }

    /// Build a label widget: (label "text" :width w)
    fn label(text: &str, width: Option<f64>) -> Value {
        let mut args = vec![s(text)];
        if let Some(w) = width {
            args.push(kw("width"));
            args.push(num(w));
        }
        build_widget("label", args)
    }

    /// Build a hslider: (hslider :min 0 :max 1 :value 0.5)
    fn hslider() -> Value {
        build_widget("hslider", vec![kw("min"), num(0.0), kw("max"), num(1.0), kw("value"), num(0.5)])
    }

    /// Build a vslider: (vslider :height h)
    fn vslider(height: f64) -> Value {
        build_widget("vslider", vec![kw("height"), num(height)])
    }

    /// Build a box: (box :width w :height h children...)
    fn bx(width: Option<f64>, height: Option<f64>, children: Vec<Value>) -> Value {
        let mut args = Vec::new();
        if let Some(w) = width {
            args.push(kw("width"));
            args.push(num(w));
        }
        if let Some(h) = height {
            args.push(kw("height"));
            args.push(num(h));
        }
        for child in children {
            args.push(child);
        }
        build_widget("box", args)
    }

    /// Build a v-stack: (v-stack :padding p :gap g children...)
    fn vstack(padding: f64, gap: f64, children: Vec<Value>) -> Value {
        let mut args = vec![kw("padding"), num(padding), kw("gap"), num(gap)];
        for child in children {
            args.push(child);
        }
        build_widget("v-stack", args)
    }

    /// Build a h-stack: (h-stack :gap g children...)
    fn hstack(gap: f64, children: Vec<Value>) -> Value {
        let mut args = vec![kw("gap"), num(gap)];
        for child in children {
            args.push(child);
        }
        build_widget("h-stack", args)
    }

    /// Build a grid: (grid :cols c :col-width w children...)
    fn grid(cols: f64, col_width: f64, children: Vec<Value>) -> Value {
        let mut args = vec![kw("cols"), num(cols), kw("col-width"), num(col_width)];
        for child in children {
            args.push(child);
        }
        build_widget("grid", args)
    }

    // ── Tests ──────────────────────────────────────────────────────────

    #[test]
    fn natural_width_simple_vstack_fits_viewport() {
        // A v-stack with children narrower than the 80-col viewport.
        let tree = vstack(1.0, 1.0, vec![
            label("hello", Some(10.0)),
            hslider(),
        ]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);
        // hslider default width=16, plus vstack padding 1*2 = 18
        assert_eq!(natural, 18.0, "simple layout should fit in viewport");
        // max_scroll should be 0
        assert!(natural <= 80.0, "natural width should not exceed viewport");
    }

    #[test]
    fn natural_width_grid_16_cols() {
        // Grid with 16 columns, col-width 3 — mirrors the step sequencer grid
        let children: Vec<Value> = (0..16)
            .map(|i| vstack(0.0, 0.5, vec![
                vslider(4.0),
                bx(Some(3.0), Some(1.5), vec![]),
                label(&format!("{}", i + 1), None),
            ]))
            .collect();
        let tree = vstack(1.0, 1.0, vec![
            grid(16.0, 3.0, children),
        ]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);
        // grid: 16 * 3 = 48, + vstack padding 2 = 50
        assert_eq!(natural, 50.0, "grid 16x3 + padding should be 50");
    }

    #[test]
    fn natural_width_sequencer_layout_fits_wide_viewport() {
        // Mirrors the full sequencer layout from metal-seq-grid.lisp
        // at a wide viewport (content should fit → no scroll needed)
        let transport = hstack(1.0, vec![
            bx(Some(4.0), Some(3.0), vec![]),  // play button
            bx(Some(40.0), Some(3.0), vec![    // LED panel
                label("1 | 4 | 4  BPM 120", Some(32.0)),
            ]),
        ]);
        let param_tabs = hstack(0.5, vec![
            bx(Some(8.0), Some(2.0), vec![label("vel", None)]),
            bx(Some(8.0), Some(2.0), vec![label("dur", None)]),
            bx(Some(8.0), Some(2.0), vec![label("xpose", None)]),
            bx(Some(8.0), Some(2.0), vec![label("pan", None)]),
        ]);
        let step_grid = grid(16.0, 3.0, (0..16).map(|i| {
            vstack(0.0, 0.5, vec![
                vslider(4.0),
                bx(Some(3.0), Some(1.5), vec![]),
                label(&format!("{}", i + 1), None),
            ])
        }).collect());
        let mixer_rows: Vec<Value> = ["Kick", "Snare", "Hat"]
            .iter()
            .map(|name| hstack(1.0, vec![
                bx(Some(14.0), Some(1.0), vec![label(name, None)]),
                bx(None, None, vec![hslider()]),  // flex=1 in real code, natural = hslider width
            ]))
            .collect();
        let mixer = vstack(0.0, 0.5, mixer_rows);
        let effects = hstack(1.0, vec![
            bx(Some(20.0), None, vec![
                vstack(0.0, 0.5, vec![
                    label("Filter", None),
                    hstack(0.5, vec![label("cutoff", Some(8.0)), bx(Some(10.0), None, vec![hslider()])]),
                ]),
            ]),
            bx(Some(20.0), None, vec![
                vstack(0.0, 0.5, vec![
                    label("Delay", None),
                    hstack(0.5, vec![label("wet", Some(8.0)), bx(Some(10.0), None, vec![hslider()])]),
                ]),
            ]),
        ]);

        let tree = vstack(1.0, 1.0, vec![
            transport, param_tabs, step_grid, mixer, effects,
        ]);

        let engine = LayoutEngine::new(80, 60, 1.0);
        let natural = engine.natural_content_width(&tree);

        // The widest row should be the grid: 16*3=48, + padding 2 = 50
        // Or transport: 4 + 40 + 1 gap = 45, + padding 2 = 47
        // Or effects: 20 + 20 + 1 gap = 41, + padding 2 = 43
        // Or param tabs: 8*4 + 0.5*3 gaps = 33.5, + padding 2 = 35.5
        // Or mixer: 14 + 16 + 1 gap = 31, + padding 2 = 33
        // So natural_width should be 50 (grid is widest)
        assert_eq!(natural, 50.0, "natural width should be driven by the grid");
        assert!(natural <= 80.0, "content should fit in 80-col viewport");
    }

    #[test]
    fn natural_width_exceeds_narrow_viewport() {
        // Same layout but in a narrow viewport — natural width > viewport → scroll needed
        let step_grid = grid(16.0, 3.0, (0..16).map(|i| {
            vstack(0.0, 0.5, vec![
                vslider(4.0),
                bx(Some(3.0), Some(1.5), vec![]),
                label(&format!("{}", i + 1), None),
            ])
        }).collect());
        let tree = vstack(1.0, 1.0, vec![step_grid]);

        let engine = LayoutEngine::new(40, 24, 1.0);
        let natural = engine.natural_content_width(&tree);

        // grid: 48 + padding 2 = 50, viewport 40
        assert_eq!(natural, 50.0, "natural width same regardless of viewport");
        assert!(natural > 40.0, "content should exceed narrow viewport → scroll needed");
    }

    #[test]
    fn natural_width_long_labels_do_not_inflate() {
        // Labels with long text inside fixed-width boxes should NOT inflate natural width
        // beyond what the layout structure specifies.
        // (In TUI mode, label measures as char count if no explicit width)
        let mixer_rows: Vec<Value> = [
            "LS301-808ii-FC-Maraca-3-Extended-Name-Very-Long",
            "_12'' Augustus Pablo - King Tubby Meets",
        ].iter().map(|name| {
            hstack(1.0, vec![
                bx(Some(14.0), Some(1.0), vec![label(name, None)]),
                bx(Some(20.0), None, vec![hslider()]),
            ])
        }).collect();
        let tree = vstack(1.0, 0.5, mixer_rows);

        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);

        // box widths: 14 + 20 + 1 gap = 35, + padding 2 = 37
        // The long label text should NOT push beyond 37 because its box is fixed at 14
        assert_eq!(natural, 37.0, "fixed-width boxes should contain long labels");
    }

    #[test]
    fn natural_width_label_without_box_uses_text_width() {
        // A bare label (not in a fixed-width box) SHOULD use its text width
        let tree = vstack(0.0, 0.0, vec![
            label("short", None),
            label("a much longer label text here", None),
        ]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);
        // In TUI mode (no TextMeasurer), label width = char count
        // "a much longer label text here" = 29 chars
        assert_eq!(natural, 29.0, "bare label should use text char count as width");
    }
}
