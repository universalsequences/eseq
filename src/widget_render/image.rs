use std::collections::HashMap;

use super::{CellBuffer, ImageFit, WidgetDefinition};
#[cfg(target_os = "macos")]
use super::{MetalImagePrimitive, MetalPrimitive};
use crate::backend::Color;
use crate::layout::{
    Constraints, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, prop_is_keyword,
};
use crate::vm::Value;

pub struct ImageWidget;

pub static IMAGE_WIDGET: ImageWidget = ImageWidget;

fn image_fit(props: &HashMap<String, Value>) -> ImageFit {
    match props.get("fit") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) => match value.as_str() {
            "contain" => ImageFit::Contain,
            "stretch" => ImageFit::Stretch,
            _ => ImageFit::Cover,
        },
        _ => ImageFit::Cover,
    }
}

fn opacity(props: &HashMap<String, Value>) -> f32 {
    match props.get("opacity") {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

fn radius_px(props: &HashMap<String, Value>) -> f32 {
    match props.get("radius") {
        Some(Value::Number(value)) => (*value as f32).max(0.0),
        _ => 0.0,
    }
}

impl WidgetDefinition for ImageWidget {
    fn names(&self) -> &'static [&'static str] {
        &["image"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "aspect"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let pixel_aspect = get_prop_num(node, "aspect")
            .map(f64_to_f32)
            .filter(|value| *value > 0.0)
            .unwrap_or(1.0);
        let cell_pixel_aspect = if ctx.cell_h > 0.0 {
            ctx.cell_w / ctx.cell_h
        } else {
            1.0
        };

        let width = if prop_is_keyword(node, "width", "fill") && constraints.max_width.is_finite() {
            constraints.max_width
        } else {
            get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or_else(|| {
                    get_prop_num(node, "height")
                        .map(f64_to_f32)
                        .map(|height| height * pixel_aspect / cell_pixel_aspect)
                        .unwrap_or(12.0)
                })
        };

        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(width / pixel_aspect * cell_pixel_aspect);

        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let Some(src) = props.get("src").and_then(|value| match value {
            Value::String(src) if !src.is_empty() => Some(src.as_str()),
            _ => None,
        }) else {
            return;
        };
        let label = src
            .rsplit(std::path::MAIN_SEPARATOR)
            .next()
            .unwrap_or(src)
            .chars()
            .take(rect.width.max(0.0).round() as usize)
            .collect::<String>();
        let fg = Color::rgba(0.65, 0.68, 0.72, 1.0);
        for (idx, ch) in label.chars().enumerate() {
            buf.set(
                rect.row.round() as u16,
                rect.col.round() as u16 + idx as u16,
                super::styled_cell(ch, fg, None),
            );
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &crate::layout::LayoutNode,
        _viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let Some(src) = node.props.get("src").and_then(|value| match value {
            Value::String(src) => Some(src.clone()),
            _ => None,
        }) else {
            return Vec::new();
        };
        if src.is_empty() || node.rect.width <= 0.0 || node.rect.height <= 0.0 {
            return Vec::new();
        }
        vec![MetalPrimitive::Image(MetalImagePrimitive {
            rect: node.rect,
            src,
            fit: image_fit(&node.props),
            radius_px: radius_px(&node.props),
            opacity: opacity(&node.props),
        })]
    }
}
