use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

use crate::sequencer::{StepParam, STEPS_PER_PAGE};

use super::command::{apply_command, AppCommand};
use super::draw::{is_in_selection, param_color, region_border_style};
use super::{App, InputMode, Region, BAR_HEIGHT, COL_WIDTH};

pub(super) struct TrackListRowLayout {
    pub name_x: u16,
    pub name_width: u16,
    pub volume_x: u16,
    pub volume_width: u16,
    pub volume_inner_x: u16,
    pub volume_inner_width: u16,
    pub db_x: u16,
    pub arm_x: u16,
}

pub(super) fn track_list_row_layout(area: Rect) -> TrackListRowLayout {
    let arm_width = 2u16;
    let db_width = 5u16;
    let volume_inner_width = area.width.saturating_sub(20).clamp(9, 15);
    let volume_width = volume_inner_width + 2;
    let reserved = 3 + volume_width + 1 + db_width + 1 + arm_width;
    let name_width = area.width.saturating_sub(reserved).max(3);
    let name_x = area.x;
    let volume_x = name_x + name_width + 1;
    let db_x = volume_x + volume_width + 1;
    let arm_x = db_x + db_width + 1;
    TrackListRowLayout {
        name_x,
        name_width,
        volume_x,
        volume_width,
        volume_inner_x: volume_x + 1,
        volume_inner_width,
        db_x,
        arm_x,
    }
}

fn gain_db_label(volume: f32) -> String {
    if volume <= 0.0001 {
        "-inf".to_string()
    } else {
        format!("{:+}dB", (20.0 * volume.log10()).round() as i32)
    }
}

// ── App impl: cirklon input ──

impl App {
    pub(super) fn handle_cirklon_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.touch_follow_timer();
        let has_shift = modifiers.contains(KeyModifiers::SHIFT);
        let has_alt = modifiers.contains(KeyModifiers::ALT);
        let ns = self.num_steps();

