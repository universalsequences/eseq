use std::collections::HashMap;

use crate::widget_render;
use crate::vm::{Value, format_lisp_value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub row: u16,
    pub col: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy)]
pub struct Constraints {
    pub min_width: u16,
    pub max_width: u16,
    pub min_height: u16,
    pub max_height: u16,
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

pub struct LayoutEngine {
    pub terminal_cols: u16,
    pub terminal_rows: u16,
}

impl LayoutEngine {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            terminal_cols: cols,
            terminal_rows: rows,
        }
    }

    pub fn layout(&self, tree: &Value) -> Option<LayoutNode> {
        let size = self.measure(
            tree,
            Constraints {
                min_width: 0,
                max_width: self.terminal_cols,
                min_height: 0,
                max_height: self.terminal_rows,
            },
        )?;
        let mut layout = self.build_layout_node(
            tree,
            Rect {
                row: 0,
                col: 0,
                width: size.width,
                height: size.height,
            },
        );
        let mut next_widget_id = 1;
        assign_widget_ids(&mut layout, &mut next_widget_id);
        Some(layout)
    }

    fn measure(&self, node: &Value, constraints: Constraints) -> Option<Size> {
        let widget_type = get_widget_type(node)?;
        let children = get_children(node);

        let size = if let Some(definition) = widget_render::widget_definition(&widget_type) {
            definition.measure(node, &children, constraints, &mut |child, child_constraints| {
                self.measure(child, child_constraints)
            })?
        } else {
            measure_builtin_leaf(node, &widget_type)
        };

        Some(clamp_size(size, constraints))
    }

    fn build_layout_node(&self, node: &Value, rect: Rect) -> LayoutNode {
        let widget_type = get_widget_type(node).unwrap_or_default();
        let children_values = get_children(node);
        let children = self.layout_children(node, rect, &children_values);
        let props = collect_props(node);
        let focusable = matches!(
            props.get("focusable"),
            Some(Value::Bool(true))
        );
        LayoutNode {
            widget_id: 0,
            widget_type,
            rect,
            props,
            children,
            focusable,
        }
    }

    fn layout_children(&self, node: &Value, area: Rect, children: &[Value]) -> Vec<LayoutNode> {
        let Some(widget_type) = get_widget_type(node) else {
            return vec![];
        };

        widget_render::widget_definition(&widget_type)
            .map(|definition| {
                definition.layout_children(
                    node,
                    area,
                    children,
                    &mut |child, child_constraints| self.measure(child, child_constraints),
                    &mut |child, rect| self.build_layout_node(child, rect),
                )
            })
            .unwrap_or_default()
    }
}

pub fn hit_test_layout(node: &LayoutNode, row: u16, col: u16) -> Option<&LayoutNode> {
    if !rect_contains(node.rect, row, col) {
        return None;
    }

    for child in node.children.iter().rev() {
        if let Some(hit) = hit_test_layout(child, row, col) {
            return Some(hit);
        }
    }

    if widget_render::is_layout_widget_type(&node.widget_type) {
        None
    } else {
        Some(node)
    }
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
        .map(|(child_layout, child_tree)| reuse_layout_node(child_layout, child_tree, dirty_widget_ids))
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

fn rect_contains(rect: Rect, row: u16, col: u16) -> bool {
    row >= rect.row
        && col >= rect.col
        && row < rect.row.saturating_add(rect.height)
        && col < rect.col.saturating_add(rect.width)
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
    let mut line = format!(
        "{}:{}  row={} col={} w={} h={}",
        "  ".repeat(indent),
        node.widget_type,
        node.rect.row,
        node.rect.col,
        node.rect.width,
        node.rect.height
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

pub(crate) fn shrink_constraints(constraints: Constraints, padding: u16) -> Constraints {
    Constraints {
        min_width: 0,
        max_width: constraints
            .max_width
            .saturating_sub(padding.saturating_mul(2)),
        min_height: 0,
        max_height: constraints
            .max_height
            .saturating_sub(padding.saturating_mul(2)),
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

pub(crate) fn f64_to_u16(n: f64) -> u16 {
    if !n.is_finite() || n <= 0.0 {
        0
    } else if n >= u16::MAX as f64 {
        u16::MAX
    } else {
        n as u16
    }
}

pub(crate) fn saturating_usize_to_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn measure_builtin_leaf(node: &Value, widget_type: &str) -> Size {
    match widget_type {
        "knob" => Size {
            width: 5,
            height: 3,
        },
        "meter" => Size {
            width: 2,
            height: 8,
        },
        "text-input" => Size {
            width: get_prop_num(node, "width").map(f64_to_u16).unwrap_or(20),
            height: 1,
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
                    .map(saturating_usize_to_u16)
                    .unwrap_or(8),
                _ => 8,
            };
            Size { width, height: 1 }
        }
        _ => Size {
            width: 0,
            height: 0,
        },
    }
}
