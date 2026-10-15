//! A tree item's `:swatch` paints a color chip in the icon slot, and the
//! label moves right to make room for it, as it does for an `:icon`.

use eseqlisp::{
    Runtime,
    layout::{LayoutEngine, LayoutNode},
    widget_render::{GpuPrimitive, WidgetViewport, widget_primitives_for_node},
};

fn find<'a>(node: &'a LayoutNode, widget_type: &str) -> Option<&'a LayoutNode> {
    if node.widget_type == widget_type {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, widget_type))
}

#[test]
fn swatch_item_paints_its_color_chip() {
    let mut rt = Runtime::new();
    let tree = rt
        .eval_str(
            r##"(tree :key "swatch-tree" :width 40 :height 6
                 :items (list (dict :label "accent" :swatch '(1 0 0.5 1) :detail "#ff0080")))"##,
        )
        .expect("tree evaluates")
        .expect("tree value");
    let layout = LayoutEngine::new(80, 20, 1.0).layout(&tree).expect("layout");
    let node = find(&layout, "tree").expect("tree node");
    let prims = widget_primitives_for_node(
        node,
        WidgetViewport {
            cell_w: 8.0,
            cell_h: 16.0,
            vp_w: 640.0,
            vp_h: 320.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 20.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        },
    );
    let chip = prims.iter().any(|prim| match prim {
        GpuPrimitive::Rect(rect) => {
            (rect.color.r, rect.color.g, rect.color.b) == (1.0, 0.0, 0.5) && rect.rect.width > 0.0
        }
        _ => false,
    });
    assert!(chip, "a Rect in the swatch color is painted");
}
