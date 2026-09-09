use std::collections::HashMap;

use super::{
    CellBuffer, EventOutput, GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive,
    MouseEventOutcome, WidgetDefinition, WidgetEvent, get_f32_prop, resolve_named_color,
    styled_cell,
};
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, MeasureCtx, Rect, Size, f64_to_f32, get_map, get_prop_num,
    get_prop_str, usize_to_f32,
};
use crate::theme;
use crate::vm::Value;

pub struct LabelWidget;

pub static LABEL_WIDGET: LabelWidget = LabelWidget;

fn resolve_color(props: &HashMap<String, Value>, hovered: bool) -> Color {
    if hovered {
        if let Some(value) = props.get("hover-color") {
            return crate::theme::parse_color_value(value).unwrap_or(theme::WIDGET_LABEL_FG());
        }
    }
    if get_f32_prop(props, "active", 0.0) != 0.0
        && let Some(value) = props.get("active-color")
    {
        return crate::theme::parse_color_value(value).unwrap_or(theme::WIDGET_LABEL_FG());
    }
    resolve_named_color(props, "color", theme::WIDGET_LABEL_FG())
}

fn resolve_h_align(props: &HashMap<String, Value>) -> f32 {
    match props.get("h-align") {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "center" || value == "middle" =>
        {
            0.5
        }
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "right" || value == "end" =>
        {
            1.0
        }
        _ => 0.0,
    }
}

fn underline_enabled(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("underline"), Some(Value::Bool(true)))
}

/// A one-pixel rule under one rendered line, in cell space, matching the
/// line's horizontal alignment and measured width. `row` is the line's top.
fn underline_rect(
    line: &str,
    row: f32,
    col: f32,
    align_width: f32,
    h_align: f32,
    font_size: f32,
    viewport: super::WidgetViewport,
) -> Rect {
    let cell_w = viewport.cell_w.max(1.0);
    let cell_h = viewport.cell_h.max(1.0);
    let (text_px, cap_height_px) = super::with_render_text_measurer(|measurer| {
        (
            measurer.measure_text_px(line, font_size),
            measurer.cap_height_px(font_size),
        )
    })
    .unwrap_or((line.chars().count() as f32 * cell_w, cell_h * 0.7));
    let baseline_px = crate::ui::glyph_atlas::centered_text_baseline_px(cell_h, cap_height_px, 1.0);
    let slack_px = (align_width * cell_w - text_px).max(0.0);
    Rect {
        col: col + slack_px * h_align / cell_w,
        // Two pixels below the baseline clears descender-free glyphs without
        // colliding with the next wrapped line.
        row: row + (baseline_px + 2.0) / cell_h,
        width: text_px / cell_w,
        height: 1.0 / cell_h,
    }
}

fn wrap_enabled(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("wrap"), Some(Value::Bool(true)))
}

fn node_wrap_enabled(node: &Value) -> bool {
    get_map(node).is_some_and(|props| wrap_enabled(&props))
}

