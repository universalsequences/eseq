use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;

use crate::effects::BUILTIN_SLOT_COUNT;

use super::effects::OverlayPickerKind;
use super::params::draw_dropdown;
use super::synth::SYNTH_COLUMN_GAP;
use super::{App, CompileTarget, EffectPaneEntry, EffectTab, InputMode, PendingCompile};

fn fit_cell(text: &str, width: usize) -> String {
    let clipped: String = text.chars().take(width).collect();
    format!("{clipped:<width$}")
}

fn effect_pane_entry_label(app: &App, entry: EffectPaneEntry) -> String {
    match entry {
        EffectPaneEntry::Tab(EffectTab::Synth) => "Synth".to_string(),
        EffectPaneEntry::Tab(EffectTab::Mod) => "Mod".to_string(),
        EffectPaneEntry::Tab(EffectTab::Sources) => "Sources".to_string(),
        EffectPaneEntry::Tab(EffectTab::Reverb) => "Reverb".to_string(),
        EffectPaneEntry::Tab(EffectTab::Slot(i)) => app
            .graph
            .effect_descriptors
            .get(app.ui.cursor_track)
            .and_then(|descs| descs.get(i))
            .map(|d| d.name.clone())
            .unwrap_or_else(|| format!("Effect {}", i + 1)),
        EffectPaneEntry::PlusButton => "+".to_string(),
    }
}

