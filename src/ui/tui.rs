use crate::backend::{Cell, CellStyle, Color, RenderFrame, TiledRenderFrame};
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

fn brighten_rcolor(color: RColor, amount: f32) -> RColor {
    match color {
        RColor::Rgb(r, g, b) => {
            let brighten = |channel: u8| -> u8 {
                let channel = channel as f32;
                (channel + (255.0 - channel) * amount)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            RColor::Rgb(brighten(r), brighten(g), brighten(b))
        }
        other => other,
    }
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

    let text_widget = Paragraph::new(text_lines).block(Block::default().borders(Borders::ALL));
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
    let status_widget = Paragraph::new(cells_to_line(&render_frame.status_cells))
        .style(Style::default().bg(to_rcolor(crate::theme::STATUS_BG())));
    frame.render_widget(status_widget, chunks[1]);

    // ── Widget overlay (with scroll offset) ─────────────────────────────────
    let wscroll = render_frame.widget_scroll_top;
    if let Some(ref layout) = render_frame.widget_layout {
        let inner = chunks[0].inner(ratatui::layout::Margin::new(1, 1));
        let total_h = (layout.rect.row + layout.rect.height).ceil() as u16;
        let total_w = (layout.rect.col + layout.rect.width).ceil() as u16;
        let buf_h = total_h.max(inner.height);
        let buf_w = total_w.max(inner.width);
        let mut cell_buf = widget_render::CellBuffer::new(buf_w, buf_h);
        widget_render::render_widget_tree(layout, &mut cell_buf);
        blit_cell_buffer_scrolled(
            &cell_buf,
            frame,
            inner,
            wscroll,
            render_frame.widget_scroll_left,
        );
    }

    // ── Focus highlight ──────────────────────────────────────────────────────
    if let (Some(layout), Some(focused_id)) =
        (&render_frame.widget_layout, render_frame.focused_widget_id)
    {
        if let Some(node) = find_focused_node(layout, focused_id) {
            if node.widget_type != "timeline" {
                let inner = chunks[0].inner(ratatui::layout::Margin::new(1, 1));
                let buf = frame.buffer_mut();
                let row_start = node.rect.row.floor() as i32;
                let row_end = (node.rect.row + node.rect.height).ceil() as i32;
                for row in row_start..row_end {
                    let vis_row = row - wscroll as i32;
                    if vis_row < 0 || vis_row >= inner.height as i32 {
                        continue;
                    }
                    let y = inner.y + vis_row as u16;
                    let col_start = node.rect.col.floor() as u16;
                    let col_end = (node.rect.col + node.rect.width).ceil() as u16;
                    for col in col_start..col_end {
                        let x = inner.x + col;
                        if x >= inner.right() {
                            break;
                        }
                        let cell = &mut buf[(x, y)];
                        let style = cell.style();
                        let fg = style.fg.map(|color| brighten_rcolor(color, 0.15));
                        let mut new_style = Style::default();
                        if let Some(fg) = fg {
                            new_style = new_style.fg(fg);
                        }
                        new_style = new_style.bg(to_rcolor(crate::theme::WIDGET_FOCUS_BG()));
                        cell.set_style(new_style.add_modifier(Modifier::BOLD));
                    }
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
                            .bg(to_rcolor(crate::theme::COMP_SELECTED_BG()))
                            .fg(to_rcolor(crate::theme::COMP_FG())),
                    ))
                } else {
                    Line::from(Span::styled(
                        label,
                        Style::default()
                            .bg(to_rcolor(crate::theme::COMP_UNSELECTED_BG()))
                            .fg(to_rcolor(crate::theme::COMP_FG())),
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
                        .fg(to_rcolor(crate::theme::COMP_DOC_TITLE_FG()))
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
                        .style(
                            Style::default()
                                .bg(to_rcolor(crate::theme::COMP_DOC_BG()))
                                .fg(to_rcolor(crate::theme::COMP_DOC_FG())),
                        )
                        .wrap(Wrap { trim: false }),
                    doc_area,
                );
            }
        }
    }
}

/// Blit a CellBuffer onto the ratatui frame with a vertical scroll offset.
fn blit_cell_buffer_scrolled(
    cell_buf: &widget_render::CellBuffer,
    frame: &mut Frame,
    area: Rect,
    scroll_top: u16,
    scroll_left: u16,
) {
    let buf = frame.buffer_mut();
    for (vis_row, src_row) in (scroll_top as usize..).enumerate() {
        if vis_row >= area.height as usize || src_row >= cell_buf.cells.len() {
            break;
        }
        let row = &cell_buf.cells[src_row];
        for (col_idx, cell_opt) in row.iter().enumerate().skip(scroll_left as usize) {
            if let Some(cell) = cell_opt {
                let x = area.x + (col_idx - scroll_left as usize) as u16;
                let y = area.y + vis_row as u16;
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

// ── Tiled renderer ───────────────────────────────────────────────────────────

/// Render a `TiledRenderFrame` — each tile in its sub-rect with borders.
pub fn render_tiled(frame: &mut Frame, tiled: &TiledRenderFrame) {
    let area = frame.area();

    // Render each tile
    for tile in &tiled.tiles {
        let tile_area = Rect::new(
            tile.rect.col.round() as u16,
            tile.rect.row.round() as u16,
            tile.rect.width.round() as u16,
            tile.rect.height.round() as u16,
        );

        if tile_area.width < 3 || tile_area.height < 3 {
            continue; // too small to render
        }

        render_tile_in_area(
            frame,
            &tile.frame,
            tile_area,
            tile.is_active,
            tile.show_status,
        );
    }

    // Completion popup (rendered last, on top of everything, in active tile's coord space)
    if let Some(comp) = &tiled.completion {
        // Find active tile rect for positioning
        let active_tile_area = tiled
            .tiles
            .iter()
            .find(|t| t.is_active)
            .map(|t| {
                Rect::new(
                    t.rect.col.round() as u16,
                    t.rect.row.round() as u16,
                    t.rect.width.round() as u16,
                    t.rect.height.round() as u16,
                )
            })
            .unwrap_or(area);

        render_completion_popup(frame, comp, active_tile_area);
    }
}

/// Render a single tile's content into the given area.
fn render_tile_in_area(
    frame: &mut Frame,
    render_frame: &RenderFrame,
    tile_area: Rect,
    is_active: bool,
    show_status: bool,
) {
    let border_color = if is_active {
        to_rcolor(crate::theme::PURPLE())
    } else {
        to_rcolor(Color::DARK_GRAY)
    };

    // Determine layout: content + optional per-tile status
    let (content_area, status_area) = if show_status {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(tile_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (tile_area, None)
    };

    // ── Text area with border ──────────────────────────────────────────────
    let text_lines: Vec<Line> = render_frame
        .lines
        .iter()
        .map(|row| cells_to_line(row))
        .collect();

    let text_widget = Paragraph::new(text_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    );
    frame.render_widget(text_widget, content_area);

    // ── Cursor (only for active tile) ──────────────────────────────────────
    if is_active {
        if let Some((vis_row, vis_col)) = render_frame.cursor {
            let inner_left = content_area.x.saturating_add(1);
            let inner_top = content_area.y.saturating_add(1);
            let inner_right = content_area.right().saturating_sub(2);
            let inner_bottom = content_area.bottom().saturating_sub(2);
            let cursor_x = inner_left.saturating_add(vis_col as u16).min(inner_right);
            let cursor_y = inner_top.saturating_add(vis_row as u16).min(inner_bottom);
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    // ── Per-tile status bar ────────────────────────────────────────────────
    if let Some(status_rect) = status_area {
        let status_widget = Paragraph::new(cells_to_line(&render_frame.status_cells))
            .style(Style::default().bg(to_rcolor(crate::theme::STATUS_BG())));
        frame.render_widget(status_widget, status_rect);
    }

    // ── Widget overlay ─────────────────────────────────────────────────────
    let wscroll = render_frame.widget_scroll_top;
    if let Some(ref layout) = render_frame.widget_layout {
        let inner = content_area.inner(ratatui::layout::Margin::new(1, 1));
        let total_h = (layout.rect.row + layout.rect.height).ceil() as u16;
        let total_w = (layout.rect.col + layout.rect.width).ceil() as u16;
        let buf_h = total_h.max(inner.height);
        let buf_w = total_w.max(inner.width);
        let mut cell_buf = widget_render::CellBuffer::new(buf_w, buf_h);
        widget_render::render_widget_tree(layout, &mut cell_buf);
        blit_cell_buffer_scrolled(
            &cell_buf,
            frame,
            inner,
            wscroll,
            render_frame.widget_scroll_left,
        );
    }

    // ── Focus highlight ────────────────────────────────────────────────────
    if let (Some(layout), Some(focused_id)) =
        (&render_frame.widget_layout, render_frame.focused_widget_id)
    {
        if let Some(node) = find_focused_node(layout, focused_id) {
            if node.widget_type != "timeline" {
                let inner = content_area.inner(ratatui::layout::Margin::new(1, 1));
                let buf = frame.buffer_mut();
                let row_start = node.rect.row.floor() as i32;
                let row_end = (node.rect.row + node.rect.height).ceil() as i32;
                for row in row_start..row_end {
                    let vis_row = row - wscroll as i32;
                    if vis_row < 0 || vis_row >= inner.height as i32 {
                        continue;
                    }
                    let y = inner.y + vis_row as u16;
                    let col_start = node.rect.col.floor() as u16;
                    let col_end = (node.rect.col + node.rect.width).ceil() as u16;
                    for col in col_start..col_end {
                        let x = inner.x + col;
                        if x >= inner.right() {
                            break;
                        }
                        let cell = &mut buf[(x, y)];
                        let style = cell.style();
                        let fg = style.fg.map(|color| brighten_rcolor(color, 0.15));
                        let mut new_style = Style::default();
                        if let Some(fg) = fg {
                            new_style = new_style.fg(fg);
                        }
                        new_style = new_style.bg(to_rcolor(crate::theme::WIDGET_FOCUS_BG()));
                        cell.set_style(new_style.add_modifier(Modifier::BOLD));
                    }
                }
            }
        }
    }
}

fn render_completion_popup(
    frame: &mut Frame,
    comp: &crate::backend::CompletionFrame,
    anchor_area: Rect,
) {
    let list_width = comp
        .entries
        .iter()
        .map(|e| e.label.len())
        .max()
        .unwrap_or(0)
        .max(12)
        .min((anchor_area.width / 2).max(12) as usize);

    let preferred_height = comp.entries.len() as u16;
    let (anchor_row, anchor_col) = comp.anchor;
    let popup_x = (anchor_area.x + anchor_col as u16)
        .min(anchor_area.right().saturating_sub(list_width as u16 + 1));
    let popup_y = (anchor_area.y + 1 + anchor_row as u16 + 1)
        .min(anchor_area.bottom().saturating_sub(preferred_height.max(1)));
    let list_height = preferred_height
        .min(anchor_area.bottom().saturating_sub(popup_y))
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
                        .bg(to_rcolor(crate::theme::COMP_SELECTED_BG()))
                        .fg(to_rcolor(crate::theme::COMP_FG())),
                ))
            } else {
                Line::from(Span::styled(
                    label,
                    Style::default()
                        .bg(to_rcolor(crate::theme::COMP_UNSELECTED_BG()))
                        .fg(to_rcolor(crate::theme::COMP_FG())),
                ))
            }
        })
        .collect();

    frame.render_widget(Clear, list_area);
    frame.render_widget(Paragraph::new(items), list_area);

    // Doc panel
    if let Some((title, body)) = &comp.doc {
        let available_width = anchor_area.right().saturating_sub(list_area.right() + 1);
        if available_width > 0 {
            let doc_width = available_width.min(48);
            let available_height = anchor_area.bottom().saturating_sub(popup_y).max(1);
            let doc_lines: Vec<Line> = std::iter::once(Line::from(Span::styled(
                title.clone(),
                Style::default()
                    .fg(to_rcolor(crate::theme::COMP_DOC_TITLE_FG()))
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
                    .style(
                        Style::default()
                            .bg(to_rcolor(crate::theme::COMP_DOC_BG()))
                            .fg(to_rcolor(crate::theme::COMP_DOC_FG())),
                    )
                    .wrap(Wrap { trim: false }),
                doc_area,
            );
        }
    }
}
