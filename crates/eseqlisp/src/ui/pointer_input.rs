use winit::event::MouseScrollDelta;

/// Conventional number of content lines advanced by one discrete wheel detent.
///
/// Winit's `LineDelta` identifies a unit, not a pixel distance. Treating one
/// unit as one line made common Linux mouse wheels noticeably slower than
/// native scrolling, where a detent normally advances multiple lines.
const LINES_PER_WHEEL_DETENT: f32 = 3.0;

/// Winit 0.29 forwards Wayland's continuous axis distance at a substantially
/// smaller useful magnitude than AppKit's precise scrolling delta. Normalize
/// that platform boundary here so all widget consumers continue to operate on
/// the same pixel-delta contract.
#[cfg(target_os = "linux")]
const PIXEL_SCROLL_SCALE: f32 = 2.0;
#[cfg(not(target_os = "linux"))]
const PIXEL_SCROLL_SCALE: f32 = 1.0;

pub(crate) fn scroll_delta_pixels(
    delta: MouseScrollDelta,
    cell_height: f32,
) -> (f32, f32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => {
            let pixels_per_line = cell_height.max(20.0);
            let pixels_per_detent = pixels_per_line * LINES_PER_WHEEL_DETENT;
            (x * pixels_per_detent, y * pixels_per_detent)
        }
        MouseScrollDelta::PixelDelta(delta) => (
            delta.x as f32 * PIXEL_SCROLL_SCALE,
            delta.y as f32 * PIXEL_SCROLL_SCALE,
        ),
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    #[test]
    fn discrete_wheel_detent_advances_three_lines() {
        assert_eq!(
            scroll_delta_pixels(MouseScrollDelta::LineDelta(-1.0, 1.0), 24.0),
            (-72.0, 72.0)
        );
    }

    #[test]
    fn discrete_wheel_uses_minimum_line_height() {
        assert_eq!(
            scroll_delta_pixels(MouseScrollDelta::LineDelta(0.0, 1.0), 12.0),
            (0.0, 60.0)
        );
    }

    #[test]
    fn pixel_scroll_uses_platform_normalization() {
        let normalized = scroll_delta_pixels(
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(2.5, -4.0)),
            24.0,
        );
        assert_eq!(normalized, (2.5 * PIXEL_SCROLL_SCALE, -4.0 * PIXEL_SCROLL_SCALE));
    }
}