fn wrap_text_by_columns(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let word_len = word.chars().count();
            if current.is_empty() {
                if word_len <= width {
                    current.push_str(word);
                } else {
                    push_hard_wrapped_word(&mut lines, word, width, &mut current);
                }
            } else {
                let current_len = current.chars().count();
                if current_len + 1 + word_len <= width {
                    current.push(' ');
                    current.push_str(word);
                } else {
                    lines.push(std::mem::take(&mut current));
                    if word_len <= width {
                        current.push_str(word);
                    } else {
                        push_hard_wrapped_word(&mut lines, word, width, &mut current);
                    }
                }
            }
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_hard_wrapped_word(lines: &mut Vec<String>, word: &str, width: usize, current: &mut String) {
    let mut chunk = String::new();
    for ch in word.chars() {
        chunk.push(ch);
        if chunk.chars().count() == width {
            lines.push(std::mem::take(&mut chunk));
        }
    }
    *current = chunk;
}

fn measure_wrapped_text_lines(
    text: &str,
    max_width_cells: f32,
    font_size: f32,
    ctx: &MeasureCtx<'_>,
) -> Vec<String> {
    let width_cells = max_width_cells.max(1.0);
    let Some(measurer) = ctx.text_measurer else {
        return wrap_text_by_columns(text, width_cells.floor() as usize);
    };
    let max_px = width_cells * ctx.cell_w;
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if measurer.measure_text_px(&candidate, font_size) <= max_px || current.is_empty() {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }

            while measurer.measure_text_px(&current, font_size) > max_px
                && current.chars().count() > 1
            {
                let mut fit = String::new();
                let mut rest = String::new();
                for ch in current.chars() {
                    let next = format!("{fit}{ch}");
                    if !fit.is_empty() && measurer.measure_text_px(&next, font_size) > max_px {
                        rest.push(ch);
                    } else {
                        fit.push(ch);
                    }
                }
                if fit.is_empty() || rest.is_empty() {
                    break;
                }
                lines.push(fit);
                current = rest;
            }
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_text_row_respects_vertical_alignment() {
        let rect = Rect {
            row: 2.0,
            col: 0.0,
            width: 8.0,
            height: 3.0,
        };
        let mut props = HashMap::new();

        assert_eq!(label_text_row(&props, rect), 2.0);

        props.insert("v-align".to_string(), Value::Keyword("center".to_string()));
        assert_eq!(label_text_row(&props, rect), 3.0);

        props.insert("v-align".to_string(), Value::Keyword("end".to_string()));
        assert_eq!(label_text_row(&props, rect), 4.0);
    }

    #[test]
    fn label_text_row_centers_inside_sub_cell_height_rects() {
        let rect = Rect {
            row: 2.0,
            col: 0.0,
            width: 8.0,
            height: 0.82,
        };
        let props = HashMap::from([("v-align".to_string(), Value::Keyword("center".to_string()))]);

        assert!((label_text_row(&props, rect) - 1.91).abs() < 0.0001);
    }

    #[test]
    fn active_color_overrides_base_color_when_active() {
        let mut props = HashMap::from([
            ("color".to_string(), Value::Keyword("dim".to_string())),
            (
                "active-color".to_string(),
                Value::Keyword("yellow".to_string()),
            ),
        ]);

        assert_eq!(resolve_color(&props, false), theme::DIM());

        props.insert("active".to_string(), Value::Number(1.0));

        assert_eq!(resolve_color(&props, false), theme::YELLOW());
    }
    #[test]
    fn wrapped_gpu_lines_match_measured_proportional_width_and_height() {
        struct Font;
        impl crate::layout::TextMeasurer for Font {
            fn measure_text_px(&self, text: &str, _: f32) -> f32 { text.chars().count() as f32 * 4.0 }
            fn line_height_px(&self, _: f32) -> f32 { 10.0 }
        }
        let props: HashMap<String, Value> = [
            ("text".into(), Value::String("wide letters".into())),
            ("wrap".into(), Value::Bool(true)),
            ("width".into(), Value::Number(4.0)),
            ("font-size".into(), Value::Number(10.0)),
            ("bg".into(), Value::Keyword("transparent".into())),
        ].into_iter().collect();
        let value = Value::Map(props.iter().map(|(key, value)|
            (key.clone(), std::rc::Rc::new(std::cell::RefCell::new(value.clone())))).collect());
        let ctx = MeasureCtx { text_measurer: Some(&Font), cell_w: 10.0, cell_h: 20.0, inherited_font_size: 10.0 };
        let size = LABEL_WIDGET.measure(&value, &[], Constraints {
            min_width: 0.0, max_width: 4.0, min_height: 0.0, max_height: 100.0, aspect: 0.5,
        }, &ctx, &mut |_, _| None).unwrap();
        assert_eq!(size.height, 1.0, "two half-cell font lines");
        let node = crate::layout::LayoutNode {
            widget_id: 1, stable_widget_id: None, subtree_root_id: None,
            parent_subtree_root_id: None, stable_key: None, widget_type: "label".into(),
            rect: Rect { col: 0.0, row: 0.0, width: size.width, height: size.height },
            props, children: vec![], focusable: false, animation: Default::default(),
        };
        super::super::set_render_text_measurer(std::rc::Rc::new(Font));
        let primitives = LABEL_WIDGET.build_primitives("label", &node, super::super::WidgetViewport {
            cell_w: 10.0, cell_h: 20.0, vp_w: 1000.0, vp_h: 800.0,
            time_seconds: 0.0, focused_widget_id: None, focused_branch: false,
            overlay_viewport_bottom: 40.0, scroll_top: 0.0, scroll_left: 0.0, inherited_hover: false,
        });
        let lines: Vec<_> = primitives.iter().filter_map(|p| match p {
            GpuPrimitive::ProportionalText(t) => Some((t.text.as_str(), t.row)), _ => None,
        }).collect();
        assert_eq!(lines, vec![("wide", 0.0), ("letters", 0.5)]);
    }

    #[test]
    fn underline_draws_one_pixel_rule_per_line_matching_text_width() {
        struct Font;
        impl crate::layout::TextMeasurer for Font {
            fn measure_text_px(&self, text: &str, _: f32) -> f32 { text.chars().count() as f32 * 4.0 }
            fn line_height_px(&self, _: f32) -> f32 { 10.0 }
        }
        let props: HashMap<String, Value> = [
            ("text".into(), Value::String("wide letters".into())),
            ("wrap".into(), Value::Bool(true)),
            ("underline".into(), Value::Bool(true)),
            ("h-align".into(), Value::Keyword("right".into())),
            ("font-size".into(), Value::Number(10.0)),
            ("bg".into(), Value::Keyword("transparent".into())),
        ].into_iter().collect();
        let node = crate::layout::LayoutNode {
            widget_id: 1, stable_widget_id: None, subtree_root_id: None,
            parent_subtree_root_id: None, stable_key: None, widget_type: "label".into(),
            rect: Rect { col: 1.0, row: 0.0, width: 4.0, height: 1.0 },
            props, children: vec![], focusable: false, animation: Default::default(),
        };
        super::super::set_render_text_measurer(std::rc::Rc::new(Font));
        let viewport = super::super::WidgetViewport {
            cell_w: 10.0, cell_h: 20.0, vp_w: 1000.0, vp_h: 800.0,
            time_seconds: 0.0, focused_widget_id: None, focused_branch: false,
            overlay_viewport_bottom: 40.0, scroll_top: 0.0, scroll_left: 0.0, inherited_hover: false,
        };
        let primitives = LABEL_WIDGET.build_primitives("label", &node, viewport);
        let rules: Vec<Rect> = primitives.iter().filter_map(|p| match p {
            GpuPrimitive::Rect(r) => Some(r.rect), _ => None,
        }).collect();
        assert_eq!(rules.len(), 2, "one rule per wrapped line, no bg rect");
        // "wide" is 16px in a 40px box, right-aligned: starts 24px in.
        assert!((rules[0].col - (1.0 + 2.4)).abs() < 1e-4, "{:?}", rules[0]);
        assert!((rules[0].width - 1.6).abs() < 1e-4);
        assert!((rules[0].height - 0.05).abs() < 1e-4, "one pixel in a 20px cell");
        // The backend centres each line's baseline in one mono cell from the
        // line's top: (20 + 7) / 2 + 2 = 15.5px = 0.775 cells; line 2 sits
        // half a cell (one 10px font line) lower.
        assert!((rules[0].row - 0.775).abs() < 1e-4, "under line 1: {:?}", rules[0]);
        assert!((rules[1].row - 1.275).abs() < 1e-4, "under line 2: {:?}", rules[1]);
        let text_fg = primitives.iter().find_map(|p| match p {
            GpuPrimitive::ProportionalText(t) => Some(t.fg), _ => None,
        }).unwrap();
        let rule_color = primitives.iter().find_map(|p| match p {
            GpuPrimitive::Rect(r) => Some(r.color), _ => None,
        }).unwrap();
        assert_eq!(rule_color, text_fg, "underline takes the text colour (incl. hover)");
    }

}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let text = match props.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };

    let fg = resolve_color(props, false);
    let row_u16 = label_text_row(props, rect).round() as u16;
    let col_u16 = rect.col.round() as u16;
    let width_u16 = rect.width.round() as u16;

    let lines = if wrap_enabled(props) {
        wrap_text_by_columns(&text, width_u16 as usize)
    } else {
        vec![text]
    };

    for (line_idx, line) in lines.iter().enumerate() {
        let row = row_u16 + line_idx as u16;
        if row >= row_u16 + rect.height.round() as u16 {
            break;
        }
        for (i, ch) in line.chars().enumerate() {
            let col = col_u16 + i as u16;
            if col >= col_u16 + width_u16 {
                break;
            }
            buf.set(row, col, styled_cell(ch, fg, None));
        }
        let text_len = line.chars().count() as u16;
        for i in text_len..width_u16 {
            let col = col_u16 + i;
            buf.set(row, col, styled_cell(' ', fg, None));
        }
    }
}

