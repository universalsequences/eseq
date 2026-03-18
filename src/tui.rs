use crate::backend::{Cell, CellStyle, Color, RenderFrame};
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
    let text_lines: Vec<Line> = render_frame.lines.iter().map(|row| cells_to_line(row)).collect();

    let title = if render_frame.dirty {
        format!("**{}**", render_frame.buffer_name)
    } else {
        render_frame.buffer_name.clone()
    };

    let text_widget = Paragraph::new(text_lines)
        .block(Block::default().borders(Borders::ALL).title(title));
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
        .style(Style::default().bg(RColor::DarkGray).fg(RColor::White));
    frame.render_widget(status_widget, chunks[1]);

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
                        Style::default().bg(RColor::Rgb(84, 78, 150)).fg(RColor::White),
                    ))
                } else {
                    Line::from(Span::styled(
                        label,
                        Style::default().bg(RColor::Rgb(38, 38, 56)).fg(RColor::White),
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
                        .fg(RColor::Rgb(187, 129, 255))
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
                        .style(Style::default().bg(RColor::Black))
                        .wrap(Wrap { trim: false }),
                    doc_area,
                );
            }
        }
    }
}

fn pad_right(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    while out.len() < width {
        out.push(' ');
    }
    out
}