        match code {
            // Option+Left/Right: beat jump (4 steps)
            KeyCode::Left if has_alt => {
                self.ui.cursor_step = self.ui.cursor_step.saturating_sub(4);
            }
            KeyCode::Right if has_alt => {
                self.ui.cursor_step = (self.ui.cursor_step + 4).min(ns - 1);
            }

            // Shift+Left/Right: extend selection
            KeyCode::Left if has_shift => {
                if self.ui.selection_anchor.is_none() {
                    self.ui.selection_anchor = Some(self.ui.cursor_step);
                }
                if self.ui.cursor_step > 0 {
                    self.ui.cursor_step -= 1;
                }
            }
            KeyCode::Right if has_shift => {
                if self.ui.selection_anchor.is_none() {
                    self.ui.selection_anchor = Some(self.ui.cursor_step);
                }
                if self.ui.cursor_step < ns - 1 {
                    self.ui.cursor_step += 1;
                }
            }

            KeyCode::Left => {
                if !self.ui.visual_steps.is_empty() {
                    let mut steps: Vec<usize> = self.ui.visual_steps.iter().copied().collect();
                    steps.sort();
                    let track = self.ui.cursor_track;
                    apply_command(
                        self,
                        AppCommand::RotateSteps {
                            track,
                            steps,
                            direction: -1,
                        },
                    );
                } else if self.ui.selection_anchor.is_some() {
                    self.shift_selection(-1);
                } else if self.ui.cursor_step > 0 {
                    self.ui.cursor_step -= 1;
                } else {
                    self.ui.cursor_step = ns - 1;
                }
            }
            KeyCode::Right => {
                if !self.ui.visual_steps.is_empty() {
                    let mut steps: Vec<usize> = self.ui.visual_steps.iter().copied().collect();
                    steps.sort();
                    let track = self.ui.cursor_track;
                    apply_command(
                        self,
                        AppCommand::RotateSteps {
                            track,
                            steps,
                            direction: 1,
                        },
                    );
                } else if self.ui.selection_anchor.is_some() {
                    self.shift_selection(1);
                } else {
                    self.ui.cursor_step = (self.ui.cursor_step + 1) % ns;
                }
            }

            // Shift+Up/Down: adjust step param value
            KeyCode::Up if has_shift => {
                self.adjust_selected(self.ui.active_param.increment());
            }
            KeyCode::Down if has_shift => {
                self.adjust_selected(-self.ui.active_param.increment());
            }

            // Up/Down: switch tracks
            KeyCode::Up => {
                if self.ui.cursor_track > 0 {
                    self.ui.cursor_track -= 1;
                } else if !self.tracks.is_empty() {
                    self.ui.cursor_track = self.tracks.len() - 1;
                }
                self.ui.track_selection_anchor = None;
                self.clamp_cursor_to_steps();
                self.browser.sync_to_track(
                    &self.tracks,
                    self.ui.cursor_track,
                    self.is_sampler_track(self.ui.cursor_track),
                    &self.ui,
                );
            }
            KeyCode::Down => {
                if !self.tracks.is_empty() {
                    self.ui.cursor_track = (self.ui.cursor_track + 1) % self.tracks.len();
                }
                self.ui.track_selection_anchor = None;
                self.clamp_cursor_to_steps();
                self.browser.sync_to_track(
                    &self.tracks,
                    self.ui.cursor_track,
                    self.is_sampler_track(self.ui.cursor_track),
                    &self.ui,
                );
            }

            KeyCode::Enter => {
                if !self.tracks.is_empty() {
                    let track = self.ui.cursor_track;
                    for step in self.selected_steps() {
                        apply_command(self, AppCommand::ToggleStep { track, step });
                    }
                }
            }

            KeyCode::Backspace | KeyCode::Delete => {
                if !self.tracks.is_empty() {
                    let track = self.ui.cursor_track;
                    if self.has_selection() {
                        // With selection: clear all selected steps completely (including plocks)
                        let steps = self.selected_steps();
                        apply_command(self, AppCommand::ClearSteps { track, steps });
                        self.ui.selection_anchor = None;
                        self.ui.visual_steps.clear();
                    } else {
                        // No selection: just deactivate cursor step, no plock clear
                        apply_command(
                            self,
                            AppCommand::SetStepActive {
                                track,
                                step: self.ui.cursor_step,
                                active: false,
                            },
                        );
                    }
                }
            }

            KeyCode::Char('+') => {
                if !self.tracks.is_empty() {
                    let track = self.ui.cursor_track;
                    let old_len = self.num_steps();
                    apply_command(self, AppCommand::DuplicateTrackPattern { track });
                    let new_len = self.num_steps();
                    if new_len == old_len {
                        self.editor.status_message = Some((
                            format!("Already at max ({} steps)", new_len),
                            Instant::now(),
                        ));
                    } else {
                        self.editor.status_message = Some((
                            format!("Pattern doubled to {} steps", new_len),
                            Instant::now(),
                        ));
                    }
                    self.clamp_cursor_to_steps();
                }
            }
            KeyCode::Char('_') => {
                if !self.tracks.is_empty() {
                    let track = self.ui.cursor_track;
                    let old_len = self.num_steps();
                    apply_command(self, AppCommand::HalveTrackPattern { track });
                    let new_len = self.num_steps();
                    if new_len == old_len {
                        self.editor.status_message =
                            Some(("Already at minimum (1 step)".to_string(), Instant::now()));
                    } else {
                        self.editor.status_message = Some((
                            format!("Pattern halved to {} steps", new_len),
                            Instant::now(),
                        ));
                    }
                    self.clamp_cursor_to_steps();
                }
            }
            KeyCode::Char('.') => {
                self.ui.value_buffer.clear();
                self.ui.value_buffer.push_str("0.");
                self.ui.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char('-') => {
                self.ui.value_buffer.clear();
                self.ui.value_buffer.push('-');
                self.ui.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.ui.value_buffer.clear();
                self.ui.value_buffer.push(c);
                self.ui.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char('b') => {
                self.ui.bpm_entry = true;
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char('P') => {
                self.ui.input_mode = InputMode::PatternSelect;
                self.ui.value_buffer.clear();
                self.ui.pattern_clone_pending = false;
            }
            KeyCode::Char(']') => {
                let total_pages = ns.div_ceil(STEPS_PER_PAGE);
                if total_pages > 1 {
                    let current_page = self.current_page();
                    self.ui.cursor_step = ((current_page + 1) % total_pages) * STEPS_PER_PAGE;
                }
            }
            KeyCode::Char('[') => {
                let total_pages = ns.div_ceil(STEPS_PER_PAGE);
                if total_pages > 1 {
                    let current_page = self.current_page();
                    self.ui.cursor_step =
                        ((current_page + total_pages - 1) % total_pages) * STEPS_PER_PAGE;
                }
            }
            KeyCode::Char(c) => {
                if let Some(param) = StepParam::from_hotkey(c) {
                    if StepParam::VISIBLE.contains(&param) {
                        self.ui.active_param = param;
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Drawing ──

pub(super) fn draw_cirklon_region(frame: &mut Frame, app: &mut App, area: Rect) {
    app.ui.layout.cirklon_area = area;

    let mode_label = match app.ui.input_mode {
        super::InputMode::StepInsert => Some(" INSERT "),
        super::InputMode::StepSelect => Some(" SELECT "),
        super::InputMode::StepArm => Some(" ARM "),
        _ => None,
    };
    let title_line = if let Some(label) = mode_label {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Smooth sine-wave pulse between 192 (75%) and 255 (100%) brightness
        let t = (ms as f64) / 800.0 * std::f64::consts::TAU;
        let brightness = 192.0 + 63.0 * (0.5 + 0.5 * t.sin());
        let b = brightness as u8;
        let mode_style = Style::default().fg(Color::Rgb(b, b, b)).bold();
        Line::from(vec![
            Span::raw(" Cirklon "),
            Span::styled(label, mode_style),
        ])
    } else {
        Line::from(" Cirklon ")
    };
    let block = Block::default()
        .title(title_line)
        .borders(Borders::ALL)
        .border_style(region_border_style(app, Region::Cirklon));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 {
        return;
    }

    // Horizontal split: sequencer content | track list
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),     // Sequencer content
            Constraint::Length(35), // Track list + mixer column
        ])
        .split(inner);

    app.ui.layout.track_list = h_chunks[1];

    draw_track_list(frame, app, h_chunks[1]);

    // Sequencer content vertical layout
    let seq_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                 // param tabs
            Constraint::Length(BAR_HEIGHT as u16), // bars
            Constraint::Length(2),                 // trigger + step numbers
            Constraint::Length(2),                 // page blocks + playhead dot
            Constraint::Length(1),                 // value line
            Constraint::Length(1),                 // spacer
            Constraint::Length(3),                 // piano keyboard
        ])
        .split(h_chunks[0]);

    app.ui.layout.param_tabs = seq_chunks[0];
    app.ui.layout.bars = seq_chunks[1];
    app.ui.layout.trigger_row = seq_chunks[2];
    app.ui.layout.page_blocks_area = seq_chunks[3];

    draw_param_tabs(frame, app, seq_chunks[0]);
    draw_bars(frame, app, seq_chunks[1]);
    draw_trigger_row(frame, app, seq_chunks[2]);
    draw_page_blocks(frame, app, seq_chunks[3]);
    draw_value_line(frame, app, seq_chunks[4]);
    // seq_chunks[5] is the spacer row
    draw_piano_roll(frame, app, seq_chunks[6]);
}

fn draw_track_list(frame: &mut Frame, app: &App, area: Rect) {
    // Clear the entire track list area first to prevent stale content
    let buf = frame.buffer_mut();
    for y in area.y..(area.y + area.height) {
        for x in area.x..(area.x + area.width) {
            buf[(x, y)].reset();
        }
    }

    for (i, name) in app.tracks.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let y = area.y + i as u16;
        let layout = track_list_row_layout(area);
        let is_selected = i == app.ui.cursor_track;
        let in_track_selection = if app.has_track_selection() {
            let (lo, hi) = app.track_selected_range();
            i >= lo && i <= hi
        } else {
            false
        };
        let sanitized: String = name
            .chars()
            .filter(|c| {
                !c.is_control() && unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0) > 0
            })
            .collect();
        let trimmed = sanitized.trim_start();

        // Arm indicator: large record dot, closer to a mixer/DAW affordance
        let armed = i < app.graph.record_armed.len() && app.graph.record_armed[i];
        let arm_sym = if armed { "\u{25cf}" } else { "\u{25cf}" };
        let arm_style = if armed {
            Style::default().fg(Color::Red).bold()
        } else {
            Style::default().fg(Color::Rgb(70, 70, 70))
        };

        let prefix = format!("{:>2} ", i + 1);
        let available_name = layout
            .name_width
            .saturating_sub(UnicodeWidthStr::width(prefix.as_str()) as u16)
            as usize;
        let truncated: String = trimmed.chars().take(available_name).collect();
        let label = format!("{prefix}{truncated:<width$}", width = available_name);
        let tp = &app.state.pattern.track_params[i];
        let volume = tp.get_volume().clamp(0.0, 1.0);
        let filled = (volume * layout.volume_inner_width as f32).round() as usize;
        let filled = filled.min(layout.volume_inner_width as usize);
        let vol_bar = format!(
            "[{}{}]",
            "\u{2501}".repeat(filled),
            " ".repeat(layout.volume_inner_width as usize - filled)
        );
        let db_label = format!("{:>5}", gain_db_label(volume));

        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).bold()
        } else if in_track_selection {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(170, 145, 70))
                .bold()
        } else {
            // Flash decay: read value, decay by 30 per frame, interpolate gray(90)→white(255)
            let flash = app.state.transport.trigger_flash[i].load(Ordering::Relaxed);
            let decayed = flash.saturating_sub(30);
            app.state.transport.trigger_flash[i].store(decayed, Ordering::Relaxed);
            let base = 90u8;
            let brightness = base + ((255 - base) as u32 * decayed / 255) as u8;
            Style::default().fg(Color::Rgb(brightness, brightness, brightness))
        };

        // Write directly to buffer using set_string for proper unicode width handling
        let clip = |s: &str, max_width: u16| -> String {
            let mut out = String::new();
            let mut width = 0u16;
            for ch in s.chars() {
                let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
                if width + ch_w > max_width {
                    break;
                }
                out.push(ch);
                width += ch_w;
            }
            out
        };

        let buf = frame.buffer_mut();
        buf.set_string(area.x, y, " ".repeat(area.width as usize), style);
        buf.set_string(layout.name_x, y, clip(&label, layout.name_width), style);
        let vol_style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).bold()
        } else if in_track_selection {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(170, 145, 70))
                .bold()
        } else {
            Style::default().fg(Color::Cyan)
        };
        buf.set_string(
            layout.volume_x,
            y,
            clip(&vol_bar, layout.volume_width),
            vol_style,
        );
        buf.set_string(layout.db_x, y, clip(&db_label, 5), style);
        buf.set_string(layout.arm_x, y, arm_sym, arm_style);
        let x = layout.arm_x + UnicodeWidthStr::width(arm_sym) as u16;
        // Fill remaining with spaces in label style
        let remaining = (area.x + area.width).saturating_sub(x) as usize;
        if remaining > 0 {
            buf.set_string(x, y, &" ".repeat(remaining), style);
        }
    }
}