pub fn label_text_row(props: &HashMap<String, Value>, rect: Rect) -> f32 {
    match props.get("v-align") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "center" => {
            rect.row + (rect.height - 1.0) * 0.5
        }
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "end" => {
            rect.row + rect.height - 1.0
        }
        _ => rect.row,
    }
}

impl WidgetDefinition for LabelWidget {
    fn names(&self) -> &'static [&'static str] {
        &["label"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "width", "height", "font-size", "wrap"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["active"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "text", "color", "active", "active-color", "hover-color", "bg", "font-size",
            "width", "height", "wrap", "h-align", "v-align", "underline", "on-click",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let text = get_prop_str(node, "text").unwrap_or_default();
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let explicit_width = get_prop_num(node, "width").map(f64_to_f32);
        if node_wrap_enabled(node)
            && (explicit_width.is_some() || constraints.max_width.is_finite())
        {
            let width = explicit_width.unwrap_or(constraints.max_width).max(1.0);
            let line_count = measure_wrapped_text_lines(&text, width, font_size, ctx).len();
            let line_height = ctx
                .text_measurer
                .map(|measurer| measurer.line_height_px(font_size) / ctx.cell_h)
                .unwrap_or(1.0);
            return Some(Size {
                width,
                height: get_prop_num(node, "height")
                    .map(f64_to_f32)
                    .unwrap_or(line_height * line_count as f32),
            });
        }

        // If a TextMeasurer is available (Metal backend), use proportional measurement.
        if let Some(measurer) = ctx.text_measurer {
            let px_width = if let Some(explicit_w) = explicit_width {
                // Explicit width is in cell units, convert to pixels.
                explicit_w * ctx.cell_w
            } else {
                measurer.measure_text_px(&text, font_size)
            };
            let px_height = measurer.line_height_px(font_size);
            return Some(Size {
                width: px_width / ctx.cell_w,
                height: get_prop_num(node, "height")
                    .map(f64_to_f32)
                    .unwrap_or(px_height / ctx.cell_h),
            });
        }

        // TUI fallback: monospace char-count measurement.
        Some(Size {
            width: explicit_width.unwrap_or_else(|| {
                get_prop_str(node, "text")
                    .map(|text| usize_to_f32(text.chars().count()))
                    .unwrap_or(0.0)
            }),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.0),
        })
    }

    fn baseline_offset(&self, node: &Value, size: Size, ctx: &MeasureCtx<'_>) -> Option<f32> {
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let row_offset = match get_map(node).and_then(|props| props.get("v-align").cloned()) {
            Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "center" => {
                (size.height - 1.0) * 0.5
            }
            Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "end" => {
                size.height - 1.0
            }
            _ => 0.0,
        };
        Some(super::proportional_text_baseline_offset(
            font_size,
            row_offset,
            ctx,
        ))
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    // A label with `:on-click` / `:on-right-click` / `:on-double-click` is a
    // link: dispatch the same one-argument pointer event a `box` does, so
    // handlers are written `(lambda (event) …)`. Without these, the generic
    // pointer-target check made a clickable label swallow the click and do
    // nothing.
    fn mouse_event(
        &self,
        node: &crate::layout::LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let dispatch = |phase: &str| {
            MouseEventOutcome::Dispatch(WidgetEvent::Custom(super::pointer_event_info(
                phase, modifiers, node, local_col, local_row,
            )))
        };
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Right)
                if node.props.contains_key("on-right-click") =>
            {
                dispatch("right-click")
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if modifiers.contains(KeyModifiers::CONTROL)
                    && node.props.contains_key("on-right-click")
                {
                    dispatch("right-click")
                } else if node.props.contains_key("on-click") {
                    dispatch("click")
                } else {
                    MouseEventOutcome::Ignore
                }
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn double_click_event(
        &self,
        node: &crate::layout::LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        node.props.contains_key("on-double-click").then(|| {
            WidgetEvent::Custom(super::pointer_event_info(
                "double-click",
                KeyModifiers::empty(),
                node,
                local_col,
                local_row,
            ))
        })
    }

    fn handle_event(
        &self,
        node: &crate::layout::LayoutNode,
        event: WidgetEvent,
    ) -> Option<EventOutput> {
        let (callback_name, arg) = match event {
            WidgetEvent::Activate(modifiers) => (
                "on-click",
                super::pointer_event_info("click", modifiers, node, node.rect.col, node.rect.row),
            ),
            WidgetEvent::Custom(value) => {
                let phase = match &value {
                    Value::Map(map) => map.get("phase").and_then(|v| match &*v.borrow() {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    }),
                    _ => None,
                }?;
                let callback_name = match phase.as_str() {
                    "click" => "on-click",
                    "right-click" => "on-right-click",
                    "double-click" => "on-double-click",
                    _ => return None,
                };
                (callback_name, value)
            }
            _ => return None,
        };
        let callback = node.props.get(callback_name)?.clone();
        Some(EventOutput {
            callback,
            args: vec![arg],
        })
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &crate::layout::LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let Some(Value::String(text)) = node.props.get("text") else {
            return Vec::new();
        };
        let fg = resolve_color(&node.props, viewport.inherited_hover);
        let bg_transparent = matches!(
            node.props.get("bg"),
            Some(Value::Keyword(k)) if k == "transparent"
        );
        let bg = if viewport.focused_branch {
            theme::WIDGET_FOCUS_BG()
        } else {
            theme::BG()
        };
        let font_size = node
            .props
            .get("font-size")
            .and_then(|v| match v {
                Value::Number(n) => Some(*n as f32),
                _ => None,
            })
            .unwrap_or(DEFAULT_FONT_SIZE);
        let mut prims = Vec::new();
        if !bg_transparent {
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: node.rect,
                color: bg,
            }));
        }
        let (lines, line_height) = if wrap_enabled(&node.props) {
            // Use the same font metrics and wrapping as measure(). Monospace
            // column counts or a one-cell advance can clip proportional lines.
            super::with_render_text_measurer(|measurer| {
                let ctx = MeasureCtx {
                    text_measurer: Some(measurer),
                    cell_w: viewport.cell_w,
                    cell_h: viewport.cell_h,
                    inherited_font_size: font_size,
                };
                (measure_wrapped_text_lines(text, node.rect.width, font_size, &ctx),
                    measurer.line_height_px(font_size) / viewport.cell_h)
            }).unwrap_or_else(|| (
                wrap_text_by_columns(text, node.rect.width.floor().max(1.0) as usize), 1.0,
            ))
        } else {
            (vec![text.clone()], 1.0)
        };
        let start_row = label_text_row(&node.props, node.rect);
        let h_align = resolve_h_align(&node.props);
        let underline = underline_enabled(&node.props);
        for (line_idx, line) in lines.into_iter().enumerate() {
            let row = start_row + line_idx as f32 * line_height;
            if row >= node.rect.row + node.rect.height {
                break;
            }
            if underline && !line.trim().is_empty() {
                prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: underline_rect(
                        &line,
                        row,
                        node.rect.col,
                        node.rect.width,
                        h_align,
                        font_size,
                        viewport,
                    ),
                    color: fg,
                }));
            }
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
                    row,
                    col: node.rect.col,
                    align_width: node.rect.width,
                    h_align,
                    text: line,
                    font_size,
                    scale: 1.0,
                    fg,
                    bg,
                },
            ));
        }
        prims
    }
}
