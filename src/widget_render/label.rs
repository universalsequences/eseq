use std::collections::HashMap;

use super::{CellBuffer, styled_cell};
use crate::backend::Color;
use crate::layout::Rect;
use crate::vm::Value;

/// TUI render for label: write each character with white foreground.
pub fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let text = match props.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };

    for (i, ch) in text.chars().enumerate() {
        let col = rect.col + i as u16;
        if col >= rect.col + rect.width {
            break;
        }
        buf.set(rect.row, col, styled_cell(ch, Color::WHITE, None));
    }
}