fn draw_param_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw("  ")];
    for param in StepParam::VISIBLE {
        let (prefix, hotkey, suffix) = param.tab_parts();
        let is_active = param == app.ui.active_param;
        let color = param_color(param);

        let base_style = if is_active {
            Style::default().fg(Color::Black).bg(color).bold()
        } else {
            Style::default().fg(color)
        };
        let hotkey_style = base_style.add_modifier(Modifier::UNDERLINED);

        spans.push(Span::styled(" ", base_style));
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix, base_style));
        }
        spans.push(Span::styled(hotkey, hotkey_style));
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, base_style));
        }
        spans.push(Span::styled(" ", base_style));
        spans.push(Span::raw(" "));
    }
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

/// Whether a step is in the "odd" beat group (steps 4-7, 12-15, ...).
fn is_beat_group_odd(step: usize) -> bool {
    (step / 4) % 2 == 1
}

/// Pulsing background for the cursor column (slow sine wave).
fn cursor_pulse_bg() -> Color {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let t = (ms as f64) / 700.0 * std::f64::consts::TAU;
    let b = (55.0 + 25.0 * (0.5 + 0.5 * t.sin())) as u8;
    Color::Rgb(b, b, b)
}

/// Pulsing foreground for the blinking cursor border.
fn cursor_pulse_fg() -> Color {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let t = (ms as f64) / 700.0 * std::f64::consts::TAU;
    let b = (110.0 + 110.0 * (0.5 + 0.5 * t.sin())) as u8;
    Color::Rgb(b, b, b)
}

