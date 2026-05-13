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
    pub stable_widget_id: Option<u64>,
    pub subtree_root_id: Option<u64>,
    pub parent_subtree_root_id: Option<u64>,
    pub stable_key: Option<String>,
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
        self.layout_with_id_offset(tree, 0)
    }

    pub fn layout_with_id_offset(&self, tree: &Value, widget_id_offset: u64) -> Option<LayoutNode> {
        let size = self.measure(
            tree,
            Constraints {
                min_width: 0.0,
                max_width: self.terminal_cols as f32,
                min_height: 0.0,
                max_height: f32::INFINITY,
                aspect: self.aspect,
            },
            DEFAULT_FONT_SIZE,
        )?;
        let root_width = if prop_is_keyword(tree, "width", "fill") {
            self.terminal_cols as f32
        } else {
            size.width
        };
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
                width: root_width,
                height: root_height,
            },
            DEFAULT_FONT_SIZE,
        );
        let mut next_widget_id = widget_id_offset.wrapping_add(1);
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
                max_width: f32::INFINITY,
                min_height: 0.0,
                max_height: f32::INFINITY,
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
            let content_height = children.first().map(|c| c.rect.height).unwrap_or(0.0);
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
        let stable_widget_id = get_prop_u64(node, "__stable-widget-id");
        let subtree_root_id = get_prop_u64(node, "__subtree-root-id");
        let parent_subtree_root_id = get_prop_u64(node, "__parent-subtree-root-id");
        let stable_key = get_prop_str(node, "__stable-key");
        LayoutNode {
            widget_id: 0,
            stable_widget_id,
            subtree_root_id,
            parent_subtree_root_id,
            stable_key,
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
        || node.props.contains_key("on-mouse-up")
        || widget_render::widget_captures_drag(&node.widget_type)
}

pub fn hit_test_layout(node: &LayoutNode, row: f32, col: f32) -> Option<&LayoutNode> {
    // Scroll containers: only hit-test within viewport rect, and adjust
    // coordinates by scroll offset before recursing into children.
    if node.widget_type == "scroll" {
        if !rect_contains(node.rect, row, col) {
            return None;
        }
        let state =
            widget_render::scroll::get_scroll_state(widget_render::scroll::scroll_state_key(node));
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
        if node.props.contains_key("on-change")
            || node.props.contains_key("bind")
            || node.props.contains_key("on-click")
            || node.props.contains_key("on-drag")
            || node.props.contains_key("on-mouse-down")
            || node.props.contains_key("on-mouse-up")
        {
            return Some(node);
        }
    }

    None
}

pub fn layout_contains_widget_id(node: &LayoutNode, widget_id: u64) -> bool {
    node.widget_id == widget_id
        || node
            .children
            .iter()
            .any(|child| layout_contains_widget_id(child, widget_id))
}

fn reuse_layout_node_impl(
    existing: &LayoutNode,
    tree: &Value,
    dirty_widget_ids: &mut Vec<u64>,
    path: &mut Vec<String>,
) -> Result<LayoutNode, String> {
    let format_opt_u64 = |value: Option<u64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    };
    let format_opt_str = |value: Option<&str>| value.unwrap_or("-").to_string();
    let format_reason = |reason: String, path: &[String]| {
        if path.is_empty() {
            reason
        } else {
            format!("{reason}@{}", path.join(">"))
        }
    };
    let widget_type = get_widget_type(tree).ok_or_else(|| "not-widget".to_string())?;
    if widget_type != existing.widget_type {
        return Err(format_reason(
            format!("widget-type:{}->{}", existing.widget_type, widget_type),
            path,
        ));
    }
    let new_stable_widget_id = get_prop_u64(tree, "__stable-widget-id");
    let new_subtree_root_id = get_prop_u64(tree, "__subtree-root-id");
    let new_parent_subtree_root_id = get_prop_u64(tree, "__parent-subtree-root-id");
    let new_stable_key = get_prop_str(tree, "__stable-key");
    let is_explicit_subtree_root =
        existing.subtree_root_id.is_some() && existing.subtree_root_id == new_subtree_root_id;
    let identity_mismatch = if is_explicit_subtree_root {
        existing.subtree_root_id != new_subtree_root_id || existing.stable_key != new_stable_key
    } else {
        existing.stable_widget_id != new_stable_widget_id
            || existing.subtree_root_id != new_subtree_root_id
            || existing.parent_subtree_root_id != new_parent_subtree_root_id
            || existing.stable_key != new_stable_key
    };
    if identity_mismatch {
        return Err(format_reason(
            format!(
                "stable-identity:{widget_type}:wid:{}->{}:root:{}->{}:parent:{}->{}:key:{}->{}",
                format_opt_u64(existing.stable_widget_id),
                format_opt_u64(new_stable_widget_id),
                format_opt_u64(existing.subtree_root_id),
                format_opt_u64(new_subtree_root_id),
                format_opt_u64(existing.parent_subtree_root_id),
                format_opt_u64(new_parent_subtree_root_id),
                format_opt_str(existing.stable_key.as_deref()),
                format_opt_str(new_stable_key.as_deref()),
            ),
            path,
        ));
    }
    // Tree widgets manage internal expand/collapse state that changes their
    // height without changing props. Always force full relayout.
    if widget_type == "tree" {
        return Err(format_reason("tree-widget".to_string(), path));
    }

    let children_values = get_children(tree);
    let effective_children_values: Vec<&Value> = if widget_type == "tabs" {
        let selected = (get_prop_num(tree, "value").map(f64_to_f32).unwrap_or(0.0) as usize)
            .min(children_values.len().saturating_sub(1));
        children_values.get(selected).into_iter().collect()
    } else {
        children_values.iter().collect()
    };

    if effective_children_values.len() != existing.children.len() {
        let old_children = existing
            .children
            .iter()
            .map(|child| child.widget_type.clone())
            .collect::<Vec<_>>()
            .join(",");
        let new_children = effective_children_values
            .iter()
            .map(|child| get_widget_type(child).unwrap_or_else(|| "non-widget".to_string()))
            .collect::<Vec<_>>()
            .join(",");
        return Err(format_reason(
            format!(
                "children-len:{}:{}->{}:[{}]->[{}]",
                widget_type,
                existing.children.len(),
                effective_children_values.len(),
                old_children,
                new_children
            ),
            path,
        ));
    }

    let mut new_props = collect_props(tree);
    preserve_layout_internal_props(&existing.props, &mut new_props);
    if !size_affecting_props_equal(&widget_type, &existing.props, &new_props) {
        return Err(format_reason(format!("size-props:{widget_type}"), path));
    }

    if existing.props != new_props {
        dirty_widget_ids.push(existing.widget_id);
    }

    let children = existing
        .children
        .iter()
        .zip(effective_children_values.iter())
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
        stable_widget_id: existing.stable_widget_id,
        subtree_root_id: existing.subtree_root_id,
        parent_subtree_root_id: existing.parent_subtree_root_id,
        stable_key: existing.stable_key.clone(),
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

