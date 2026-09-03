use crossterm::event::KeyModifiers;
use winit::event::MouseScrollDelta;

/// Conventional number of content lines advanced by one discrete wheel detent.
///
/// Winit's `LineDelta` identifies a unit, not a pixel distance. Treating one
/// unit as one line made common Linux mouse wheels noticeably slower than
/// native scrolling, where a detent normally advances multiple lines. Other
/// platforms keep one unit per line, matching what they shipped before this
/// helper existed.
#[cfg(target_os = "linux")]
const LINES_PER_WHEEL_DETENT: f32 = 3.0;
#[cfg(not(target_os = "linux"))]
const LINES_PER_WHEEL_DETENT: f32 = 1.0;

/// Winit 0.29 forwards Wayland's continuous axis distance at a substantially
/// smaller useful magnitude than AppKit's precise scrolling delta. Normalize
/// that platform boundary here so all widget consumers continue to operate on
/// the same pixel-delta contract.
#[cfg(target_os = "linux")]
const PIXEL_SCROLL_SCALE: f32 = 2.0;
#[cfg(not(target_os = "linux"))]
const PIXEL_SCROLL_SCALE: f32 = 1.0;

/// Vertical scroll distance corresponding to one octave of cursor-anchored zoom.
///
/// Keeping this in normalized pixel units makes Ctrl+trackpad-scroll smooth while
/// giving a discrete wheel detent a modest, predictable zoom step.
const CTRL_SCROLL_PIXELS_PER_ZOOM_OCTAVE: f32 = 480.0;

pub(crate) fn ctrl_scroll_magnify_delta(
    modifiers: KeyModifiers,
    delta_pixels: (f32, f32),
) -> Option<f64> {
    // macOS zooms from the native `TouchpadMagnify` pinch gesture, and the
    // system reserves Ctrl+scroll for its own screen-zoom accessibility
    // gesture, so Ctrl+scroll stays an ordinary scroll there.
    if cfg!(target_os = "macos") {
        return None;
    }
    if !modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let (delta_x, delta_y) = delta_pixels;
    if !delta_x.is_finite()
        || !delta_y.is_finite()
        || delta_y == 0.0
        || delta_y.abs() <= delta_x.abs()
    {
        return None;
    }
    Some((delta_y / CTRL_SCROLL_PIXELS_PER_ZOOM_OCTAVE) as f64)
}

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
    fn discrete_wheel_detent_advances_platform_line_count() {
        assert_eq!(
            scroll_delta_pixels(MouseScrollDelta::LineDelta(-1.0, 1.0), 24.0),
            (
                -24.0 * LINES_PER_WHEEL_DETENT,
                24.0 * LINES_PER_WHEEL_DETENT
            )
        );
    }

    #[test]
    fn discrete_wheel_uses_minimum_line_height() {
        assert_eq!(
            scroll_delta_pixels(MouseScrollDelta::LineDelta(0.0, 1.0), 12.0),
            (0.0, 20.0 * LINES_PER_WHEEL_DETENT)
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

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn ctrl_vertical_scroll_converts_pixels_to_magnify_delta() {
        assert_eq!(
            ctrl_scroll_magnify_delta(KeyModifiers::CONTROL, (2.0, 120.0)),
            Some(0.25)
        );
        assert_eq!(
            ctrl_scroll_magnify_delta(KeyModifiers::CONTROL, (-2.0, -120.0)),
            Some(-0.25)
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn ctrl_vertical_scroll_stays_a_scroll_on_macos() {
        assert_eq!(
            ctrl_scroll_magnify_delta(KeyModifiers::CONTROL, (2.0, 120.0)),
            None
        );
    }

    #[test]
    fn ordinary_scroll_is_not_converted_to_magnify() {
        assert_eq!(
            ctrl_scroll_magnify_delta(KeyModifiers::NONE, (0.0, 120.0)),
            None
        );
    }

    #[test]
    fn ctrl_horizontal_scroll_is_not_converted_to_magnify() {
        assert_eq!(
            ctrl_scroll_magnify_delta(KeyModifiers::CONTROL, (120.0, 2.0)),
            None
        );
    }
}

// ── Hidden-cursor infinite drag ──────────────────────────────────────────────

/// Live state for an Ableton-style hidden-cursor drag.
///
/// While active the OS pointer is hidden and grabbed, so `CursorMoved` stops
/// arriving (macOS) or reports a pinned position (Wayland). Raw
/// `DeviceEvent::MouseMotion` deltas are accumulated into `virtual_pos`, a
/// pointer position in physical pixels that is free to walk arbitrarily far
/// past the window bounds — that is the whole point: a knob at the bottom edge
/// of the screen keeps receiving downward travel.
#[derive(Debug, Clone, Copy)]
pub struct HiddenDrag {
    /// Physical-pixel position of the press. The cursor is warped back here
    /// when the drag ends.
    pub anchor: winit::dpi::PhysicalPosition<f64>,
    /// Unbounded virtual pointer position in physical pixels.
    pub virtual_pos: (f64, f64),
    /// True when the pointer was actually grabbed; a failed grab still runs
    /// the drag (with a visible cursor) rather than panicking.
    pub grabbed: bool,
}

impl HiddenDrag {
    pub fn new(anchor: winit::dpi::PhysicalPosition<f64>, grabbed: bool) -> Self {
        Self {
            anchor,
            virtual_pos: (anchor.x, anchor.y),
            grabbed,
        }
    }

    /// Accumulate one raw motion delta and return the new virtual position.
    pub fn accumulate(&mut self, delta: (f64, f64)) -> (f64, f64) {
        self.virtual_pos.0 += delta.0;
        self.virtual_pos.1 += delta.1;
        self.virtual_pos
    }
}

/// Hide and grab the pointer for a hidden-cursor drag. Returns whether the
/// grab succeeded; a failure is logged and the drag proceeds ungrabbed rather
/// than aborting or panicking.
pub fn grab_pointer_for_hidden_drag(window: &winit::window::Window) -> bool {
    use winit::window::CursorGrabMode;
    let grabbed = match window.set_cursor_grab(CursorGrabMode::Locked) {
        Ok(()) => true,
        Err(locked_err) => match window.set_cursor_grab(CursorGrabMode::Confined) {
            Ok(()) => {
                eprintln!(
                    "hidden drag: locked cursor grab unavailable ({locked_err}); using confined grab"
                );
                true
            }
            Err(confined_err) => {
                eprintln!(
                    "hidden drag: cursor grab unavailable (locked: {locked_err}; confined: {confined_err}); dragging without a grab"
                );
                false
            }
        },
    };
    window.set_cursor_visible(false);
    grabbed
}

/// Release the pointer grab, warp the cursor back to the press point and make
/// it visible again. Safe to call when no drag is active.
pub fn release_pointer_after_hidden_drag(window: &winit::window::Window, drag: Option<HiddenDrag>) {
    use winit::window::CursorGrabMode;
    if let Some(drag) = drag.as_ref()
        && drag.grabbed
        && let Err(err) = window.set_cursor_grab(CursorGrabMode::None)
    {
        eprintln!("hidden drag: releasing cursor grab failed: {err}");
    }
    if let Some(drag) = drag.as_ref()
        && let Err(err) = window.set_cursor_position(drag.anchor)
    {
        eprintln!("hidden drag: warping cursor back to the press point failed: {err}");
    }
    window.set_cursor_visible(true);
}