/// Background color for a step column. Alternates every 4 steps for beat grouping.
fn step_bg(app: &App, step: usize, is_playing: bool, playhead: usize) -> Color {
    let is_cursor = step == app.ui.cursor_step;
    let is_sel = app.has_selection() && is_in_selection(app, step);
    let is_ph = is_playing && step == playhead;

    if is_cursor {
        cursor_pulse_bg()
    } else if is_sel {
        Color::Rgb(20, 30, 60) // blue tint
    } else if is_ph {
        Color::Rgb(55, 42, 8) // amber tint
    } else if is_beat_group_odd(step) {
        Color::Rgb(22, 22, 22)
    } else {
        Color::Reset
    }
}

/// Dim foreground color for inactive elements, alternating by beat group.
fn step_dim_fg(step: usize) -> Color {
    if is_beat_group_odd(step) {
        Color::Rgb(70, 70, 70)
    } else {
        Color::Rgb(45, 45, 45)
    }
}

/// Step number color, alternating by beat group.
fn step_num_fg(step: usize) -> Color {
    if is_beat_group_odd(step) {
        Color::Rgb(110, 110, 110)
    } else {
        Color::Rgb(70, 70, 70)
    }
}

/// Active (filled) foreground color, alternating by beat group.
/// Full white vs ~80% brightness white.
fn step_active_fg(step: usize) -> Color {
    if is_beat_group_odd(step) {
        Color::Rgb(200, 200, 200)
    } else {
        Color::Rgb(255, 255, 255)
    }
}