pub fn reuse_layout_node_for_subtree(
    existing: &LayoutNode,
    tree: &Value,
    subtree_root_id: u64,
    dirty_widget_ids: &mut Vec<u64>,
) -> Option<LayoutNode> {
    let mut child_path = Vec::new();
    find_subtree_path(existing, subtree_root_id, &mut child_path)?;
    reuse_layout_node_for_subtree_path(existing, tree, &child_path, dirty_widget_ids)
}

pub fn subtree_root_paths(existing: &LayoutNode) -> HashMap<u64, Vec<usize>> {
    let mut paths = HashMap::new();
    let mut path = Vec::new();
    collect_subtree_root_paths(existing, &mut path, &mut paths);
    paths
}

fn collect_subtree_root_paths(
    node: &LayoutNode,
    path: &mut Vec<usize>,
    paths: &mut HashMap<u64, Vec<usize>>,
) {
    if let Some(root_id) = node.subtree_root_id {
        paths.insert(root_id, path.clone());
    }
    for (idx, child) in node.children.iter().enumerate() {
        path.push(idx);
        collect_subtree_root_paths(child, path, paths);
        path.pop();
    }
}

pub fn reuse_layout_node_for_subtree_path(
    existing: &LayoutNode,
    tree: &Value,
    child_path: &[usize],
    dirty_widget_ids: &mut Vec<u64>,
) -> Option<LayoutNode> {
    reuse_layout_node_for_subtree_path_result(existing, tree, child_path, dirty_widget_ids).ok()
}

pub fn reuse_layout_node_for_subtree_path_result(
    existing: &LayoutNode,
    tree: &Value,
    child_path: &[usize],
    dirty_widget_ids: &mut Vec<u64>,
) -> Result<LayoutNode, String> {
    let mut trace_path = Vec::new();
    reuse_layout_node_at_path(
        existing,
        tree,
        child_path,
        dirty_widget_ids,
        &mut trace_path,
    )
}

fn find_subtree_path(node: &LayoutNode, subtree_root_id: u64, path: &mut Vec<usize>) -> Option<()> {
    if node.subtree_root_id == Some(subtree_root_id) {
        return Some(());
    }
    for (idx, child) in node.children.iter().enumerate() {
        path.push(idx);
        if find_subtree_path(child, subtree_root_id, path).is_some() {
            return Some(());
        }
        path.pop();
    }
    None
}