pub(super) fn draw_effects_column(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    region_focused: bool,
) {
    let tab_list_focused = region_focused && app.ui.params_column == 0;
    let editor_focused = region_focused && app.ui.params_column == 1;
    let border_style = if region_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.ui.layout.effects_block = area;
    let rail_width = inner.width.min(18).max(12);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(rail_width), Constraint::Min(10)])
        .split(inner);
    let rail = chunks[0];
    let editor = chunks[1];

    app.ui.layout.effects_tabs = rail;
    app.ui.layout.effects_inner = editor;

    if app.tracks.is_empty() || inner.height < 1 {
        return;
    }

    let entries = app.effect_pane_entries();
    app.sync_effect_tab_cursor();
    for (idx, entry) in entries.iter().copied().enumerate() {
        if idx as u16 >= rail.height {
            break;
        }
        let row = Rect::new(rail.x, rail.y + idx as u16, rail.width, 1);
        let is_cursor = idx == app.ui.effect_tab_cursor;
        let is_selected = matches!(entry, EffectPaneEntry::Tab(tab) if tab == app.ui.effect_tab);
        let style = if is_cursor && tab_list_focused {
            Style::default().fg(Color::Black).bg(Color::White).bold()
        } else if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(100, 100, 100))
        } else {
            Style::default().fg(Color::Gray)
        };
        let label = effect_pane_entry_label(app, entry);
        let text = if matches!(entry, EffectPaneEntry::PlusButton) {
            format!(
                "[ {:^width$} ]",
                label,
                width = (rail.width as usize).saturating_sub(4)
            )
        } else {
            format!(
                "  {:<width$}",
                label,
                width = (rail.width as usize).saturating_sub(2)
            )
        };
        frame.render_widget(Paragraph::new(text).style(style), row);
    }

    if rail.x + rail.width < inner.x + inner.width {
        for y in inner.y..inner.y + inner.height {
            frame.buffer_mut()[(rail.x + rail.width, y)]
                .set_symbol("│")
                .set_style(Style::default().fg(Color::Rgb(60, 60, 60)));
        }
    }

    let inner = editor;

    if app.ui.effect_tab == EffectTab::Synth {
        app.clamp_synth_scroll(inner);
        let desc = match app.graph.instrument_descriptors.get(app.ui.cursor_track) {
            Some(d) if !d.params.is_empty() => d,
            _ => return,
        };
        let synth_indices = app.synth_param_indices(app.ui.cursor_track);
        let slot = &app.state.pattern.instrument_slots[app.ui.cursor_track];
        let is_entering_value = editor_focused && app.ui.input_mode == InputMode::ValueEntry;
        let total_rows = app.synth_row_count();
        let rows_per_column = app.synth_rows_per_column(inner);
        let partition_rows = app.instrument_partition_rows_per_column(inner, total_rows);
        let column_width = app.instrument_column_width(inner, total_rows);

        for column in 0..app.instrument_column_count(inner, total_rows) {
            for local_row in 0..rows_per_column {
                let row_idx = column * partition_rows + app.ui.synth_scroll_offset + local_row;
                if row_idx >= total_rows {
                    continue;
                }
                let row_y = inner.y + local_row as u16;
                let row_x = inner.x + column as u16 * (column_width + SYNTH_COLUMN_GAP);
                let row_area = Rect::new(row_x, row_y, column_width, 1);

                let is_base_row = row_idx == 0;
                let param_idx = if is_base_row {
                    None
                } else {
                    synth_indices.get(row_idx - 1).copied()
                };
                let param_desc = if is_base_row {
                    None
                } else {
                    param_idx.and_then(|idx| desc.params.get(idx))
                };
                let default_val = if is_base_row {
                    app.instrument_base_note_offset(app.ui.cursor_track)
                } else {
                    slot.defaults
                        .get(param_idx.expect("synth param row should resolve to param idx"))
                };
                let is_cursor_row = editor_focused && app.ui.instrument_param_cursor == row_idx;
                let cursor = if is_cursor_row { "> " } else { "  " };
                let cursor_style = if is_cursor_row {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Rgb(100, 200, 140))
                };

                let label_width = if column_width >= 44 { 14 } else { 11 };
                let value_width = if column_width >= 40 { 12 } else { 9 };
                let slider_width =
                    (column_width as usize).saturating_sub(label_width + value_width + 6);
                let label = fit_cell(
                    if is_base_row {
                        "base_note"
                    } else {
                        &param_desc.expect("synth param row").name
                    },
                    label_width,
                );

                if is_cursor_row && is_entering_value {
                    let target_label = if !app.ui.visual_steps.is_empty() {
                        format!("{} steps", app.ui.visual_steps.len())
                    } else if app.ui.selection_anchor.is_some() {
                        let (lo, hi) = app.selected_range();
                        format!("steps {}-{}", lo + 1, hi + 1)
                    } else {
                        "default".to_string()
                    };
                    let spans = vec![
                        Span::styled(cursor, cursor_style),
                        Span::styled(label.clone(), Style::default().fg(Color::Gray)),
                        Span::styled(
                            format!("{}\u{2588}", app.ui.value_buffer),
                            Style::default()
                                .fg(Color::Yellow)
                                .bg(Color::Rgb(60, 60, 20))
                                .bold(),
                        ),
                        Span::styled(
                            format!("  ({target_label})"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ];
                    frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
                    continue;
                }

                let (display_val, plock_label) =
                    if !is_base_row && app.has_selection() && is_cursor_row {
                        let plock_val = slot.plocks.get(
                            app.ui.cursor_step,
                            param_idx.expect("synth param row should resolve to param idx"),
                        );
                        match plock_val {
                            Some(v) => (v, Some(" (p-lock)")),
                            None => (default_val, None),
                        }
                    } else {
                        (default_val, None)
                    };

                let formatted = if is_base_row {
                    format!("{display_val:.0} st")
                } else {
                    param_desc
                        .expect("synth param row")
                        .format_value(display_val)
                };

                let mut spans = vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(label, Style::default().fg(Color::Gray)),
                    Span::styled(fit_cell(&formatted, value_width), cursor_style),
                ];

                if let Some(lbl) = plock_label {
                    spans.push(Span::styled(lbl, Style::default().fg(Color::White)));
                }

                if is_base_row || !param_desc.expect("synth param row").is_boolean() {
                    let range = if is_base_row {
                        96.0
                    } else {
                        let param_desc = param_desc.expect("synth param row");
                        param_desc.max - param_desc.min
                    };
                    if range > 0.0 {
                        let (slider_val, is_plock) = if is_base_row {
                            (default_val, false)
                        } else if app.has_selection() {
                            let pv = slot.plocks.get(
                                app.ui.cursor_step,
                                param_idx.expect("synth param row should resolve to param idx"),
                            );
                            (pv.unwrap_or(default_val), pv.is_some())
                        } else if app.state.is_playing() {
                            let step = app.state.track_step(app.ui.cursor_track);
                            let pv = slot.plocks.get(
                                step,
                                param_idx.expect("synth param row should resolve to param idx"),
                            );
                            (pv.unwrap_or(default_val), pv.is_some())
                        } else {
                            (default_val, false)
                        };
                        let norm = if is_base_row {
                            ((slider_val + 48.0_f32) / 96.0_f32).clamp(0.0_f32, 1.0_f32)
                        } else {
                            param_desc.expect("synth param row").normalize(slider_val)
                        };
                        if slider_width > 2 {
                            let filled =
                                ((norm * slider_width as f32).round() as usize).min(slider_width);
                            let bar: String =
                                "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                            let slider_color = if is_plock {
                                Color::Cyan
                            } else {
                                Color::Rgb(100, 200, 140)
                            };
                            spans.push(Span::styled(
                                format!("[{bar}]"),
                                Style::default().fg(slider_color),
                            ));
                        }
                    }
                }

                frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
            }
        }

        if partition_rows > rows_per_column && inner.width > 12 {
            let end = (app.ui.synth_scroll_offset + rows_per_column).min(partition_rows);
            let summary = format!(
                "{}-{}/{}",
                app.ui.synth_scroll_offset + 1,
                end,
                partition_rows
            );
            let summary_len = summary.len() as u16;
            let x = inner.x + inner.width.saturating_sub(summary_len);
            frame.render_widget(
                Paragraph::new(summary).style(Style::default().fg(Color::DarkGray)),
                Rect::new(x, inner.y, summary_len, 1),
            );
        }

        if app.ui.dropdown_open && editor_focused {
            draw_dropdown(frame, app, inner);
        }
        return;
    }

    if app.ui.effect_tab == EffectTab::Mod {
        app.clamp_mod_scroll(inner);
        let desc = match app.current_mod_descriptor() {
            Some(d) if !d.params.is_empty() => d,
            _ => return,
        };
        let track = app.ui.cursor_track;
        let mod_indices = app.mod_param_indices(track);
        let slot = &app.state.pattern.instrument_slots[track];
        let is_entering_value = editor_focused && app.ui.input_mode == InputMode::ValueEntry;
        let total_rows = app.mod_row_count();
        let rows_per_column = app.synth_rows_per_column(inner);
        let partition_rows = app.instrument_partition_rows_per_column(inner, total_rows);
        let column_width = app.instrument_column_width(inner, total_rows);

        for column in 0..app.instrument_column_count(inner, total_rows) {
            for local_row in 0..rows_per_column {
                let row_idx = column * partition_rows + app.ui.mod_scroll_offset + local_row;
                if row_idx >= total_rows {
                    continue;
                }
                let row_y = inner.y + local_row as u16;
                let row_x = inner.x + column as u16 * (column_width + SYNTH_COLUMN_GAP);
                let row_area = Rect::new(row_x, row_y, column_width, 1);

                let actual_idx = mod_indices[row_idx];
                let param_desc = &desc.params[row_idx];
                let is_cursor_row = editor_focused && app.ui.mod_param_cursor == row_idx;
                let (display_stored, is_plock) = if app.has_selection() && is_cursor_row {
                    let step = app.ui.cursor_step;
                    match slot.plocks.get(step, actual_idx) {
                        Some(v) => (v, true),
                        None => (slot.defaults.get(actual_idx), false),
                    }
                } else {
                    (slot.defaults.get(actual_idx), false)
                };
                let cursor = if is_cursor_row { "> " } else { "  " };
                let cursor_style = if is_cursor_row {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Rgb(100, 200, 140))
                };
                let label_width = if column_width >= 44 { 16 } else { 13 };
                let value_width = if column_width >= 40 { 12 } else { 9 };
                let slider_width =
                    (column_width as usize).saturating_sub(label_width + value_width + 6);
                let label = fit_cell(&param_desc.name, label_width);

                if is_cursor_row && is_entering_value {
                    let spans = vec![
                        Span::styled(cursor, cursor_style),
                        Span::styled(label.clone(), Style::default().fg(Color::Gray)),
                        Span::styled(
                            format!("{}\u{2588}", app.ui.value_buffer),
                            Style::default()
                                .fg(Color::Yellow)
                                .bg(Color::Rgb(60, 60, 20))
                                .bold(),
                        ),
                    ];
                    frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
                    continue;
                }

                let display_val = param_desc.format_value(display_stored);
                let norm = param_desc.normalize(display_stored);
                let fill = ((slider_width as f32 * norm).round() as usize).min(slider_width);
                let bar = format!(
                    "{}{}",
                    "─".repeat(fill),
                    " ".repeat(slider_width.saturating_sub(fill))
                );
                let label_style = if is_cursor_row {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let value_style = if is_plock {
                    Style::default().fg(Color::Cyan)
                } else if is_cursor_row {
                    Style::default().fg(Color::Rgb(255, 210, 120))
                } else {
                    Style::default().fg(Color::Rgb(100, 200, 140))
                };
                let bar_style = Style::default().fg(Color::Rgb(90, 220, 180));
                let line = Line::from(vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(label, label_style),
                    Span::raw(" "),
                    Span::styled(fit_cell(&display_val, value_width), value_style),
                    Span::raw(" "),
                    Span::styled(format!("[{}]", bar), bar_style),
                ]);
                frame.render_widget(Paragraph::new(line), row_area);
            }
        }
        if partition_rows > rows_per_column && inner.width > 12 {
            let end = (app.ui.mod_scroll_offset + rows_per_column).min(partition_rows);
            let summary = format!(
                "{}-{}/{}",
                app.ui.mod_scroll_offset + 1,
                end,
                partition_rows
            );
            let summary_len = summary.len() as u16;
            let x = inner.x + inner.width.saturating_sub(summary_len);
            frame.render_widget(
                Paragraph::new(summary).style(Style::default().fg(Color::DarkGray)),
                Rect::new(x, inner.y, summary_len, 1),
            );
        }
        if app.ui.dropdown_open && editor_focused {
            draw_dropdown(frame, app, inner);
        }
        return;
    }

    if app.ui.effect_tab == EffectTab::Sources {
        app.clamp_source_scroll(inner);
        let desc = match app.current_source_descriptor() {
            Some(d) if !d.params.is_empty() => d,
            _ => return,
        };
        let track = app.ui.cursor_track;
        let source_indices = app.source_param_actual_indices(track);
        let slot = &app.state.pattern.instrument_slots[track];
        let is_entering_value = editor_focused && app.ui.input_mode == InputMode::ValueEntry;
        let total_rows = app.source_row_count();
        let rows_per_column = app.synth_rows_per_column(inner);
        let partition_rows = app.instrument_partition_rows_per_column(inner, total_rows);
        let column_width = app.instrument_column_width(inner, total_rows);

        for column in 0..app.instrument_column_count(inner, total_rows) {
            for local_row in 0..rows_per_column {
                let display_row = column * partition_rows + app.ui.source_scroll_offset + local_row;
                if display_row >= total_rows {
                    continue;
                }
                let row_y = inner.y + local_row as u16;
                let row_x = inner.x + column as u16 * (column_width + SYNTH_COLUMN_GAP);
                let row_area = Rect::new(row_x, row_y, column_width, 1);

                let header = app
                    .source_display_rows(track)
                    .get(display_row)
                    .and_then(|(header, _)| *header);

                if let Some(header) = header {
                    let line = Line::from(vec![Span::styled(
                        header,
                        Style::default().fg(Color::Rgb(220, 180, 120)).bold(),
                    )]);
                    frame.render_widget(Paragraph::new(line), row_area);
                    continue;
                }

                let Some(row_idx) = app.source_param_row_for_display(display_row) else {
                    continue;
                };
                let actual_idx = source_indices[row_idx];
                let param_desc = &desc.params[row_idx];
                let is_cursor_row = editor_focused && app.ui.source_param_cursor == row_idx;
                let (display_stored, is_plock) = if app.has_selection() && is_cursor_row {
                    let step = app.ui.cursor_step;
                    match slot.plocks.get(step, actual_idx) {
                        Some(v) => (v, true),
                        None => (slot.defaults.get(actual_idx), false),
                    }
                } else {
                    (slot.defaults.get(actual_idx), false)
                };
                let cursor = if is_cursor_row { "> " } else { "  " };
                let cursor_style = if is_cursor_row {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Rgb(220, 180, 120))
                };
                let label_width = if column_width >= 44 { 16 } else { 13 };
                let value_width = if column_width >= 40 { 12 } else { 9 };
                let slider_width =
                    (column_width as usize).saturating_sub(label_width + value_width + 6);
                let label = fit_cell(&param_desc.name, label_width);

                if is_cursor_row && is_entering_value {
                    let spans = vec![
                        Span::styled(cursor, cursor_style),
                        Span::styled(label.clone(), Style::default().fg(Color::Gray)),
                        Span::styled(
                            format!("{}\u{2588}", app.ui.value_buffer),
                            Style::default()
                                .fg(Color::Yellow)
                                .bg(Color::Rgb(60, 60, 20))
                                .bold(),
                        ),
                    ];
                    frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
                    continue;
                }

                let display_val = param_desc.format_value(display_stored);
                let norm = param_desc.normalize(display_stored);
                let fill = ((slider_width as f32 * norm).round() as usize).min(slider_width);
                let bar = format!(
                    "{}{}",
                    "─".repeat(fill),
                    " ".repeat(slider_width.saturating_sub(fill))
                );
                let label_style = if is_cursor_row {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let value_style = if is_plock {
                    Style::default().fg(Color::Cyan)
                } else if is_cursor_row {
                    Style::default().fg(Color::Rgb(255, 220, 140))
                } else {
                    Style::default().fg(Color::Rgb(220, 180, 120))
                };
                let bar_style = Style::default().fg(Color::Rgb(220, 180, 120));
                let line = Line::from(vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(label, label_style),
                    Span::raw(" "),
                    Span::styled(fit_cell(&display_val, value_width), value_style),
                    Span::raw(" "),
                    Span::styled(format!("[{}]", bar), bar_style),
                ]);
                frame.render_widget(Paragraph::new(line), row_area);
            }
        }
        if partition_rows > rows_per_column && inner.width > 12 {
            let end = (app.ui.source_scroll_offset + rows_per_column).min(partition_rows);
            let summary = format!(
                "{}-{}/{}",
                app.ui.source_scroll_offset + 1,
                end,
                partition_rows
            );
            let summary_len = summary.len() as u16;
            let x = inner.x + inner.width.saturating_sub(summary_len);
            frame.render_widget(
                Paragraph::new(summary).style(Style::default().fg(Color::DarkGray)),
                Rect::new(x, inner.y, summary_len, 1),
            );
        }
        if app.ui.dropdown_open && editor_focused {
            draw_dropdown(frame, app, inner);
        }
        return;
    }

    if app.ui.effect_tab == EffectTab::Reverb {
        let reverb_params: [(&str, f32); 3] = [
            ("size", app.ui.reverb_size),
            ("brightness", app.ui.reverb_brightness),
            ("replace", app.ui.reverb_replace),
        ];
        let is_entering_value = editor_focused && app.ui.input_mode == InputMode::ValueEntry;

        for (i, (name, val)) in reverb_params.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let row_y = inner.y + i as u16;
            let is_cursor_row = editor_focused && app.ui.reverb_param_cursor == i;
            let cursor = if is_cursor_row { "> " } else { "  " };
            let cursor_style = if is_cursor_row {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Rgb(180, 140, 220))
            };

            let label_width = 12;
            let value_width = 14;

            if is_cursor_row && is_entering_value {
                let spans = vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(
                        format!("{name:<width$}", width = label_width),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::styled(
                        format!("{}\u{2588}", app.ui.value_buffer),
                        Style::default()
                            .fg(Color::Yellow)
                            .bg(Color::Rgb(60, 60, 20))
                            .bold(),
                    ),
                    Span::styled(
                        "  Enter: set  Esc: cancel",
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(inner.x, row_y, inner.width, 1),
                );
                continue;
            }

            let formatted = format!("{val:.2}");
            let norm = val.clamp(0.0, 1.0);
            let mut spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(
                    format!("{name:<width$}", width = label_width),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("{formatted:<width$}", width = value_width),
                    cursor_style,
                ),
            ];

            let slider_width = (inner.width as usize).saturating_sub(label_width + value_width + 6);
            if slider_width > 2 {
                let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                let bar: String = "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                spans.push(Span::styled(
                    format!("[{bar}]"),
                    Style::default().fg(Color::Rgb(160, 130, 200)),
                ));
            }

            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(inner.x, row_y, inner.width, 1),
            );
        }
        return;
    }

    let track = app.ui.cursor_track;
    let Some(slot_idx) = app.selected_effect_slot() else {
        return;
    };
    app.clamp_effect_scroll(inner);

    let is_custom_slot = slot_idx >= BUILTIN_SLOT_COUNT;
    let chain = &app.state.pattern.effect_chains[track];
    let has_node = if slot_idx < chain.len() {
        chain[slot_idx].node_id.load(Ordering::Relaxed) != 0
    } else {
        false
    };

    if is_custom_slot && !has_node {
        let hint = Line::from(vec![
            Span::styled("  Ctrl+L", Style::default().fg(Color::White).bold()),
            Span::styled(" to add effect", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(
            Paragraph::new(hint),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    }

    let desc = match app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|d| d.get(slot_idx))
    {
        Some(d) => d,
        None => return,
    };

    if slot_idx >= chain.len() {
        return;
    }
    let slot = &chain[slot_idx];
    let is_entering_value = editor_focused && app.ui.input_mode == InputMode::ValueEntry;
    let total_rows = desc.params.len();
    let rows_per_column = app.synth_rows_per_column(inner);
    let partition_rows = app.instrument_partition_rows_per_column(inner, total_rows);
    let column_width = app.instrument_column_width(inner, total_rows);

    for column in 0..app.instrument_column_count(inner, total_rows) {
        for local_row in 0..rows_per_column {
            let i = column * partition_rows + app.ui.effect_scroll_offset + local_row;
            if i >= total_rows {
                continue;
            }

            let row_y = inner.y + local_row as u16;
            let row_x = inner.x + column as u16 * (column_width + SYNTH_COLUMN_GAP);
            let row_area = Rect::new(row_x, row_y, column_width, 1);
            let param_desc = &desc.params[i];
            let default_val = slot.defaults.get(i);
            let is_cursor_row = editor_focused && app.ui.effect_param_cursor == i;
            let cursor = if is_cursor_row { "> " } else { "  " };
            let cursor_style = if is_cursor_row {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            let label_width = if column_width >= 44 { 14 } else { 11 };
            let value_width = if column_width >= 40 { 12 } else { 9 };
            let slider_width =
                (column_width as usize).saturating_sub(label_width + value_width + 6);
            let label = fit_cell(&param_desc.name, label_width);

            if is_cursor_row && is_entering_value {
                let target_label = if !app.ui.visual_steps.is_empty() {
                    format!("p-lock {} steps", app.ui.visual_steps.len())
                } else if app.ui.selection_anchor.is_some() {
                    let (lo, hi) = app.selected_range();
                    format!("p-lock steps {}-{}", lo + 1, hi + 1)
                } else {
                    "default".to_string()
                };
                let spans = vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(label.clone(), Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("{}\u{2588}", app.ui.value_buffer),
                        Style::default()
                            .fg(Color::Yellow)
                            .bg(Color::Rgb(60, 60, 20))
                            .bold(),
                    ),
                    Span::styled(
                        format!("  ({target_label})"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
                continue;
            }

            let (display_val, plock_label) = if app.has_selection() && is_cursor_row {
                let plock_val = slot.plocks.get(app.ui.cursor_step, i);
                match plock_val {
                    Some(v) => (v, Some(" (p-lock)")),
                    None => (default_val, None),
                }
            } else {
                (default_val, None)
            };

            let formatted = param_desc.format_value(display_val);
            let mut spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(label, Style::default().fg(Color::Gray)),
                Span::styled(fit_cell(&formatted, value_width), cursor_style),
            ];

            if let Some(lbl) = plock_label {
                spans.push(Span::styled(lbl, Style::default().fg(Color::White)));
            }

            if !param_desc.is_boolean() {
                let range = param_desc.max - param_desc.min;
                if range > 0.0 && slider_width > 2 {
                    let (slider_val, is_plock) = if app.has_selection() {
                        let pv = slot.plocks.get(app.ui.cursor_step, i);
                        (pv.unwrap_or(default_val), pv.is_some())
                    } else if app.state.is_playing() {
                        let step = app.state.track_step(app.ui.cursor_track);
                        let pv = slot.plocks.get(step, i);
                        (pv.unwrap_or(default_val), pv.is_some())
                    } else {
                        (default_val, false)
                    };
                    let norm = param_desc.normalize(slider_val);
                    let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                    let bar: String =
                        "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                    let slider_color = if is_plock { Color::Cyan } else { Color::White };
                    spans.push(Span::styled(
                        format!("[{bar}]"),
                        Style::default().fg(slider_color),
                    ));
                }
            }

            frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
        }
    }

    if partition_rows > rows_per_column && inner.width > 12 {
        let end = (app.ui.effect_scroll_offset + rows_per_column).min(partition_rows);
        let summary = format!(
            "{}-{}/{}",
            app.ui.effect_scroll_offset + 1,
            end,
            partition_rows
        );
        let summary_len = summary.len() as u16;
        let x = inner.x + inner.width.saturating_sub(summary_len);
        frame.render_widget(
            Paragraph::new(summary).style(Style::default().fg(Color::DarkGray)),
            Rect::new(x, inner.y, summary_len, 1),
        );
    }

    if app.ui.dropdown_open && editor_focused {
        draw_dropdown(frame, app, inner);
    }
}

fn draw_overlay_picker(frame: &mut Frame, app: &App, area: Rect, kind: OverlayPickerKind) {
    let items = app.filtered_overlay_items(kind);
    let max_visible = 10usize;
    let list_height = items.len().min(max_visible) as u16;
    let w = 36u16;
    let h = list_height + 4;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let picker_area = Rect::new(x, y, w, h);

    let bg = Style::default().bg(Color::Rgb(20, 20, 20));
    for row in 0..h {
        frame.render_widget(
            Paragraph::new(" ".repeat(w as usize)).style(bg),
            Rect::new(x, y + row, w, 1),
        );
    }

    let title = match kind {
        OverlayPickerKind::Effect => " Effects ",
        OverlayPickerKind::Instrument => " Instruments ",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(picker_area);
    frame.render_widget(block, picker_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let filter_text = format!(" > {}\u{2588}", app.editor.picker_filter);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            filter_text,
            Style::default().fg(Color::White),
        ))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let list_start_y = inner.y + 1;
    let scroll_offset = app
        .editor
        .picker_cursor
        .saturating_sub(max_visible.saturating_sub(1));
    for (visible_idx, item) in items
        .iter()
        .skip(scroll_offset)
        .take(max_visible)
        .enumerate()
    {
        let item_idx = scroll_offset + visible_idx;
        let row_y = list_start_y + visible_idx as u16;
        if row_y >= inner.y + inner.height {
            break;
        }
        let is_cursor = item_idx == app.editor.picker_cursor;
        let is_new_item = item == App::overlay_new_label(kind);
        let display_text = if matches!(kind, OverlayPickerKind::Instrument) && !is_new_item {
            app.instrument_picker_label(item)
        } else {
            item.to_string()
        };
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if is_new_item {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Gray)
        };
        let prefix = if is_cursor { " > " } else { "   " };
        let truncated: String = display_text
            .chars()
            .take((inner.width as usize).saturating_sub(4))
            .collect();
        let text = format!(
            "{}{:<width$}",
            prefix,
            truncated,
            width = (inner.width as usize).saturating_sub(3)
        );
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(inner.x, row_y, inner.width, 1),
        );
    }
}

pub(super) fn draw_effect_picker(frame: &mut Frame, app: &App, area: Rect) {
    draw_overlay_picker(frame, app, area, OverlayPickerKind::Effect);
}

pub(super) fn draw_compiling_overlay(frame: &mut Frame, pending: &PendingCompile, area: Rect) {
    const SPINNER: &[char] = &[
        '\u{28F7}', '\u{28EF}', '\u{28DF}', '\u{287F}', '\u{28BF}', '\u{28FB}', '\u{28FD}',
        '\u{28FE}',
    ];
    let spin = SPINNER[pending.tick / 2 % SPINNER.len()];
    let name = match &pending.target {
        CompileTarget::Effect { name, .. } | CompileTarget::Instrument { name } => name,
    };
    let name_display = if name.len() > 14 {
        format!("{}...", &name[..11])
    } else {
        name.clone()
    };

    let w = 20u16;
    let h = 4u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let overlay = Rect::new(x, y, w, h);

    let bg = Style::default().bg(Color::Rgb(20, 20, 20));
    for row in 0..h {
        frame.render_widget(
            Paragraph::new(" ".repeat(w as usize)).style(bg),
            Rect::new(x, y + row, w, 1),
        );
    }

    let block = Block::default()
        .title(" Compiling ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    if inner.height >= 2 && inner.width >= 4 {
        let line = Line::from(Span::styled(
            format!("  {spin} {name_display}  "),
            Style::default().fg(Color::Yellow),
        ));
        let center_y = inner.y + inner.height / 2;
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(inner.x, center_y, inner.width, 1),
        );
    }
}

pub(super) fn draw_project_loading_overlay(frame: &mut Frame, name: &str, tick: usize, area: Rect) {
    const SPINNER: &[char] = &[
        '\u{28F7}', '\u{28EF}', '\u{28DF}', '\u{287F}', '\u{28BF}', '\u{28FB}', '\u{28FD}',
        '\u{28FE}',
    ];
    let spin = SPINNER[tick / 2 % SPINNER.len()];
    let name_display = if name.len() > 14 {
        format!("{}...", &name[..11])
    } else {
        name.to_string()
    };

    let w = 20u16;
    let h = 4u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let overlay = Rect::new(x, y, w, h);

    let bg = Style::default().bg(Color::Rgb(20, 20, 20));
    for row in 0..h {
        frame.render_widget(
            Paragraph::new(" ".repeat(w as usize)).style(bg),
            Rect::new(x, y + row, w, 1),
        );
    }

    let block = Block::default()
        .title(" Opening ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    if inner.height >= 2 && inner.width >= 4 {
        let line = Line::from(Span::styled(
            format!("  {spin} {name_display}  "),
            Style::default().fg(Color::Yellow),
        ));
        let center_y = inner.y + inner.height / 2;
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(inner.x, center_y, inner.width, 1),
        );
    }
}

pub(super) fn draw_instrument_picker(frame: &mut Frame, app: &App, area: Rect) {
    draw_overlay_picker(frame, app, area, OverlayPickerKind::Instrument);
}
