use std::collections::HashMap;

use super::{
    CellBuffer, MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive,
    WidgetDefinition, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

pub struct MixerMeterWidget;

pub static MIXER_METER_WIDGET: MixerMeterWidget = MixerMeterWidget;

const LABELS: [(f32, &str); 7] = [
    (0.00, "0"),
    (0.14, "6"),
    (0.25, "12"),
    (0.42, "24"),
    (0.59, "36"),
    (0.76, "48"),
    (1.00, "60"),
];

fn meter_color(level: f32, threshold: f32) -> Color {
    if level <= threshold {
        Color::rgba(0.045, 0.048, 0.052, 1.0)
    } else if threshold > 0.88 {
        Color::rgba(0.95, 0.18, 0.16, 1.0)
    } else if threshold > 0.70 {
        Color::rgba(0.96, 0.82, 0.18, 1.0)
    } else {
        Color::rgba(0.10, 0.85, 0.30, 1.0)
    }
}

fn label_row(rect: Rect, position: f32, font_height: f32, top_inset: f32) -> f32 {
    let scale_top = rect.row + top_inset.max(0.0);
    let scale_height = (rect.height - top_inset.max(0.0)).max(0.0);
    let centered = scale_top + position.clamp(0.0, 1.0) * scale_height - font_height * 0.5;
    centered.clamp(scale_top, rect.row + (rect.height - font_height).max(0.0))
}

impl WidgetDefinition for MixerMeterWidget {
    fn names(&self) -> &'static [&'static str] {
        &["mixer-meter"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "font-size"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["level-l", "level-r"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(2.22),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(4.24),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let color = resolve_named_color(props, "label-color", theme::WIDGET_LABEL_FG());
        let row = rect.row.round() as u16;
        let col = rect.col.round() as u16;
        for (idx, (_, text)) in LABELS.iter().enumerate() {
            for (ch_idx, ch) in text.chars().enumerate() {
                buf.set(
                    row + idx as u16,
                    col + ch_idx as u16,
                    styled_cell(ch, color, None),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let level_l = get_f32_prop(&node.props, "level-l", 0.0).clamp(0.0, 1.0);
        let level_r = get_f32_prop(&node.props, "level-r", 0.0).clamp(0.0, 1.0);
        let label_color = resolve_named_color(&node.props, "label-color", theme::WIDGET_LABEL_FG());
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE * 0.5);
        let font_height = get_f32_prop(&node.props, "label-height", 0.42);
        let label_top_inset = get_f32_prop(&node.props, "label-top-inset", 0.45);

        let bar_w = get_f32_prop(&node.props, "bar-width", 0.28);
        let bar_gap = get_f32_prop(&node.props, "bar-gap", 0.08);
        let label_gap = get_f32_prop(&node.props, "label-gap", 0.18);
        let segment_gap = get_f32_prop(&node.props, "segment-gap", 0.08);
        let segments = get_f32_prop(&node.props, "segments", 12.0).round().max(1.0) as usize;
        let segment_h = (node.rect.height - segment_gap * (segments.saturating_sub(1) as f32))
            .max(0.0)
            / segments as f32;

        let mut prims = Vec::with_capacity(segments * 2 + LABELS.len());
        for si in 0..segments {
            let threshold = (segments - si) as f32 / segments as f32;
            let y = node.rect.row + si as f32 * (segment_h + segment_gap);
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: y,
                    col: node.rect.col,
                    width: bar_w,
                    height: segment_h,
                },
                color: meter_color(level_l, threshold),
            }));
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: y,
                    col: node.rect.col + bar_w + bar_gap,
                    width: bar_w,
                    height: segment_h,
                },
                color: meter_color(level_r, threshold),
            }));
        }

        let label_col = node.rect.col + bar_w * 2.0 + bar_gap + label_gap;
        for (position, text) in LABELS {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: label_row(node.rect, position, font_height, label_top_inset),
                    col: label_col,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: text.to_string(),
                    font_size,
                    scale: 1.0,
                    fg: label_color,
                    bg: theme::BG(),
                },
            ));
        }

        prims
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::widget_render::WidgetViewport;

    #[test]
    fn labels_share_meter_scale_extents() {
        let rect = Rect {
            row: 10.0,
            col: 3.0,
            width: 2.22,
            height: 4.24,
        };
        let top_inset = 0.45;
        assert!((label_row(rect, 0.0, 0.42, top_inset) - (rect.row + top_inset)).abs() < 0.0001);
        assert!(
            (label_row(rect, 1.0, 0.42, top_inset) - (rect.row + rect.height - 0.42)).abs()
                < 0.0001
        );
        let minus_six_row = label_row(rect, 0.14, 0.42, top_inset);
        assert!(minus_six_row >= rect.row + top_inset + 0.3);
        assert!(minus_six_row < rect.row + rect.height * 0.25);
    }

    #[test]
    fn metal_primitives_anchor_labels_to_meter_rect() {
        let rect = Rect {
            row: 10.0,
            col: 3.0,
            width: 2.22,
            height: 4.24,
        };
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "mixer-meter".to_string(),
            rect,
            props: HashMap::new(),
            children: Vec::new(),
            focusable: false,
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 1000.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 50.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };

        let prims = MIXER_METER_WIDGET.build_metal_primitives("mixer-meter", &node, viewport);
        let first_bar_row = prims
            .iter()
            .find_map(|prim| match prim {
                MetalPrimitive::Rect(rect) => Some(rect.rect.row),
                _ => None,
            })
            .unwrap();
        let top_label_row = prims
            .iter()
            .find_map(|prim| match prim {
                MetalPrimitive::ProportionalText(text) if text.text == "0" => Some(text.row),
                _ => None,
            })
            .unwrap();

        assert!((first_bar_row - rect.row).abs() < 0.0001);
        assert!((top_label_row - (rect.row + 0.45)).abs() < 0.0001);
    }

    #[test]
    fn metal_primitives_resolve_reactive_ref_levels_at_draw_time() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let mut registry = crate::reactive::ReactiveRegistry::with_float_slots(slots.clone());
        registry.register("APP", vec![("peak", Value::Number(0.0))], true);

        let rect = Rect {
            row: 10.0,
            col: 3.0,
            width: 2.22,
            height: 4.24,
        };
        let mut props = HashMap::new();
        props.insert(
            "level-l".to_string(),
            Value::ReactiveRef {
                namespace: "APP".to_string(),
                field: "peak".to_string(),
                index: None,
                kind: crate::vm::BindingKind::Float,
                slot: slots.slot("APP", "peak"),
            },
        );
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "mixer-meter".to_string(),
            rect,
            props,
            children: Vec::new(),
            focusable: false,
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 1000.0,
            vp_h: 1000.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 50.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };

        let near_top_left_color = |prims: Vec<MetalPrimitive>| {
            prims
                .into_iter()
                .filter_map(|prim| match prim {
                    MetalPrimitive::Rect(rect) => Some(rect.color),
                    _ => None,
                })
                .nth(2)
                .expect("first meter segment")
        };

        assert_eq!(
            near_top_left_color(MIXER_METER_WIDGET.build_metal_primitives(
                "mixer-meter",
                &node,
                viewport
            )),
            Color::rgba(0.045, 0.048, 0.052, 1.0)
        );

        registry.set("APP", "peak", Value::Number(1.0), false);

        assert_eq!(
            near_top_left_color(MIXER_METER_WIDGET.build_metal_primitives(
                "mixer-meter",
                &node,
                viewport
            )),
            Color::rgba(0.95, 0.18, 0.16, 1.0)
        );
    }
}