fn reuse_layout_node_at_path(
    existing: &LayoutNode,
    tree: &Value,
    child_path: &[usize],
    dirty_widget_ids: &mut Vec<u64>,
    trace_path: &mut Vec<String>,
) -> Result<LayoutNode, String> {
    if child_path.is_empty() {
        return reuse_layout_node_impl(existing, tree, dirty_widget_ids, trace_path);
    }

    let widget_type = get_widget_type(tree).ok_or_else(|| "not-widget".to_string())?;
    if widget_type != existing.widget_type {
        return Err(format!(
            "widget-type:{}->{}",
            existing.widget_type, widget_type
        ));
    }

    let new_stable_widget_id = get_prop_u64(tree, "__stable-widget-id");
    let new_subtree_root_id = get_prop_u64(tree, "__subtree-root-id");
    let new_parent_subtree_root_id = get_prop_u64(tree, "__parent-subtree-root-id");
    let new_stable_key = get_prop_str(tree, "__stable-key");
    let is_explicit_subtree_root =
        existing.subtree_root_id.is_some() && existing.subtree_root_id == new_subtree_root_id;
    let identity_mismatch = if is_explicit_subtree_root {
        existing.subtree_root_id != new_subtree_root_id || existing.stable_key != new_stable_key
    } else {
        existing.stable_widget_id != new_stable_widget_id
            || existing.subtree_root_id != new_subtree_root_id
            || existing.parent_subtree_root_id != new_parent_subtree_root_id
            || existing.stable_key != new_stable_key
    };
    if identity_mismatch {
        return Err(format!("stable-identity:{widget_type}"));
    }

    let mut new_props = collect_props(tree);
    preserve_layout_internal_props(&existing.props, &mut new_props);
    if !size_affecting_props_equal(&widget_type, &existing.props, &new_props) {
        return Err(format!("size-props:{widget_type}"));
    }
    if existing.props != new_props {
        dirty_widget_ids.push(existing.widget_id);
    }

    let children_values = get_children(tree);
    let effective_children_values: Vec<&Value> = if widget_type == "tabs" {
        let selected = (get_prop_num(tree, "value").map(f64_to_f32).unwrap_or(0.0) as usize)
            .min(children_values.len().saturating_sub(1));
        children_values.get(selected).into_iter().collect()
    } else {
        children_values.iter().collect()
    };
    if effective_children_values.len() != existing.children.len() {
        return Err(format!("children-len:{widget_type}"));
    }

    let child_idx = child_path[0];
    let child_layout = existing
        .children
        .get(child_idx)
        .ok_or_else(|| format!("missing-layout-child:{widget_type}[{child_idx}]"))?;
    let child_tree = effective_children_values
        .get(child_idx)
        .ok_or_else(|| format!("missing-tree-child:{widget_type}[{child_idx}]"))?;
    trace_path.push(format!("{widget_type}[{child_idx}]"));
    let updated_child = reuse_layout_node_at_path(
        child_layout,
        child_tree,
        &child_path[1..],
        dirty_widget_ids,
        trace_path,
    )?;
    trace_path.pop();

    let mut children = existing.children.clone();
    children[child_idx] = updated_child;
    let focusable = matches!(new_props.get("focusable"), Some(Value::Bool(true)));
    Ok(LayoutNode {
        widget_id: existing.widget_id,
        stable_widget_id: existing.stable_widget_id,
        subtree_root_id: existing.subtree_root_id,
        parent_subtree_root_id: existing.parent_subtree_root_id,
        stable_key: existing.stable_key.clone(),
        widget_type,
        rect: existing.rect,
        props: new_props,
        children,
        focusable,
    })
}

pub fn reuse_layout_failure_reason(existing: &LayoutNode, tree: &Value) -> Option<String> {
    let mut dirty_widget_ids = Vec::new();
    let mut path = Vec::new();
    reuse_layout_node_impl(existing, tree, &mut dirty_widget_ids, &mut path).err()
}

fn preserve_layout_internal_props(
    existing_props: &HashMap<String, Value>,
    new_props: &mut HashMap<String, Value>,
) {
    for (key, value) in existing_props {
        if key.starts_with('_') && !new_props.contains_key(key) {
            new_props.insert(key.clone(), value.clone());
        }
    }
}

pub fn same_layout_geometry(left: &LayoutNode, right: &LayoutNode) -> bool {
    left.widget_type == right.widget_type
        && left.stable_widget_id == right.stable_widget_id
        && left.subtree_root_id == right.subtree_root_id
        && left.parent_subtree_root_id == right.parent_subtree_root_id
        && left.stable_key == right.stable_key
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
        let height_equal = value_option_eq(old_props.get("height"), new_props.get("height"));
        let font_size_equal =
            value_option_eq(old_props.get("font-size"), new_props.get("font-size"));
        let wrap_equal = value_option_eq(old_props.get("wrap"), new_props.get("wrap"));
        let width_locked = old_props.contains_key("width") || new_props.contains_key("width");
        return width_equal
            && height_equal
            && font_size_equal
            && wrap_equal
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
        (
            Value::ReactiveRef {
                namespace: a_ns,
                field: a_field,
                index: a_index,
                kind: a_kind,
                ..
            },
            Value::ReactiveRef {
                namespace: b_ns,
                field: b_field,
                index: b_index,
                kind: b_kind,
                ..
            },
        ) => a_ns == b_ns && a_field == b_field && a_index == b_index && a_kind == b_kind,
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
    let has_explicit_width = get_prop_num(node, "width").is_some();
    Size {
        width: if has_explicit_width {
            size.width.max(constraints.min_width)
        } else {
            size.width
                .clamp(constraints.min_width, constraints.max_width)
        },
        height: size
            .height
            .clamp(constraints.min_height, constraints.max_height),
    }
}

