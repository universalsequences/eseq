use std::collections::HashMap;

use super::{CellBuffer, ImageFit, WidgetDefinition};
use super::{GpuImagePrimitive, GpuPrimitive};
use crate::backend::Color;
use crate::layout::{
    Constraints, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, prop_is_keyword,
};
use crate::vm::Value;

pub struct ImageWidget;

pub static IMAGE_WIDGET: ImageWidget = ImageWidget;

/// Shared GPU image decode policy. Keep UI screenshots at native resolution;
/// the old 640-pixel album-thumbnail cap made document text unreadable.
/// Bound uploads by the device limit and an 8192-pixel resource budget.
pub(crate) fn decode_image_file(path: &std::path::Path, device_limit: u32) -> Option<image::RgbaImage> {
    let limit = device_limit.min(8192);
    if limit == 0 { return None; }
    let mut decoded = image::ImageReader::open(path).ok()?.decode().ok()?;
    if decoded.width().max(decoded.height()) > limit {
        decoded = decoded.resize(limit, limit, image::imageops::FilterType::Triangle);
    }
    Some(decoded.to_rgba8())
}

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

fn rotation(props: &HashMap<String, Value>) -> f32 {
    match props.get("rotation") {
        Some(Value::Number(value)) => *value as f32,
        _ => 0.0,
    }
}

fn rotation_speed(props: &HashMap<String, Value>) -> f32 {
    match props.get("rotation-speed") {
        Some(Value::Number(value)) => *value as f32,
        _ => 0.0,
    }
}

fn clip_circle(props: &HashMap<String, Value>) -> bool {
    matches!(
        props.get("clip"),
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "circle"
    )
}

impl WidgetDefinition for ImageWidget {
    fn names(&self) -> &'static [&'static str] {
        &["image"]
    }

    fn doc(&self) -> &'static str {
        "Shows an image file (PNG, JPEG and the like) from :src, a file path; it is decoded \
in the background and picked up again when the file changes. :fit is :cover \
(default, fills the box and crops), :contain (fits inside, letterboxed) or :stretch. \
Size: :width N or :fill, and :height N; without a height it follows :aspect (pixel \
width over height, default 1, square), without a width it is derived from the \
height, and with neither it is 12 cells wide. :max-pixel-width caps the width at the \
image's native pixel size, for screenshots in documents. :radius rounds the corners \
(px), :clip :circle crops to a circle, :opacity (0 to 1) fades it, and :rotation \
(radians) with :rotation-speed (radians per second) turn it, like a spinning record \
label. Example: (image :src cover-path :width 12 :aspect 1 :clip :circle \
:rotation-speed 0.8)"
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "src",
            "width",
            "height",
            "aspect",
            "max-pixel-width",
            "fit",
            "radius",
            "clip",
            "opacity",
            "rotation",
            "rotation-speed",
        ]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "aspect", "max-pixel-width"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[&Value],
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

        // Optional native-resolution cap for document illustrations. Width
        // still shrinks with the panel, and height follows the same aspect.
        let width = get_prop_num(node, "max-pixel-width")
            .filter(|value| value.is_finite() && *value > 0.0 && ctx.cell_w > 0.0)
            .map(|pixels| width.min(pixels as f32 / ctx.cell_w))
            .unwrap_or(width);
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

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &crate::layout::LayoutNode,
        _viewport: super::WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let Some(src) = node.props.get("src").and_then(|value| match value {
            Value::String(src) => Some(src.clone()),
            _ => None,
        }) else {
            return Vec::new();
        };
        if src.is_empty() || node.rect.width <= 0.0 || node.rect.height <= 0.0 {
            return Vec::new();
        }
        vec![GpuPrimitive::Image(GpuImagePrimitive {
            widget_id: node.widget_id,
            rect: node.rect,
            src,
            fit: image_fit(&node.props),
            radius_px: super::ui_design_px(radius_px(&node.props)),
            opacity: opacity(&node.props),
            rotation: rotation(&node.props),
            rotation_speed: rotation_speed(&node.props),
            clip_circle: clip_circle(&node.props),
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_images_keep_detail_and_respect_the_device_texture_limit() {
        let path = std::env::temp_dir().join(format!("eseq-image-detail-{}.png", std::process::id()));
        let source = image::RgbaImage::from_fn(1200, 100, |x, _|
            image::Rgba([if x % 2 == 0 { 255 } else { 0 }, 0, 0, 255]));
        source.save(&path).unwrap();
        assert_eq!(decode_image_file(&path, 8192).unwrap(), source);
        assert_eq!(decode_image_file(&path, 600).unwrap().dimensions(), (600, 50));
        std::fs::remove_file(path).unwrap();
    }
}
