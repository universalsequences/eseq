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
        let mut layout = self.build_layout_node(
            tree,
            Rect {
                row: 0.0,
                col: 0.0,
                width: size.width,
                height: size.height,
            },
            DEFAULT_FONT_SIZE,
        );
        let mut next_widget_id = 1;
        assign_widget_ids(&mut layout, &mut next_widget_id);
        Some(layout)
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

        Some(clamp_size(size, constraints))
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
                    &mut |child, mut child_constraints| {
                        child_constraints.aspect = self.aspect;
                        self.measure(child, child_constraints, font_size)
                    },
                    &mut |child, rect| self.build_layout_node(child, rect, font_size),
                )
            })
            .unwrap_or_default()
    }
}

pub fn hit_test_layout(node: &LayoutNode, row: f32, col: f32) -> Option<&LayoutNode> {
    // Container nodes: always recurse into children — their rects may be
    // clamped to the viewport while children extend beyond (scroll).
    for child in node.children.iter().rev() {
        if let Some(hit) = hit_test_layout(child, row, col) {
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
        if node.props.contains_key("on-change") || node.props.contains_key("bind") {
            return Some(node);
        }
    }

    None
}

pub fn reuse_layout_node(
    existing: &LayoutNode,
    tree: &Value,
    dirty_widget_ids: &mut Vec<u64>,
) -> Option<LayoutNode> {
    let widget_type = get_widget_type(tree)?;
    if widget_type != existing.widget_type {
        return None;
    }

    let children_values = get_children(tree);
    if children_values.len() != existing.children.len() {
        return None;
    }

    let new_props = collect_props(tree);
    if !size_affecting_props_equal(&widget_type, &existing.props, &new_props) {
        return None;
    }

    if existing.props != new_props {
        dirty_widget_ids.push(existing.widget_id);
    }

    let children = existing
        .children
        .iter()
        .zip(children_values.iter())
        .map(|(child_layout, child_tree)| {
            reuse_layout_node(child_layout, child_tree, dirty_widget_ids)
        })
        .collect::<Option<Vec<_>>>()?;

    let focusable = matches!(new_props.get("focusable"), Some(Value::Bool(true)));
    Some(LayoutNode {
        widget_id: existing.widget_id,
        widget_type,
        rect: existing.rect,
        props: new_props,
        children,
        focusable,
    })
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

pub(crate) fn shrink_constraints(constraints: Constraints, padding: f32) -> Constraints {
    Constraints {
        min_width: 0.0,
        max_width: (constraints.max_width - padding * 2.0).max(0.0),
        min_height: 0.0,
        max_height: (constraints.max_height - padding * 2.0).max(0.0),
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
