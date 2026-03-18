use std::collections::HashMap;

use super::{CellBuffer, get_bool_prop, styled_cell};
use crate::backend::Color;
use crate::layout::Rect;
use crate::vm::Value;

/// TUI render for toggle: "[×]" or "[ ]"
pub fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let on = get_bool_prop(props, "value", false);

    let (text, fg) = if on {
        ("[×]", Color::GREEN)
    } else {
        ("[ ]", Color::DARK_GRAY)
    };

    for (i, ch) in text.chars().enumerate() {
        let col = rect.col + i as u16;
        if col >= rect.col + rect.width {
            break;
        }
        buf.set(rect.row, col, styled_cell(ch, fg, None));
    }
}

// ── Metal GPU data ───────────────────────────────────────────────────────────

pub const TOGGLE_SHADER: &str = "toggle";

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ToggleGpuData {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub on: f32,
    pub on_color: [f32; 4],
    pub off_color: [f32; 4],
}

pub fn write_gpu_data(
    props: &HashMap<String, Value>,
    rect: Rect,
    _viewport: (f32, f32),
    out: &mut Vec<u8>,
) {
    let on = get_bool_prop(props, "value", false);

    let data = ToggleGpuData {
        pos: [rect.col as f32, rect.row as f32],
        size: [rect.width as f32, rect.height as f32],
        on: if on { 1.0 } else { 0.0 },
        on_color: [0.0, 0.8, 0.0, 1.0],     // Green
        off_color: [0.25, 0.25, 0.25, 1.0], // Dark gray
    };

    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            &data as *const ToggleGpuData as *const u8,
            std::mem::size_of::<ToggleGpuData>(),
        )
    };
    out.extend_from_slice(bytes);
}
