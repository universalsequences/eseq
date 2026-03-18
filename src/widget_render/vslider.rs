use std::collections::HashMap;

use super::{CellBuffer, get_f32_prop, styled_cell};
use crate::backend::Color;
use crate::layout::Rect;
use crate::vm::Value;

/// TUI render for vertical slider: fill from bottom up.
pub fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let value = get_f32_prop(props, "value", 0.0);
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    let t = if range > 0.0 {
        (value - min) / range
    } else {
        0.0
    };
    let t = t.clamp(0.0, 1.0);

    let height = rect.height;
    let filled = (t * height as f32).round() as u16;
    let threshold = height.saturating_sub(filled);

    for row_offset in 0..height {
        let row = rect.row + row_offset;
        // Render 2-wide column
        for col_offset in 0..rect.width.min(2) {
            let col = rect.col + col_offset;
            if row_offset >= threshold {
                buf.set(row, col, styled_cell('\u{2588}', Color::GREEN, None)); // '█'
            } else {
                buf.set(row, col, styled_cell(' ', Color::DARK_GRAY, None));
            }
        }
    }
}

// ── Metal GPU data ───────────────────────────────────────────────────────────

pub const VSLIDER_SHADER: &str = "vslider";

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VSliderGpuData {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub value_t: f32,
    pub fill_color: [f32; 4],
    pub track_color: [f32; 4],
}

pub fn write_gpu_data(
    props: &HashMap<String, Value>,
    rect: Rect,
    _viewport: (f32, f32),
    out: &mut Vec<u8>,
) {
    let value = get_f32_prop(props, "value", 0.0);
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    let t = if range > 0.0 {
        (value - min) / range
    } else {
        0.0
    };

    let data = VSliderGpuData {
        pos: [rect.col as f32, rect.row as f32],
        size: [rect.width as f32, rect.height as f32],
        value_t: t.clamp(0.0, 1.0),
        fill_color: [0.0, 0.8, 0.0, 1.0],     // Green
        track_color: [0.25, 0.25, 0.25, 1.0], // Dark gray
    };

    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            &data as *const VSliderGpuData as *const u8,
            std::mem::size_of::<VSliderGpuData>(),
        )
    };
    out.extend_from_slice(bytes);
}
