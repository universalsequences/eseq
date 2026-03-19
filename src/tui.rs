use crate::backend::{Cell, CellStyle, Color, RenderFrame};
use crate::widget_render;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color as RColor, Modifier, Style},
    text::{Line, Span},
    widgets::Clear,
    widgets::{Block, Borders, Paragraph, Wrap},
};

// ── Color conversion ──────────────────────────────────────────────────────────

fn to_rcolor(c: Color) -> RColor {
    RColor::Rgb(
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8,
    )
}

fn cell_style_to_ratatui(s: CellStyle) -> Style {
    let mut style = Style::default().fg(to_rcolor(s.fg));
    if let Some(bg) = s.bg {
        style = style.bg(to_rcolor(bg));
    }
    if s.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

// ── Cell → ratatui span conversion ───────────────────────────────────────────

/// Converts a row of cells into a ratatui `Line` by run-length encoding
/// consecutive cells that share the same style into a single `Span`.
fn cells_to_line(cells: &[Cell]) -> Line<'static> {
    if cells.is_empty() {
        return Line::default();
    }
    let mut spans: Vec<Span> = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        let style = cells[start].style;
        let mut end = start + 1;
        while end < cells.len() && cells[end].style == style {
            end += 1;
        }
        let text: String = cells[start..end].iter().map(|c| c.ch).collect();
        if style == CellStyle::default() {
            spans.push(Span::raw(text));
        } else {
            spans.push(Span::styled(text, cell_style_to_ratatui(style)));
        }
        start = end;
    }
    Line::from(spans)
}

// ── Renderer ──────────────────────────────────────────────────────────────────

