//! Reusable Metal focus decorations for measured widget bounds.

use super::{GpuPrimitive, GpuRectPrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::Rect;

/// A focus decoration selected by a widget and rendered by the shared widget
/// primitive pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FocusDecoration {
    None,
    Corners(FocusCornerStyle),
}

impl FocusDecoration {
    pub(crate) fn primitives(
        self,
        rect: Rect,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        match self {
            Self::None => Vec::new(),
            Self::Corners(style) => style.primitives(rect, viewport),
        }
    }
}

/// Four sharp L-shaped corner marks, expressed in pixels so their visual weight
/// stays consistent across widgets with different cell-space dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusCornerStyle {
    pub color: Color,
    pub arm_length_px: f32,
    pub stroke_width_px: f32,
    pub inset_px: f32,
}

impl FocusCornerStyle {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            arm_length_px: 7.0,
            stroke_width_px: 1.5,
            inset_px: 1.0,
        }
    }

    fn primitives(self, rect: Rect, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
        if !rect.row.is_finite()
            || !rect.col.is_finite()
            || !rect.width.is_finite()
            || !rect.height.is_finite()
            || rect.width <= 0.0
            || rect.height <= 0.0
            || !viewport.cell_w.is_finite()
            || !viewport.cell_h.is_finite()
            || viewport.cell_w <= 0.0
            || viewport.cell_h <= 0.0
        {
            return Vec::new();
        }

        let width_px = rect.width * viewport.cell_w;
        let height_px = rect.height * viewport.cell_h;
        let inset_px = super::ui_design_px(self.inset_px.max(0.0));
        let inset_x_px = inset_px.min(width_px * 0.5);
        let inset_y_px = inset_px.min(height_px * 0.5);
        let inner_width_px = (width_px - inset_x_px * 2.0).max(0.0);
        let inner_height_px = (height_px - inset_y_px * 2.0).max(0.0);
        if inner_width_px <= 0.0 || inner_height_px <= 0.0 {
            return Vec::new();
        }

        let requested_arm_px = super::ui_design_px(self.arm_length_px.max(0.0));
        let arm_x_px = requested_arm_px.min(inner_width_px);
        let arm_y_px = requested_arm_px.min(inner_height_px);
        let requested_stroke_px = super::ui_design_px(self.stroke_width_px.max(0.0));
        let stroke_x_px = requested_stroke_px.min(arm_x_px);
        let stroke_y_px = requested_stroke_px.min(arm_y_px);
        if arm_x_px <= 0.0 || arm_y_px <= 0.0 || stroke_x_px <= 0.0 || stroke_y_px <= 0.0 {
            return Vec::new();
        }

        let left = rect.col + inset_x_px / viewport.cell_w;
        let top = rect.row + inset_y_px / viewport.cell_h;
        let right = rect.col + rect.width - inset_x_px / viewport.cell_w;
        let bottom = rect.row + rect.height - inset_y_px / viewport.cell_h;
        let arm_width = arm_x_px / viewport.cell_w;
        let arm_height = arm_y_px / viewport.cell_h;
        let stroke_width = stroke_x_px / viewport.cell_w;
        let stroke_height = stroke_y_px / viewport.cell_h;

        let rects = [
            // Top-left.
            Rect {
                row: top,
                col: left,
                width: arm_width,
                height: stroke_height,
            },
            Rect {
                row: top,
                col: left,
                width: stroke_width,
                height: arm_height,
            },
            // Top-right.
            Rect {
                row: top,
                col: right - arm_width,
                width: arm_width,
                height: stroke_height,
            },
            Rect {
                row: top,
                col: right - stroke_width,
                width: stroke_width,
                height: arm_height,
            },
            // Bottom-left.
            Rect {
                row: bottom - stroke_height,
                col: left,
                width: arm_width,
                height: stroke_height,
            },
            Rect {
                row: bottom - arm_height,
                col: left,
                width: stroke_width,
                height: arm_height,
            },
            // Bottom-right.
            Rect {
                row: bottom - stroke_height,
                col: right - arm_width,
                width: arm_width,
                height: stroke_height,
            },
            Rect {
                row: bottom - arm_height,
                col: right - stroke_width,
                width: stroke_width,
                height: arm_height,
            },
        ];

        rects
            .into_iter()
            .map(|rect| {
                GpuPrimitive::ForegroundRect(GpuRectPrimitive {
                    rect,
                    color: self.color,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport(cell_w: f32, cell_h: f32) -> WidgetViewport {
        WidgetViewport {
            cell_w,
            cell_h,
            vp_w: 800.0,
            vp_h: 600.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 30.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

    fn focus_rects(primitives: &[GpuPrimitive]) -> Vec<Rect> {
        primitives
            .iter()
            .map(|primitive| match primitive {
                GpuPrimitive::ForegroundRect(rect) => rect.rect,
                _ => panic!("focus corners must render as foreground rectangles"),
            })
            .collect()
    }

    fn assert_contains(outer: Rect, inner: Rect) {
        let epsilon = 0.000_01;
        assert!(inner.col >= outer.col - epsilon);
        assert!(inner.row >= outer.row - epsilon);
        assert!(inner.col + inner.width <= outer.col + outer.width + epsilon);
        assert!(inner.row + inner.height <= outer.row + outer.height + epsilon);
    }

    #[test]
    fn corners_are_pixel_sized_and_bounded_by_the_widget_rect() {
        let rect = Rect {
            row: 2.0,
            col: 3.0,
            width: 8.0,
            height: 4.0,
        };
        let primitives = FocusDecoration::Corners(FocusCornerStyle::new(Color::WHITE))
            .primitives(rect, viewport(10.0, 20.0));
        let rects = focus_rects(&primitives);

        assert_eq!(rects.len(), 8);
        assert!((rects[0].width - 0.7).abs() < 0.000_01);
        assert!((rects[0].height - 0.075).abs() < 0.000_01);
        assert!((rects[1].width - 0.15).abs() < 0.000_01);
        assert!((rects[1].height - 0.35).abs() < 0.000_01);
        for corner_rect in rects {
            assert_contains(rect, corner_rect);
        }
    }

    #[test]
    fn corners_clamp_cleanly_inside_a_tiny_widget() {
        let rect = Rect {
            row: 1.0,
            col: 1.0,
            width: 0.2,
            height: 0.2,
        };
        let style = FocusCornerStyle {
            arm_length_px: 20.0,
            stroke_width_px: 10.0,
            inset_px: 0.25,
            ..FocusCornerStyle::new(Color::WHITE)
        };
        let rects = focus_rects(
            &FocusDecoration::Corners(style).primitives(rect, viewport(10.0, 10.0)),
        );

        assert_eq!(rects.len(), 8);
        for corner_rect in rects {
            assert!(corner_rect.width.is_finite() && corner_rect.width > 0.0);
            assert!(corner_rect.height.is_finite() && corner_rect.height > 0.0);
            assert_contains(rect, corner_rect);
        }
    }

    #[test]
    fn invalid_bounds_do_not_emit_focus_geometry() {
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 0.0,
            height: 3.0,
        };
        assert!(
            FocusDecoration::Corners(FocusCornerStyle::new(Color::WHITE))
                .primitives(rect, viewport(10.0, 20.0))
                .is_empty()
        );
    }
}