fn draw_bars(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        return;
    }

    // Branch to effect slot bars when effects column is focused
    if app.ui.focused_region == Region::Params && app.ui.params_column == 1 {
        draw_slot_bars(frame, app, area);
        return;
    }

    let ns = app.num_steps();
    let track_ph = app.state.track_step(app.ui.cursor_track);
    let playhead = track_ph % ns;
    let is_playing = app.state.is_playing();
    let sd = &app.state.pattern.step_data[app.ui.cursor_track];
    let is_transpose = app.ui.active_param == StepParam::Transpose;
    let is_sync = app.ui.active_param == StepParam::Sync;

    let (page_start, page_end) = app.page_range();

    let x_offset = 2u16;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let raw = sd.get(step, app.ui.active_param);
        let normalized = app.ui.active_param.normalize(raw);
        let active = app.state.pattern.patterns[app.ui.cursor_track].is_active(step);
        let playhead_on_page = playhead >= page_start && playhead < page_end;
        let bg = step_bg(app, step, is_playing && playhead_on_page, playhead);

        // Sync param: show label text vertically centered instead of bars
        if is_sync {
            let sync_idx = raw.round() as usize;
            let label = if sync_idx < crate::sequencer::SYNC_COUNT {
                crate::sequencer::SYNC_RESOLUTIONS[sync_idx].1
            } else {
                ""
            };
            let fg = if sync_idx == 0 || !active {
                step_dim_fg(step)
            } else {
                step_active_fg(step)
            };
            // Render label bytes vertically, centered in BAR_HEIGHT (labels are ASCII)
            let label_bytes = label.as_bytes();
            let label_len = label_bytes.len();
            let start_row = if label_len < BAR_HEIGHT {
                (BAR_HEIGHT - label_len) / 2
            } else {
                0
            };
            for row in 0..BAR_HEIGHT {
                let cell_y = area.y + row as u16;
                let ci = row.wrapping_sub(start_row);
                let ch = if ci < label_len {
                    format!(" {} ", label_bytes[ci] as char)
                } else {
                    "   ".to_string()
                };
                let style = Style::default().fg(fg).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(ch).style(style), cell_area);
            }
            continue;
        }

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;

            if is_transpose {
                let center = BAR_HEIGHT / 2;
                let half_levels = if normalized >= 0.5 {
                    ((normalized - 0.5) * 2.0 * center as f32 * 2.0).round() as usize
                } else {
                    ((0.5 - normalized) * 2.0 * center as f32 * 2.0).round() as usize
                };
                let going_up = normalized >= 0.5;

                let dim = step_dim_fg(step);
                let active_fg = param_color(app.ui.active_param);
                let (cell_text, fg_override) = if going_up {
                    if row < center {
                        let dist_from_center = center - row;
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (
                                " \u{2588} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else if half_levels >= threshold + 1 {
                            (
                                " \u{2584} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else {
                            ("   ".to_string(), dim)
                        }
                    } else if row == center {
                        ("\u{2500}\u{2500}\u{2500}".to_string(), dim)
                    } else {
                        ("   ".to_string(), dim)
                    }
                } else {
                    if row > center {
                        let dist_from_center = row - center;
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (
                                " \u{2588} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else if half_levels >= threshold + 1 {
                            (
                                " \u{2580} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else {
                            ("   ".to_string(), dim)
                        }
                    } else if row == center {
                        ("\u{2500}\u{2500}\u{2500}".to_string(), dim)
                    } else {
                        ("   ".to_string(), dim)
                    }
                };

                let style = Style::default().fg(fg_override).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            } else {
                let rows_from_bottom = BAR_HEIGHT - 1 - row;
                let threshold = rows_from_bottom * 2;
                let level = if fill_levels >= threshold + 2 {
                    2
                } else if fill_levels >= threshold + 1 {
                    1
                } else {
                    0
                };

                let ch = match level {
                    2 => "\u{2588}",
                    1 => "\u{2584}",
                    _ => " ",
                };

                let cell_text = if ch == " " {
                    "   ".to_string()
                } else {
                    format!(" {} ", ch)
                };

                let fg = if active {
                    param_color(app.ui.active_param)
                } else {
                    step_dim_fg(step)
                };
                let style = Style::default().fg(fg).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            }
        }
    }

    // Blinking cursor border overlay — drawn on top of bars
    let cursor_step = app.ui.cursor_step;
    if cursor_step >= page_start && cursor_step < page_end {
        let col_x = area.x + x_offset + (cursor_step - page_start) as u16 * COL_WIDTH;
        let fg = cursor_pulse_fg();
        let bg = cursor_pulse_bg();
        let border_style = Style::default().fg(fg).bg(bg);
        // Side rails keep the cursor visible without stealing height from the bar fill.
        for mid in 0..BAR_HEIGHT as u16 {
            let y = area.y + mid;
            frame.render_widget(
                Paragraph::new("\u{2502}").style(border_style),
                Rect::new(col_x, y, 1, 1),
            );
            frame.render_widget(
                Paragraph::new("\u{2502}").style(border_style),
                Rect::new(col_x + 2, y, 1, 1),
            );
        }
    }
}