/// Render a `RenderFrame` using ratatui. Called inside `terminal.draw(...)`.
pub fn render(frame: &mut Frame, render_frame: &RenderFrame) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // ── Text area ─────────────────────────────────────────────────────────────
    let text_lines: Vec<Line> = render_frame
        .lines
        .iter()
        .map(|row| cells_to_line(row))
        .collect();

    let title = if render_frame.dirty {
        format!("**{}**", render_frame.buffer_name)
    } else {
        render_frame.buffer_name.clone()
    };

    let text_widget =
        Paragraph::new(text_lines).block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(text_widget, chunks[0]);

    // ── Cursor ────────────────────────────────────────────────────────────────
    if let Some((vis_row, vis_col)) = render_frame.cursor {
        let inner_left = chunks[0].x.saturating_add(1);
        let inner_top = chunks[0].y.saturating_add(1);
        let inner_right = chunks[0].right().saturating_sub(2);
        let inner_bottom = chunks[0].bottom().saturating_sub(2);
        let cursor_x = inner_left.saturating_add(vis_col as u16).min(inner_right);
        let cursor_y = inner_top.saturating_add(vis_row as u16).min(inner_bottom);
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    // ── Status bar ────────────────────────────────────────────────────────────
    let status_widget = Paragraph::new(render_frame.status.clone())
        .style(Style::default().bg(to_rcolor(crate::theme::STATUS_BG)).fg(to_rcolor(crate::theme::STATUS_FG)));
    frame.render_widget(status_widget, chunks[1]);

    // ── Widget overlay ──────────────────────────────────────────────────────
    if let Some(ref layout) = render_frame.widget_layout {
        let inner = chunks[0].inner(ratatui::layout::Margin::new(1, 1));
        let mut cell_buf = widget_render::CellBuffer::new(inner.width, inner.height);
        widget_render::render_widget_tree(layout, &mut cell_buf);
        blit_cell_buffer(&cell_buf, frame, inner);
    }

    // ── Focus highlight ──────────────────────────────────────────────────────
    if let (Some(layout), Some(focused_id)) =
        (&render_frame.widget_layout, render_frame.focused_widget_id)
    {
        if let Some(node) = find_focused_node(layout, focused_id) {
            let inner = chunks[0].inner(ratatui::layout::Margin::new(1, 1));
            let focus_bg = to_rcolor(crate::theme::COMP_SELECTED_BG);
            let buf = frame.buffer_mut();
            // Highlight the entire widget rect with a visible background
            for row in node.rect.row..node.rect.row + node.rect.height {
                let y = inner.y + row;
                if y >= inner.bottom() {
                    break;
                }
                for col in node.rect.col..node.rect.col + node.rect.width {
                    let x = inner.x + col;
                    if x >= inner.right() {
                        break;
                    }
                    let cell = &mut buf[(x, y)];
                    cell.set_style(
                        Style::default()
                            .fg(cell.style().fg.unwrap_or(RColor::White))
                            .bg(focus_bg)
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
    }

    // ── Completion popup ──────────────────────────────────────────────────────
    if let Some(comp) = &render_frame.completion {
        let list_width = comp
            .entries
            .iter()
            .map(|e| e.label.len())
            .max()
            .unwrap_or(0)
            .max(12)
            .min((chunks[0].width / 2).max(12) as usize);

        let preferred_height = comp.entries.len() as u16;
        let (anchor_row, anchor_col) = comp.anchor;
        let popup_x = (chunks[0].x + anchor_col as u16)
            .min(chunks[0].right().saturating_sub(list_width as u16 + 1));
        let popup_y = (chunks[0].y + 1 + anchor_row as u16 + 1)
            .min(chunks[0].bottom().saturating_sub(preferred_height.max(1)));
        let list_height = preferred_height
            .min(chunks[0].bottom().saturating_sub(popup_y))
            .max(1);
        let list_area = Rect::new(popup_x, popup_y, list_width as u16, list_height);

        let items: Vec<Line> = comp
            .entries
            .iter()
            .take(list_height as usize)
            .map(|entry| {
                let label = pad_right(&entry.label, list_width);
                if entry.selected {
                    Line::from(Span::styled(
                        label,
                        Style::default()
                            .bg(to_rcolor(crate::theme::COMP_SELECTED_BG))
                            .fg(to_rcolor(crate::theme::COMP_FG)),
                    ))
                } else {
                    Line::from(Span::styled(
                        label,
                        Style::default()
                            .bg(to_rcolor(crate::theme::COMP_UNSELECTED_BG))
                            .fg(to_rcolor(crate::theme::COMP_FG)),
                    ))
                }
            })
            .collect();

        frame.render_widget(Clear, list_area);
        frame.render_widget(Paragraph::new(items), list_area);

        // Doc panel
        if let Some((title, body)) = &comp.doc {
            let available_width = chunks[0].right().saturating_sub(list_area.right() + 1);
            if available_width > 0 {
                let doc_width = available_width.min(48);
                let available_height = chunks[0].bottom().saturating_sub(popup_y).max(1);
                let doc_lines: Vec<Line> = std::iter::once(Line::from(Span::styled(
                    title.clone(),
                    Style::default()
                        .fg(to_rcolor(crate::theme::COMP_DOC_TITLE_FG))
                        .add_modifier(Modifier::BOLD),
                )))
                .chain(std::iter::once(Line::from("")))
                .chain(body.iter().map(|l| Line::from(Span::raw(l.clone()))))
                .collect();
                let doc_height = (doc_lines.len().max(6).min(12) as u16).min(available_height);
                let doc_area = Rect::new(list_area.right() + 1, popup_y, doc_width, doc_height);
                frame.render_widget(Clear, doc_area);
                frame.render_widget(
                    Paragraph::new(doc_lines)
                        .style(Style::default().bg(to_rcolor(crate::theme::COMP_DOC_BG)).fg(to_rcolor(crate::theme::COMP_DOC_FG)))
                        .wrap(Wrap { trim: false }),
                    doc_area,
                );
            }
        }
    }
}

/// Blit a CellBuffer onto the ratatui frame. Only `Some` cells overwrite.
fn blit_cell_buffer(cell_buf: &widget_render::CellBuffer, frame: &mut Frame, area: Rect) {
    let buf = frame.buffer_mut();
    for (row_idx, row) in cell_buf.cells.iter().enumerate() {
        for (col_idx, cell_opt) in row.iter().enumerate() {
            if let Some(cell) = cell_opt {
                let x = area.x + col_idx as u16;
                let y = area.y + row_idx as u16;
                if x < area.right() && y < area.bottom() {
                    let ratatui_cell = &mut buf[(x, y)];
                    ratatui_cell.set_char(cell.ch);
                    ratatui_cell.set_style(cell_style_to_ratatui(cell.style));
                }
            }
        }
    }
}

fn find_focused_node(
    node: &crate::layout::LayoutNode,
    id: u64,
) -> Option<&crate::layout::LayoutNode> {
    if node.widget_id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_focused_node(child, id) {
            return Some(found);
        }
    }
    None
}

fn pad_right(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    while out.len() < width {
        out.push(' ');
    }
    out
}
