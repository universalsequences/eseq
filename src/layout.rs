use std::collections::HashMap;

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
    pub widget_type: String,
    pub rect: Rect,
    pub props: HashMap<String, Value>,
    pub children: Vec<LayoutNode>,
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
        Some(self.build_layout_node(
            tree,
            Rect {
                row: 0,
                col: 0,
                width: size.width,
                height: size.height,
            },
        ))
    }

    fn measure(&self, node: &Value, constraints: Constraints) -> Option<Size> {
        let widget_type = get_widget_type(node)?;
        let children = get_children(node);

        let size = match widget_type.as_str() {
            "label" => Size {
                width: get_prop_str(node, "text")
                    .map(|text| saturating_usize_to_u16(text.chars().count()))
                    .unwrap_or(0),
                height: 1,
            },
            "slider" | "hslider" => Size {
                width: get_prop_num(node, "width").map(f64_to_u16).unwrap_or(16),
                height: 1,
            },
            "vslider" => Size {
                width: 2,
                height: get_prop_num(node, "height").map(f64_to_u16).unwrap_or(8),
            },
            "toggle" => Size {
                width: 4,
                height: 1,
            },
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
            "v-stack" => {
                let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
                let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(0);
                let inner = shrink_constraints(constraints, padding);
                let child_sizes = children
                    .iter()
                    .filter_map(|child| self.measure(child, inner))
                    .collect::<Vec<_>>();
                let width = child_sizes.iter().map(|size| size.width).max().unwrap_or(0);
                let height = child_sizes
                    .iter()
                    .map(|size| size.height)
                    .fold(0_u16, saturating_add)
                    .saturating_add(gap.saturating_mul(child_sizes.len().saturating_sub(1) as u16));
                Size {
                    width: constraints
                        .max_width
                        .max(width.saturating_add(padding.saturating_mul(2))),
                    height: height.saturating_add(padding.saturating_mul(2)),
                }
            }
            "h-stack" => {
                let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
                let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(1);
                let inner = shrink_constraints(constraints, padding);
                let child_sizes = children
                    .iter()
                    .filter_map(|child| self.measure(child, inner))
                    .collect::<Vec<_>>();
                let width = child_sizes
                    .iter()
                    .map(|size| size.width)
                    .fold(0_u16, saturating_add)
                    .saturating_add(gap.saturating_mul(child_sizes.len().saturating_sub(1) as u16));
                let height = child_sizes
                    .iter()
                    .map(|size| size.height)
                    .max()
                    .unwrap_or(0);
                Size {
                    width: constraints
                        .max_width
                        .max(width.saturating_add(padding.saturating_mul(2))),
                    height: height.saturating_add(padding.saturating_mul(2)),
                }
            }
            "box" => {
                let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
                let inner = shrink_constraints(constraints, padding);
                let child_size = children
                    .first()
                    .and_then(|child| self.measure(child, inner));
                Size {
                    width: get_prop_num(node, "width")
                        .map(f64_to_u16)
                        .unwrap_or_else(|| {
                            child_size
                                .map(|size| size.width.saturating_add(padding.saturating_mul(2)))
                                .unwrap_or(padding.saturating_mul(2))
                        }),
                    height: get_prop_num(node, "height")
                        .map(f64_to_u16)
                        .unwrap_or_else(|| {
                            child_size
                                .map(|size| size.height.saturating_add(padding.saturating_mul(2)))
                                .unwrap_or(padding.saturating_mul(2))
                        }),
                }
            }
            "grid" => {
                let cols = get_prop_num(node, "cols")
                    .map(f64_to_u16)
                    .unwrap_or(1)
                    .max(1);
                let measured_children = children
                    .iter()
                    .filter_map(|child| self.measure(child, constraints))
                    .collect::<Vec<_>>();
                let col_width = get_prop_num(node, "col-width")
                    .map(f64_to_u16)
                    .unwrap_or_else(|| (constraints.max_width / cols).max(1));
                let row_height = get_prop_num(node, "row-height")
                    .map(f64_to_u16)
                    .unwrap_or_else(|| {
                        measured_children
                            .iter()
                            .map(|size| size.height)
                            .max()
                            .unwrap_or(1)
                    });
                let rows = ((children.len() as u16).saturating_add(cols - 1)) / cols;
                Size {
                    width: cols.saturating_mul(col_width),
                    height: rows.saturating_mul(row_height),
                }
            }
            _ => Size {
                width: 0,
                height: 0,
            },
        };

        Some(clamp_size(size, constraints))
    }

    fn build_layout_node(&self, node: &Value, rect: Rect) -> LayoutNode {
        let widget_type = get_widget_type(node).unwrap_or_default();
        let children_values = get_children(node);
        let children = self.layout_children(node, rect, &children_values);
        LayoutNode {
            widget_type,
            rect,
            props: collect_props(node),
            children,
        }
    }

    fn layout_children(&self, node: &Value, area: Rect, children: &[Value]) -> Vec<LayoutNode> {
        let Some(widget_type) = get_widget_type(node) else {
            return vec![];
        };

        match widget_type.as_str() {
            "v-stack" => {
                let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
                let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(0);
                let inner_width = area.width.saturating_sub(padding.saturating_mul(2));
                let inner_constraints = Constraints {
                    min_width: 0,
                    max_width: inner_width,
                    min_height: 0,
                    max_height: area.height.saturating_sub(padding.saturating_mul(2)),
                };
                let mut cursor_row = area.row.saturating_add(padding);
                children
                    .iter()
                    .filter_map(|child| {
                        let size = self.measure(child, inner_constraints)?;
                        let rect = Rect {
                            row: cursor_row,
                            col: area.col.saturating_add(padding),
                            width: size.width,
                            height: size.height,
                        };
                        cursor_row = cursor_row.saturating_add(size.height).saturating_add(gap);
                        Some(self.build_layout_node(child, rect))
                    })
                    .collect()
            }
            "h-stack" => {
                let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
                let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(1);
                let inner_height = area.height.saturating_sub(padding.saturating_mul(2));
                let inner_constraints = Constraints {
                    min_width: 0,
                    max_width: area.width.saturating_sub(padding.saturating_mul(2)),
                    min_height: 0,
                    max_height: inner_height,
                };
                let mut cursor_col = area.col.saturating_add(padding);
                children
                    .iter()
                    .filter_map(|child| {
                        let size = self.measure(child, inner_constraints)?;
                        let rect = Rect {
                            row: area.row.saturating_add(padding),
                            col: cursor_col,
                            width: size.width,
                            height: size.height,
                        };
                        cursor_col = cursor_col.saturating_add(size.width).saturating_add(gap);
                        Some(self.build_layout_node(child, rect))
                    })
                    .collect()
            }
            "box" => {
                let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
                children
                    .first()
                    .map(|child| {
                        self.build_layout_node(
                            child,
                            Rect {
                                row: area.row.saturating_add(padding),
                                col: area.col.saturating_add(padding),
                                width: area.width.saturating_sub(padding.saturating_mul(2)),
                                height: area.height.saturating_sub(padding.saturating_mul(2)),
                            },
                        )
                    })
                    .into_iter()
                    .collect()
            }
            "grid" => {
                let cols = get_prop_num(node, "cols")
                    .map(f64_to_u16)
                    .unwrap_or(1)
                    .max(1);
                let col_width = get_prop_num(node, "col-width")
                    .map(f64_to_u16)
                    .unwrap_or_else(|| (area.width / cols).max(1));
                let measure_constraints = Constraints {
                    min_width: 0,
                    max_width: col_width,
                    min_height: 0,
                    max_height: area.height,
                };
                let row_height = get_prop_num(node, "row-height")
                    .map(f64_to_u16)
                    .unwrap_or_else(|| {
                        children
                            .iter()
                            .filter_map(|child| self.measure(child, measure_constraints))
                            .map(|size| size.height)
                            .max()
                            .unwrap_or(1)
                    });

                children
                    .iter()
                    .enumerate()
                    .map(|(idx, child)| {
                        let row = idx as u16 / cols;
                        let col = idx as u16 % cols;
                        self.build_layout_node(
                            child,
                            Rect {
                                row: area.row.saturating_add(row.saturating_mul(row_height)),
                                col: area.col.saturating_add(col.saturating_mul(col_width)),
                                width: col_width,
                                height: row_height,
                            },
                        )
                    })
                    .collect()
            }
            _ => vec![],
        }
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

fn shrink_constraints(constraints: Constraints, padding: u16) -> Constraints {
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

fn get_map(v: &Value) -> Option<HashMap<String, Value>> {
    match v {
        Value::Map(map) => Some(
            map.iter()
                .map(|(key, value)| (key.clone(), value.borrow().clone()))
                .collect(),
        ),
        _ => None,
    }
}

fn get_widget_type(v: &Value) -> Option<String> {
    let map = get_map(v)?;
    match map.get("type") {
        Some(Value::Keyword(widget_type)) => Some(widget_type.clone()),
        Some(Value::String(widget_type)) => Some(widget_type.clone()),
        _ => None,
    }
}

fn get_children(v: &Value) -> Vec<Value> {
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

fn get_prop_num(v: &Value, key: &str) -> Option<f64> {
    let map = get_map(v)?;
    match map.get(key) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn get_prop_str(v: &Value, key: &str) -> Option<String> {
    let map = get_map(v)?;
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn f64_to_u16(n: f64) -> u16 {
    if !n.is_finite() || n <= 0.0 {
        0
    } else if n >= u16::MAX as f64 {
        u16::MAX
    } else {
        n as u16
    }
}

fn saturating_add(a: u16, b: u16) -> u16 {
    a.saturating_add(b)
}

fn saturating_usize_to_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}