/// Shrink constraints by separate x and y padding (for aspect-corrected padding).
pub(crate) fn shrink_constraints_xy(
    constraints: Constraints,
    pad_x: f32,
    pad_y: f32,
) -> Constraints {
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

pub(crate) fn get_prop_keyword(v: &Value, key: &str) -> Option<String> {
    let map = get_map(v)?;
    match map.get(key) {
        Some(Value::Keyword(s)) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn prop_is_keyword(v: &Value, key: &str, expected: &str) -> bool {
    get_prop_keyword(v, key).is_some_and(|value| value == expected)
}

pub(crate) fn get_prop_u64(v: &Value, key: &str) -> Option<u64> {
    let map = get_map(v)?;
    match map.get(key) {
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
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
    use crate::vm::Value;
    use crate::widget_render::{WidgetDefinition, WidgetKeyEvent};
    use crate::widgets::build_widget;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::cell::RefCell;
    use std::rc::Rc;

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

    fn assert_num_approx(actual: Option<f64>, expected: f64) {
        let actual = actual.expect("expected numeric value");
        assert!(
            (actual - expected).abs() < 0.000_01,
            "expected {expected}, got {actual}"
        );
    }

    fn assert_f32_approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.000_01,
            "expected {expected}, got {actual}"
        );
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

    fn wrapped_label(text: &str, width: Option<f64>) -> Value {
        let mut args = vec![s(text), kw("wrap"), Value::Bool(true)];
        if let Some(w) = width {
            args.push(kw("width"));
            args.push(num(w));
        }
        build_widget("label", args)
    }

    /// Build a hslider: (hslider :min 0 :max 1 :value 0.5)
    fn hslider() -> Value {
        build_widget(
            "hslider",
            vec![
                kw("min"),
                num(0.0),
                kw("max"),
                num(1.0),
                kw("value"),
                num(0.5),
            ],
        )
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

    /// Build a h-stack: (h-stack :width :fill :gap g children...)
    fn hstack_fill(gap: f64, children: Vec<Value>) -> Value {
        let mut args = vec![kw("width"), kw("fill"), kw("gap"), num(gap)];
        for child in children {
            args.push(child);
        }
        build_widget("h-stack", args)
    }

    /// Build a box: (box :flex f children...)
    fn flex_box(flex: f64, children: Vec<Value>) -> Value {
        let mut args = vec![kw("flex"), num(flex)];
        for child in children {
            args.push(child);
        }
        build_widget("box", args)
    }

    fn flex_text_input(flex: f64, placeholder: &str) -> Value {
        build_widget(
            "text-input",
            vec![kw("placeholder"), s(placeholder), kw("flex"), num(flex)],
        )
    }

    fn button(text: &str) -> Value {
        build_widget("button", vec![s(text)])
    }

    /// Build a grid: (grid :cols c :col-width w children...)
    fn grid(cols: f64, col_width: f64, children: Vec<Value>) -> Value {
        let mut args = vec![kw("cols"), num(cols), kw("col-width"), num(col_width)];
        for child in children {
            args.push(child);
        }
        build_widget("grid", args)
    }

    fn scroll_with_tall_box(background: &str) -> Value {
        build_widget(
            "scroll",
            vec![
                kw("width"),
                num(20.0),
                kw("height"),
                num(5.0),
                build_widget(
                    "box",
                    vec![
                        kw("width"),
                        num(20.0),
                        kw("height"),
                        num(20.0),
                        kw("background-color"),
                        s(background),
                    ],
                ),
            ],
        )
    }

    fn sticky_scroll_with_content_height(stable_id: f64, content_height: f64) -> Value {
        build_widget(
            "scroll",
            vec![
                kw("__stable-widget-id"),
                num(stable_id),
                kw("width"),
                num(20.0),
                kw("height"),
                num(3.0),
                kw("stick-to-bottom"),
                Value::Bool(true),
                build_widget(
                    "box",
                    vec![kw("width"), num(20.0), kw("height"), num(content_height)],
                ),
            ],
        )
    }

    fn value_cell(value: Value) -> Rc<RefCell<Value>> {
        Rc::new(RefCell::new(value))
    }

    fn tree_item(label: &str, children: Vec<Value>) -> Value {
        let mut map = std::collections::HashMap::new();
        map.insert("label".to_string(), value_cell(s(label)));
        if !children.is_empty() {
            map.insert(
                "children".to_string(),
                value_cell(Value::List(children.into_iter().map(value_cell).collect())),
            );
        }
        Value::Map(map)
    }

    fn scroll_with_stable_tree(stable_id: f64) -> Value {
        let items = Value::List(
            vec![
                tree_item(
                    "root-a",
                    vec![
                        tree_item("a-1", vec![]),
                        tree_item("a-2", vec![]),
                        tree_item("a-3", vec![]),
                        tree_item("a-4", vec![]),
                    ],
                ),
                tree_item("root-b", vec![]),
            ]
            .into_iter()
            .map(value_cell)
            .collect(),
        );
        build_widget(
            "scroll",
            vec![
                kw("width"),
                num(20.0),
                kw("height"),
                num(3.0),
                build_widget(
                    "tree",
                    vec![
                        kw("__stable-widget-id"),
                        num(stable_id),
                        kw("width"),
                        kw("fill"),
                        kw("items"),
                        items,
                    ],
                ),
            ],
        )
    }

    fn scroll_with_flat_selected_tree(stable_id: f64, labels: &[&str], selected: &str) -> Value {
        let items = Value::List(
            labels
                .iter()
                .map(|label| value_cell(tree_item(label, vec![])))
                .collect(),
        );
        build_widget(
            "scroll",
            vec![
                kw("width"),
                num(20.0),
                kw("height"),
                num(3.0),
                build_widget(
                    "tree",
                    vec![
                        kw("__stable-widget-id"),
                        num(stable_id),
                        kw("width"),
                        kw("fill"),
                        kw("items"),
                        items,
                        kw("selected-label"),
                        s(selected),
                    ],
                ),
            ],
        )
    }

    fn get_prop_num_from_layout(node: &LayoutNode, key: &str) -> Option<f64> {
        match node.props.get(key) {
            Some(Value::Number(value)) => Some(*value),
            _ => None,
        }
    }

    // ── Tests ──────────────────────────────────────────────────────────

    #[test]
    fn reused_scroll_layout_preserves_internal_content_dimensions() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let first = scroll_with_tall_box("first");
        let layout = engine.layout(&first).unwrap();

        assert_eq!(
            get_prop_num_from_layout(&layout, "_content_height"),
            Some(20.0)
        );
        assert_eq!(
            get_prop_num_from_layout(&layout, "_viewport_height"),
            Some(5.0)
        );

        let second = scroll_with_tall_box("second");
        let mut dirty_widget_ids = Vec::new();
        let reused = reuse_layout_node(&layout, &second, &mut dirty_widget_ids)
            .expect("scroll layout should be reusable for non-size prop changes");

        assert_eq!(
            get_prop_num_from_layout(&reused, "_content_height"),
            Some(20.0)
        );
        assert_eq!(
            get_prop_num_from_layout(&reused, "_viewport_height"),
            Some(5.0)
        );
    }

    #[test]
    fn tree_measure_uses_expansion_state_for_the_same_stable_tree_only() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let expanded_tree = scroll_with_stable_tree(200.0);
        let initial_layout = engine.layout(&expanded_tree).unwrap();
        assert_num_approx(
            get_prop_num_from_layout(&initial_layout, "_content_height"),
            2.5,
        );

        let tree_node = initial_layout.children.first().expect("tree child");
        let _ = crate::widget_render::tree::TREE_WIDGET.key_event(
            tree_node,
            WidgetKeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::empty(),
            },
        );

        let expanded_layout = engine.layout(&expanded_tree).unwrap();
        assert_num_approx(
            get_prop_num_from_layout(&expanded_layout, "_content_height"),
            7.5,
        );

        let collapsed_sibling_tree = scroll_with_stable_tree(100.0);
        let sibling_layout = engine.layout(&collapsed_sibling_tree).unwrap();
        assert_num_approx(
            get_prop_num_from_layout(&sibling_layout, "_content_height"),
            2.5,
        );
    }

    #[test]
    fn scroll_state_uses_collapsed_tree_height_even_when_layout_is_stale() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let tree = scroll_with_stable_tree(300.0);
        let collapsed_layout = engine.layout(&tree).unwrap();
        let tree_node = collapsed_layout.children.first().expect("tree child");
        let _ = crate::widget_render::tree::TREE_WIDGET.key_event(
            tree_node,
            WidgetKeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::empty(),
            },
        );

        let expanded_layout = engine.layout(&tree).unwrap();
        assert_num_approx(
            get_prop_num_from_layout(&expanded_layout, "_content_height"),
            7.5,
        );

        let expanded_tree_node = expanded_layout.children.first().expect("tree child");
        let _ = crate::widget_render::tree::TREE_WIDGET.key_event(
            expanded_tree_node,
            WidgetKeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::empty(),
            },
        );

        let state = crate::widget_render::scroll::sync_node_state(&expanded_layout);
        assert_f32_approx(state.content_height, 2.5);
        assert_eq!(state.viewport_height, 3.0);
        assert_eq!(state.offset_y, 0.0);
    }

    #[test]
    fn sticky_scroll_starts_at_bottom_and_follows_content_growth() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let first = sticky_scroll_with_content_height(500.0, 8.0);
        let first_layout = engine.layout(&first).unwrap();
        let first_state = crate::widget_render::scroll::sync_node_state(&first_layout);

        assert_f32_approx(first_state.offset_y, 5.0);

        let second = sticky_scroll_with_content_height(500.0, 12.0);
        let second_layout = engine.layout(&second).unwrap();
        let second_state = crate::widget_render::scroll::sync_node_state(&second_layout);

        assert_f32_approx(second_state.offset_y, 9.0);
    }

    #[test]
    fn sticky_scroll_preserves_manual_scroll_when_not_at_bottom() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let first = sticky_scroll_with_content_height(501.0, 8.0);
        let first_layout = engine.layout(&first).unwrap();
        let mut first_state = crate::widget_render::scroll::sync_node_state(&first_layout);
        first_state.offset_y = 2.0;
        crate::widget_render::scroll::set_scroll_state(501, first_state);

        let second = sticky_scroll_with_content_height(501.0, 12.0);
        let second_layout = engine.layout(&second).unwrap();
        let second_state = crate::widget_render::scroll::sync_node_state(&second_layout);

        assert_f32_approx(second_state.offset_y, 2.0);
    }

    #[test]
    fn tree_selection_resyncs_when_items_change_but_selected_label_does_not() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let first =
            scroll_with_flat_selected_tree(400.0, &["a", "b", "c", "d", "shared", "e"], "shared");
        let first_layout = engine.layout(&first).unwrap();
        let first_state = crate::widget_render::scroll::sync_node_state(&first_layout);
        assert!(
            first_state.offset_y > 0.0,
            "selected row should scroll into view"
        );

        let second = scroll_with_flat_selected_tree(400.0, &["shared", "x", "y"], "shared");
        let second_layout = engine.layout(&second).unwrap();
        let second_state = crate::widget_render::scroll::sync_node_state(&second_layout);
        assert_eq!(second_state.offset_y, 0.0);
    }

    #[test]
    fn tree_internal_selection_can_move_after_external_selection_syncs() {
        let engine = LayoutEngine::new(80, 24, 1.0);
        let tree = scroll_with_flat_selected_tree(500.0, &["a", "b", "c"], "a");
        let layout = engine.layout(&tree).unwrap();
        let tree_node = layout.children.first().expect("tree child");
        let _ = crate::widget_render::tree::TREE_WIDGET.key_event(
            tree_node,
            WidgetKeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::empty(),
            },
        );

        let state = crate::widget_render::scroll::sync_node_state(&layout);
        assert_eq!(state.offset_y, 0.0);

        let _ = crate::widget_render::tree::TREE_WIDGET.key_event(
            tree_node,
            WidgetKeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::empty(),
            },
        );
        let state = crate::widget_render::scroll::sync_node_state(&layout);
        assert_f32_approx(state.offset_y, 0.75);
    }

    #[test]
    fn natural_width_simple_vstack_fits_viewport() {
        // A v-stack with children narrower than the 80-col viewport.
        let tree = vstack(1.0, 1.0, vec![label("hello", Some(10.0)), hslider()]);
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
            .map(|i| {
                vstack(
                    0.0,
                    0.5,
                    vec![
                        vslider(4.0),
                        bx(Some(3.0), Some(1.5), vec![]),
                        label(&format!("{}", i + 1), None),
                    ],
                )
            })
            .collect();
        let tree = vstack(1.0, 1.0, vec![grid(16.0, 3.0, children)]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);
        // grid: 16 * 3 = 48, + vstack padding 2 = 50
        assert_eq!(natural, 50.0, "grid 16x3 + padding should be 50");
    }

    #[test]
    fn natural_width_sequencer_layout_fits_wide_viewport() {
        // Mirrors the full sequencer layout from metal-seq-grid.lisp
        // at a wide viewport (content should fit → no scroll needed)
        let transport = hstack(
            1.0,
            vec![
                bx(Some(4.0), Some(3.0), vec![]), // play button
                bx(
                    Some(40.0),
                    Some(3.0),
                    vec![
                        // LED panel
                        label("1 | 4 | 4  BPM 120", Some(32.0)),
                    ],
                ),
            ],
        );
        let param_tabs = hstack(
            0.5,
            vec![
                bx(Some(8.0), Some(2.0), vec![label("vel", None)]),
                bx(Some(8.0), Some(2.0), vec![label("dur", None)]),
                bx(Some(8.0), Some(2.0), vec![label("xpose", None)]),
                bx(Some(8.0), Some(2.0), vec![label("pan", None)]),
            ],
        );
        let step_grid = grid(
            16.0,
            3.0,
            (0..16)
                .map(|i| {
                    vstack(
                        0.0,
                        0.5,
                        vec![
                            vslider(4.0),
                            bx(Some(3.0), Some(1.5), vec![]),
                            label(&format!("{}", i + 1), None),
                        ],
                    )
                })
                .collect(),
        );
        let mixer_rows: Vec<Value> = ["Kick", "Snare", "Hat"]
            .iter()
            .map(|name| {
                hstack(
                    1.0,
                    vec![
                        bx(Some(14.0), Some(1.0), vec![label(name, None)]),
                        bx(None, None, vec![hslider()]), // flex=1 in real code, natural = hslider width
                    ],
                )
            })
            .collect();
        let mixer = vstack(0.0, 0.5, mixer_rows);
        let effects = hstack(
            1.0,
            vec![
                bx(
                    Some(20.0),
                    None,
                    vec![vstack(
                        0.0,
                        0.5,
                        vec![
                            label("Filter", None),
                            hstack(
                                0.5,
                                vec![
                                    label("cutoff", Some(8.0)),
                                    bx(Some(10.0), None, vec![hslider()]),
                                ],
                            ),
                        ],
                    )],
                ),
                bx(
                    Some(20.0),
                    None,
                    vec![vstack(
                        0.0,
                        0.5,
                        vec![
                            label("Delay", None),
                            hstack(
                                0.5,
                                vec![
                                    label("wet", Some(8.0)),
                                    bx(Some(10.0), None, vec![hslider()]),
                                ],
                            ),
                        ],
                    )],
                ),
            ],
        );

        let tree = vstack(
            1.0,
            1.0,
            vec![transport, param_tabs, step_grid, mixer, effects],
        );

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
        let step_grid = grid(
            16.0,
            3.0,
            (0..16)
                .map(|i| {
                    vstack(
                        0.0,
                        0.5,
                        vec![
                            vslider(4.0),
                            bx(Some(3.0), Some(1.5), vec![]),
                            label(&format!("{}", i + 1), None),
                        ],
                    )
                })
                .collect(),
        );
        let tree = vstack(1.0, 1.0, vec![step_grid]);

        let engine = LayoutEngine::new(40, 24, 1.0);
        let natural = engine.natural_content_width(&tree);

        // grid: 48 + padding 2 = 50, viewport 40
        assert_eq!(natural, 50.0, "natural width same regardless of viewport");
        assert!(
            natural > 40.0,
            "content should exceed narrow viewport → scroll needed"
        );
    }

    #[test]
    fn natural_width_long_labels_do_not_inflate() {
        // Labels with long text inside fixed-width boxes should NOT inflate natural width
        // beyond what the layout structure specifies.
        // (In TUI mode, label measures as char count if no explicit width)
        let mixer_rows: Vec<Value> = [
            "LS301-808ii-FC-Maraca-3-Extended-Name-Very-Long",
            "_12'' Augustus Pablo - King Tubby Meets",
        ]
        .iter()
        .map(|name| {
            hstack(
                1.0,
                vec![
                    bx(Some(14.0), Some(1.0), vec![label(name, None)]),
                    bx(Some(20.0), None, vec![hslider()]),
                ],
            )
        })
        .collect();
        let tree = vstack(1.0, 0.5, mixer_rows);

        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);

        // box widths: 14 + 20 + 1 gap = 35, + padding 2 = 37
        // The long label text should NOT push beyond 37 because its box is fixed at 14
        assert_eq!(
            natural, 37.0,
            "fixed-width boxes should contain long labels"
        );
    }

    #[test]
    fn natural_width_label_without_box_uses_text_width() {
        // A bare label (not in a fixed-width box) SHOULD use its text width
        let tree = vstack(
            0.0,
            0.0,
            vec![
                label("short", None),
                label("a much longer label text here", None),
            ],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);
        // In TUI mode (no TextMeasurer), label width = char count
        // "a much longer label text here" = 29 chars
        assert_eq!(
            natural, 29.0,
            "bare label should use text char count as width"
        );
    }

    #[test]
    fn wrapped_label_measures_multiple_lines_with_finite_width() {
        let tree = bx(
            Some(12.0),
            None,
            vec![wrapped_label(
                "Added a resonant low-pass filter after the carrier",
                None,
            )],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();
        let label = &layout.children[0];

        assert_eq!(label.rect.width, 12.0);
        assert!(
            label.rect.height > 1.0,
            "wrapped chat text should occupy multiple rows"
        );
    }

    #[test]
    fn width_fill_hstack_expands_to_parent_width() {
        let tree = bx(
            Some(40.0),
            None,
            vec![hstack_fill(
                1.0,
                vec![label("hello", None), label("world", None)],
            )],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();
        let stack = &layout.children[0];

        assert_eq!(layout.rect.width, 40.0);
        assert_eq!(stack.rect.width, 40.0);
    }

    #[test]
    fn width_fill_root_hstack_expands_to_viewport_width() {
        let tree = hstack_fill(1.0, vec![label("hello", None), label("world", None)]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();

        assert_eq!(layout.rect.width, 80.0);
    }

    #[test]
    fn width_fill_hstack_gives_remaining_width_to_flex_child() {
        let tree = bx(
            Some(40.0),
            None,
            vec![hstack_fill(
                1.0,
                vec![flex_box(1.0, vec![label("hello", None)]), label("x", None)],
            )],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();
        let stack = &layout.children[0];
        let flex_child = &stack.children[0];
        let fixed_child = &stack.children[1];

        assert_eq!(stack.rect.width, 40.0);
        assert_eq!(fixed_child.rect.width, 1.0);
        assert_eq!(flex_child.rect.width, 38.0);
    }

    #[test]
    fn width_fill_hstack_reserves_fixed_buttons_before_flex_text_input() {
        let tree = bx(
            Some(40.0),
            None,
            vec![hstack_fill(
                0.5,
                vec![
                    flex_text_input(1.0, "Describe..."),
                    button("Send"),
                    button("Cancel"),
                ],
            )],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();
        let stack = &layout.children[0];
        let input = &stack.children[0];
        let send = &stack.children[1];
        let cancel = &stack.children[2];

        assert_eq!(stack.rect.width, 40.0);
        assert!(input.rect.width > 0.0);
        assert!(
            cancel.rect.col + cancel.rect.width <= stack.rect.col + stack.rect.width,
            "trailing button must stay inside h-stack; stack={:?} cancel={:?}",
            stack.rect,
            cancel.rect
        );
        assert!(
            send.rect.col >= input.rect.col + input.rect.width + 0.5 - 0.000_01,
            "fixed button should be laid out after flex input"
        );
    }

    #[test]
    fn width_fill_is_ignored_for_unbounded_natural_width() {
        let tree = hstack_fill(1.0, vec![label("hello", None), label("world", None)]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let natural = engine.natural_content_width(&tree);

        assert_eq!(natural, 11.0);
    }

    #[test]
    fn width_fill_child_does_not_inflate_vstack_natural_width() {
        let tree = vstack(
            0.0,
            0.0,
            vec![
                hstack_fill(1.0, vec![label("hello", None), label("world", None)]),
                label("body content is wider", None),
            ],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();

        assert_eq!(layout.rect.width, 21.0);
        assert_eq!(layout.children[0].rect.width, 21.0);
    }

    /// Regression: `:width N` on a v-stack must constrain `:width :fill`
    /// children to N (not let them inflate to the v-stack's parent's max).
    /// Otherwise a row of v-stack columns each width 22 ends up rendering
    /// each column at the FULL rack width because the panels inside grew
    /// past the column. Multi-column layouts collapse to one.
    #[test]
    fn v_stack_explicit_width_constrains_fill_children() {
        let fill_panel = build_widget("box", vec![kw("width"), kw("fill"), kw("height"), num(2.0)]);
        let column = build_widget(
            "v-stack",
            vec![kw("width"), num(22.0), kw("gap"), num(0.1), fill_panel],
        );
        // Outer h-stack with a wide parent — the v-stack's :width must NOT be
        // overridden by the wider max-width context.
        let outer = build_widget(
            "h-stack",
            vec![kw("width"), kw("fill"), kw("gap"), num(0.5), column],
        );
        let root = bx(Some(100.0), Some(20.0), vec![outer]);

        let engine = LayoutEngine::new(120, 24, 1.0);
        let layout = engine.layout(&root).expect("layout");

        let h_stack = &layout.children[0];
        let v_stack = &h_stack.children[0];
        let panel = &v_stack.children[0];

        assert!(
            (v_stack.rect.width - 22.0).abs() < 0.01,
            "v-stack rect.width should be 22, got {}",
            v_stack.rect.width
        );
        assert!(
            (panel.rect.width - 22.0).abs() < 0.01,
            "panel :width :fill inside v-stack :width 22 should be 22, got {}",
            panel.rect.width
        );
    }

    /// Regression: a `:height :fill` child should not inflate its h-stack's
    /// measured height to the parent's max. Otherwise a padded grandparent
    /// re-adds padding on top of the already-max height and overflows.
    #[test]
    fn h_stack_height_excludes_height_fill_children() {
        let fill_box = build_widget("box", vec![kw("width"), num(4.0), kw("height"), kw("fill")]);
        let fixed_box = bx(Some(4.0), Some(3.0), vec![]);
        let inner_hstack = hstack(0.0, vec![fill_box, fixed_box]);
        // Outer box uses :v-align :start so it respects the h-stack's
        // measured height (otherwise :align :stretch hides the measure bug).
        let outer = build_widget(
            "box",
            vec![
                kw("width"),
                num(40.0),
                kw("height"),
                num(10.0),
                kw("v-align"),
                kw("start"),
                kw("h-align"),
                kw("start"),
                inner_hstack,
            ],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&outer).expect("layout");
        let h_stack = &layout.children[0];

        assert!(
            h_stack.rect.height <= 3.5,
            "h-stack height={} should track non-fill child (3.0), not :fill",
            h_stack.rect.height
        );
    }

    /// Regression: `:width :fill` on a box inside an unbounded-width container
    /// (h-stack without `:width :fill`) must NOT inflate to f32::MAX. The h-stack
    /// passes `max_width = f32::MAX` as an "unbounded" sentinel, but the box's
    /// `is_finite()` check treats MAX as bounded and explodes the layout.
    #[test]
    fn fill_box_in_unbounded_parent_does_not_explode() {
        // (h-stack (box :width :fill :height 0.5 (label "T")) (box :width 4 :height 2))
        // h-stack with no :width :fill → passes max_width = MAX to children.
        let filling_box = build_widget(
            "box",
            vec![
                kw("width"),
                kw("fill"),
                kw("height"),
                num(0.5),
                label("T", None),
            ],
        );
        let fixed_box = bx(Some(4.0), Some(2.0), vec![]);
        let tree = hstack(0.0, vec![filling_box, fixed_box]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).expect("layout");

        let header = &layout.children[0];
        assert!(
            header.rect.width.is_finite() && header.rect.width < 1000.0,
            ":width :fill in unbounded parent must not inflate to MAX; got {}",
            header.rect.width
        );
    }

    /// Repro: panel box with no explicit width, containing a v-stack whose
    /// children include a `:width :fill` header and a fixed-width body row.
    /// The header should stretch to the body's width; the box should size to
    /// the body row. Mirrors the `ui-panel` helper used by instrument UIs.
    #[test]
    fn fill_header_above_fixed_body_inflates_panel_box() {
        // header: (box :width :fill :height 0.55 (label "TITLE"))
        let header = build_widget(
            "box",
            vec![
                kw("width"),
                kw("fill"),
                kw("height"),
                num(0.55),
                label("TITLE", None),
            ],
        );
        // body: h-stack of two fixed-width knob-shaped boxes (width 4, height 2)
        let body = hstack(
            0.2,
            vec![
                bx(Some(4.0), Some(2.0), vec![]),
                bx(Some(4.0), Some(2.0), vec![]),
                bx(Some(4.0), Some(2.0), vec![]),
                bx(Some(4.0), Some(2.0), vec![]),
            ],
        );
        // inner: (v-stack :gap 0 :align :start header body)
        let inner = vstack(0.0, 0.0, vec![header, body]);
        // panel: (box :height 2.7 :padding 0.15 inner)  -- no width
        let panel = build_widget(
            "box",
            vec![kw("height"), num(2.7), kw("padding"), num(0.15), inner],
        );
        // column: (v-stack :width 28 panel)
        let column = build_widget(
            "v-stack",
            vec![kw("width"), num(28.0), kw("gap"), num(0.0), panel],
        );

        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&column).expect("layout column");

        let panel = &layout.children[0];
        assert!(
            panel.rect.width > 0.0,
            "panel width must be > 0, got {}",
            panel.rect.width
        );
        let inner = &panel.children[0];
        assert!(
            inner.rect.width > 0.0,
            "inner v-stack width must be > 0, got {}",
            inner.rect.width
        );
        let header = &inner.children[0];
        let body = &inner.children[1];
        assert!(
            header.rect.width > 0.0,
            "header should stretch to inner width, got {}",
            header.rect.width
        );
        assert!(
            body.rect.width > 0.0,
            "body row width must be > 0, got {}",
            body.rect.width
        );
        // Body should keep its natural content width (~17 cells), not collapse.
        assert!(
            body.rect.width >= 16.0,
            "body row should keep its natural knob row width, got {}",
            body.rect.width
        );
    }

    #[test]
    fn layout_width_clamps_to_viewport_but_natural_width_can_overflow() {
        let tree = hstack(
            1.0,
            vec![label("left", Some(60.0)), label("right", Some(60.0))],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();
        let natural = engine.natural_content_width(&tree);

        assert_eq!(layout.rect.width, 80.0);
        assert_eq!(natural, 121.0);
    }

    #[test]
    fn explicit_child_width_can_overflow_parent_viewport() {
        let tree = hstack(1.0, vec![bx(Some(120.0), Some(2.0), vec![])]);
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();

        assert_eq!(layout.rect.width, 80.0);
        assert_eq!(layout.children[0].rect.width, 120.0);
    }

    #[test]
    fn non_fill_hstack_positions_siblings_after_overflow_child() {
        let tree = hstack(
            1.0,
            vec![
                bx(None, Some(2.0), vec![bx(Some(120.0), Some(2.0), vec![])]),
                bx(Some(20.0), Some(2.0), vec![]),
            ],
        );
        let engine = LayoutEngine::new(80, 24, 1.0);
        let layout = engine.layout(&tree).unwrap();

        assert_eq!(layout.rect.width, 80.0);
        assert_eq!(layout.children[0].rect.width, 120.0);
        assert_eq!(layout.children[1].rect.col, 121.0);
    }
}