/// Unified bar drawing for any effect slot.
fn draw_slot_bars(frame: &mut Frame, app: &App, area: Rect) {
    let track = app.ui.cursor_track;
    let Some(slot_idx) = app.selected_effect_slot() else {
        return;
    };
    let param_idx = app.ui.effect_param_cursor;

    let desc = match app
        .graph
        .effect_descriptors
        .get(track)
        .and_then(|d| d.get(slot_idx))
    {
        Some(d) => d,
        None => return,
    };
    if param_idx >= desc.params.len() {
        return;
    }
    let param_desc = &desc.params[param_idx];

    // Skip boolean params
    if param_desc.is_boolean() {
        return;
    }

    let chain = &app.state.pattern.effect_chains[track];
    if slot_idx >= chain.len() {
        return;
    }
    let slot = &chain[slot_idx];

    let ns = app.num_steps();
    let track_ph = app.state.track_step(track);
    let playhead = track_ph % ns;
    let is_playing = app.state.is_playing();
    let (page_start, page_end) = app.page_range();
    let x_offset = 2u16;

    let default_color = Color::White;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let plock_val = slot.plocks.get(step, param_idx);
        let value = plock_val.unwrap_or_else(|| slot.defaults.get(param_idx));
        let normalized = param_desc.normalize(value);

        let active = app.state.pattern.patterns[track].is_active(step);
        let playhead_on_page = playhead >= page_start && playhead < page_end;
        let bg = step_bg(app, step, is_playing && playhead_on_page, playhead);

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;
            let rows_from_bottom = BAR_HEIGHT - 1 - row;
            let threshold = rows_from_bottom * 2;
            let level = if fill_levels >= threshold + 2 {
                2
            } else if fill_levels >= threshold + 1 {
                1
            } else {
                0
            };

            let ch = match level {
                2 => "\u{2588}",
                1 => "\u{2584}",
                _ => " ",
            };

            let cell_text = if ch == " " {
                "   ".to_string()
            } else {
                format!(" {} ", ch)
            };

            let fg = if !active {
                step_dim_fg(step)
            } else if plock_val.is_some() {
                Color::White
            } else {
                default_color
            };
            let style = Style::default().fg(fg).bg(bg);
            let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
            frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
        }
    }
}

fn draw_trigger_row(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() || area.height < 2 {
        return;
    }

    let ns = app.num_steps();
    let track_ph = app.state.track_step(app.ui.cursor_track);
    let playhead = track_ph % ns;
    let is_playing = app.state.is_playing();
    let (page_start, page_end) = app.page_range();
    let x_offset = 2u16;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let active = app.state.pattern.patterns[app.ui.cursor_track].is_active(step);
        let chord_count = app.state.pattern.chord_data[app.ui.cursor_track].count(step);
        // Check all slots for p-locks
        let has_plock = app.state.pattern.effect_chains[app.ui.cursor_track]
            .iter()
            .any(|slot| {
                let np = slot.num_params.load(Ordering::Relaxed) as usize;
                slot.plocks.step_has_any_plock(step, np)
            });
        // Symbol: circle base; append chord count digit when > 1
        let base_sym = if has_plock { "\u{25c9}" } else { "\u{25cf}" }; // ◉ or ●
        let ch: String = if !active {
            " \u{00b7} ".to_string() // ·
        } else if chord_count > 1 {
            format!(" {}{}", base_sym, chord_count.min(9))
        } else {
            format!(" {} ", base_sym)
        };
        let fg = if active && has_plock {
            Color::Cyan
        } else if active && chord_count > 1 {
            Color::Rgb(220, 160, 40) // amber for chords
        } else if active {
            step_active_fg(step)
        } else {
            step_dim_fg(step)
        };
        let bg = step_bg(app, step, is_playing, playhead);
        let style = Style::default().fg(fg).bg(bg);
        let cell = Rect::new(col_x, area.y, COL_WIDTH, 1);
        frame.render_widget(Paragraph::new(ch).style(style), cell);
    }

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let num = format!("{:>2} ", step + 1);
        let is_sel = app.has_selection() && is_in_selection(app, step);
        let beat_bg = if is_beat_group_odd(step) {
            Color::Rgb(22, 22, 22)
        } else {
            Color::Reset
        };
        let style = if step == app.ui.cursor_step {
            Style::default().fg(Color::White).bg(cursor_pulse_bg())
        } else if is_sel {
            Style::default()
                .fg(Color::Rgb(160, 160, 160))
                .bg(Color::Rgb(20, 30, 60))
        } else {
            Style::default().fg(step_num_fg(step)).bg(beat_bg)
        };
        let cell = Rect::new(col_x, area.y + 1, COL_WIDTH, 1);
        frame.render_widget(Paragraph::new(num).style(style), cell);
    }
}

fn draw_page_blocks(frame: &mut Frame, app: &mut App, area: Rect) {
    let ns = app.num_steps();
    let total_pages = ns.div_ceil(STEPS_PER_PAGE);
    if total_pages <= 1 || area.height < 2 {
        app.ui.page_btn_layout.clear();
        return;
    }

    let display_page = app.display_page();
    let playing_page = (app.state.track_step(app.ui.cursor_track) % ns) / STEPS_PER_PAGE;

    let mut btn_layout: Vec<(u16, u16, usize)> = Vec::new();
    let mut x = area.x + 2;

    // Row 1: page number buttons
    for p in 0..total_pages {
        let label = format!(" {} ", p + 1);
        let w = label.len().max(3) as u16;

        let style = if p == display_page {
            Style::default().fg(Color::Black).bg(Color::White).bold()
        } else {
            Style::default().fg(Color::Gray).bg(Color::Rgb(50, 50, 50))
        };

        if x + w <= area.x + area.width {
            frame.render_widget(
                Paragraph::new(Span::styled(&label, style)),
                Rect::new(x, area.y, w, 1),
            );
            btn_layout.push((x, x + w, p));
        }
        x += w;
    }

    // Row 2: playhead dot centered under the playing page
    for &(bx, bw, page) in &btn_layout {
        if page == playing_page {
            let block_w = bw - bx;
            let dot_x = bx + block_w / 2;
            frame.render_widget(
                Paragraph::new(Span::styled("\u{2022}", Style::default().fg(Color::White))),
                Rect::new(dot_x, area.y + 1, 1, 1),
            );
            break;
        }
    }

    app.ui.page_btn_layout = btn_layout;
}

fn draw_value_line(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        return;
    }

    let is_pattern_select = app.ui.input_mode == InputMode::PatternSelect;
    let is_bpm_entry = app.ui.input_mode == InputMode::ValueEntry && app.ui.bpm_entry;
    let is_cirklon_entry = app.ui.input_mode == InputMode::ValueEntry
        && !app.ui.bpm_entry
        && app.ui.focused_region == Region::Cirklon;

    let line = if is_bpm_entry {
        Line::from(vec![
            Span::styled("  BPM: ", Style::default().fg(Color::White)),
            Span::styled(
                format!("{}\u{2588}", app.ui.value_buffer),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(60, 60, 60))
                    .bold(),
            ),
            Span::styled(
                "  Enter: set  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if is_pattern_select {
        if app.ui.pattern_clone_pending {
            Line::from(vec![
                Span::styled(
                    "  Clone pattern \u{2192} new  ",
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    "Enter: confirm  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("  Pattern: ", Style::default().fg(Color::White)),
                Span::styled(
                    format!("{}\u{2588}", app.ui.value_buffer),
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(60, 60, 60))
                        .bold(),
                ),
                Span::styled(
                    "  Enter: go  c: clone  x: delete  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        }
    } else if is_cirklon_entry {
        let step_label = if app.has_selection() {
            let (lo, hi) = app.selected_range();
            format!("Steps {}-{}", lo + 1, hi + 1)
        } else {
            format!("Step {}", app.ui.cursor_step + 1)
        };
        Line::from(vec![
            Span::styled(
                format!("  {}: {} = ", step_label, app.ui.active_param.label()),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{}\u{2588}", app.ui.value_buffer),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(60, 60, 60))
                    .bold(),
            ),
            Span::styled(
                "  Enter: set  Esc: cancel  -: negate",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if app.has_selection() {
        let sel_text = if !app.ui.visual_steps.is_empty() {
            let count = app.ui.visual_steps.len();
            format!("  {} steps selected", count)
        } else {
            let (lo, hi) = app.selected_range();
            let count = hi - lo + 1;
            format!("  Steps {}-{} selected ({} steps)", lo + 1, hi + 1, count)
        };
        Line::from(Span::styled(
            format!(
                "{}  {} = \u{2191}\u{2193}",
                sel_text,
                app.ui.active_param.label(),
            ),
            Style::default().fg(Color::Rgb(160, 160, 160)),
        ))
    } else {
        let sd = &app.state.pattern.step_data[app.ui.cursor_track];
        let val = sd.get(app.ui.cursor_step, app.ui.active_param);
        Line::from(Span::styled(
            format!(
                "  Step {}: {} = {}",
                app.ui.cursor_step + 1,
                app.ui.active_param.label(),
                app.ui.active_param.format_value(val),
            ),
            Style::default().fg(Color::White),
        ))
    };

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_piano_roll(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.tracks.is_empty() || area.height < 2 {
        return;
    }

    // Store area for click detection
    app.ui.layout.piano_area = area;

    let track = app.ui.cursor_track;
    let now = Instant::now();
    let is_playing = app.state.is_playing();

    // Reset note state on track change
    if track != app.ui.piano_last_track {
        app.ui.piano_notes.clear();
        app.ui.piano_last_step = usize::MAX;
        app.ui.piano_last_track = track;
    }

    // Collect cursor step notes (for paused mode display and range calculation)
    let cursor_step = app.ui.cursor_step;
    let step_active = app.state.pattern.patterns[track].is_active(cursor_step);
    let chord_count = app.state.pattern.chord_data[track].count(cursor_step);
    let step_notes: Vec<i32> = if chord_count > 0 {
        (0..chord_count)
            .map(|n| {
                app.state.pattern.chord_data[track]
                    .get(cursor_step, n)
                    .round() as i32
            })
            .collect()
    } else if step_active {
        let t = app.state.pattern.step_data[track]
            .get(cursor_step, StepParam::Transpose)
            .round() as i32;
        vec![t]
    } else {
        vec![]
    };

    // Detect new step triggers (playing mode) and add notes with duration-based expiry
    if is_playing {
        let ns = app.num_steps();
        let step = app.state.track_step(track) % ns;

        if step != app.ui.piano_last_step {
            app.ui.piano_last_step = step;

            if app.state.pattern.patterns[track].is_active(step) {
                let bpm = app.state.transport.bpm.load(Ordering::Relaxed) as f64;
                let secs_per_step = 60.0 / bpm / 4.0;
                let dur = app.state.pattern.step_data[track].get(step, StepParam::Duration) as f64;
                let release_ms = app.state.pattern.track_params[track].get_release_ms() as f64;
                let total_secs = dur * secs_per_step + release_ms / 1000.0;
                let expires = now + Duration::from_secs_f64(total_secs);

                let cc = app.state.pattern.chord_data[track].count(step);
                if cc > 0 {
                    for n in 0..cc {
                        let t = app.state.pattern.chord_data[track].get(step, n).round() as i32;
                        app.ui.piano_notes.push((t, expires));
                    }
                } else {
                    let t = app.state.pattern.step_data[track]
                        .get(step, StepParam::Transpose)
                        .round() as i32;
                    app.ui.piano_notes.push((t, expires));
                }
            }
        }
    } else {
        app.ui.piano_last_step = usize::MAX;
    }

    app.ui.piano_notes.retain(|(_, exp)| *exp > now);

    // Key range: same in both playing and paused modes so highlighted keys always
    // appear at the same positions (avoids apparent octave shift between modes).
    let octaves = ((area.width as i32) / 12).min(5).max(2);
    let num_keys = octaves * 12;
    let half = num_keys / 2;
    let lo = -((half / 12) * 12);
    let hi = lo + num_keys;
    app.ui.piano_lo = lo;

    let x0 = area.x;
    let buf = frame.buffer_mut();

    let white_bg = Color::Rgb(200, 200, 200);
    let black_bg = Color::Rgb(30, 30, 30);
    let step_note_bg = Color::Rgb(220, 160, 40); // amber: cursor step notes
    let ringing_bg = Color::Cyan; // cyan: currently playing notes
    let both_bg = Color::Rgb(100, 220, 140); // green: step note that is also ringing
    let gap_bg = Color::Rgb(50, 50, 50);

    // Build note sets
    let ringing: Vec<i32> = app.ui.piano_notes.iter().map(|(s, _)| *s).collect();
    let held: Vec<i32> = app
        .ui
        .held_notes
        .iter()
        .map(|note| note.transpose.round() as i32)
        .collect();

    // Always overlay both: amber = cursor step, cyan = playing, green = both
    let key_color = |s: i32, row: u8| -> Color {
        let is_black = matches!(s.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
        let is_step = step_notes.contains(&s);
        let is_ringing = ringing.contains(&s) || held.contains(&s);
        if is_step && is_ringing {
            both_bg
        } else if is_step {
            step_note_bg
        } else if is_ringing {
            ringing_bg
        } else if is_black {
            if row == 0 {
                black_bg
            } else {
                gap_bg
            }
        } else {
            white_bg
        }
    };

    // Row 1 (back): all keys visible
    let y1 = area.y;
    for s in lo..hi {
        let x = x0 + (s - lo) as u16;
        if x >= area.x + area.width {
            break;
        }
        buf.set_string(x, y1, " ", Style::default().bg(key_color(s, 0)));
    }

    // Row 2 (front): white keys extend, black positions become gaps
    if area.height >= 2 {
        let y2 = area.y + 1;
        for s in lo..hi {
            let x = x0 + (s - lo) as u16;
            if x >= area.x + area.width {
                break;
            }
            buf.set_string(x, y2, " ", Style::default().bg(key_color(s, 1)));
        }
    }

    // Row 3: Octave labels
    if area.height >= 3 {
        let y3 = area.y + 2;
        for s in lo..hi {
            if s.rem_euclid(12) == 0 {
                let x = x0 + (s - lo) as u16;
                if x + 2 <= area.x + area.width {
                    let oct = s.div_euclid(12) + 4;
                    buf.set_string(
                        x,
                        y3,
                        &format!("C{}", oct),
                        Style::default().fg(Color::Rgb(70, 70, 70)),
                    );
                }
            }
        }
    }
}
