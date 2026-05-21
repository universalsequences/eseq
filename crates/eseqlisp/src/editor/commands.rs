use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Editor;

impl Editor {
    pub(super) fn bind_defaults(&mut self) {
        let binds: &[(KeyCode, KeyModifiers, &str)] = &[
            (KeyCode::Char('q'), KeyModifiers::CONTROL, "quit"),
            (KeyCode::Char('s'), KeyModifiers::CONTROL, "save-buffer"),
            (KeyCode::Char(' '), KeyModifiers::CONTROL, "set-mark"),
            (KeyCode::Char('a'), KeyModifiers::CONTROL, "move-line-start"),
            (KeyCode::Char('e'), KeyModifiers::CONTROL, "move-line-end"),
            (
                KeyCode::Char('f'),
                KeyModifiers::CONTROL,
                "move-page-forward",
            ),
            (
                KeyCode::Char('b'),
                KeyModifiers::CONTROL,
                "move-page-backward",
            ),
            (KeyCode::Char('l'), KeyModifiers::CONTROL, "recenter-cursor"),
            (
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
                "kill-region-or-word",
            ),
            (KeyCode::Char('w'), KeyModifiers::ALT, "copy-region"),
            (KeyCode::Char('y'), KeyModifiers::CONTROL, "yank"),
            (
                KeyCode::Char('c'),
                KeyModifiers::SUPER,
                "copy-selection-to-clipboard",
            ),
            (
                KeyCode::Char('v'),
                KeyModifiers::SUPER,
                "paste-from-clipboard",
            ),
            (
                KeyCode::Char('k'),
                KeyModifiers::CONTROL,
                "delete-to-line-end",
            ),
            (KeyCode::Tab, KeyModifiers::NONE, "complete"),
            (KeyCode::Left, KeyModifiers::CONTROL, "move-word-left"),
            (KeyCode::Right, KeyModifiers::CONTROL, "move-word-right"),
            (KeyCode::Left, KeyModifiers::ALT, "move-word-left"),
            (KeyCode::Right, KeyModifiers::ALT, "move-word-right"),
            (KeyCode::Char('b'), KeyModifiers::ALT, "move-word-left"),
            (KeyCode::Char('f'), KeyModifiers::ALT, "move-word-right"),
            (KeyCode::Left, KeyModifiers::NONE, "move-left"),
            (KeyCode::Right, KeyModifiers::NONE, "move-right"),
            (KeyCode::Up, KeyModifiers::NONE, "move-up"),
            (KeyCode::Down, KeyModifiers::NONE, "move-down"),
            (KeyCode::Backspace, KeyModifiers::NONE, "delete-char-before"),
            (
                KeyCode::Char('x'),
                KeyModifiers::ALT,
                "execute-extended-command",
            ),
            (KeyCode::Char('s'), KeyModifiers::CONTROL, "search-forward"),
            (KeyCode::Char('r'), KeyModifiers::CONTROL, "search-backward"),
            (KeyCode::Char('.'), KeyModifiers::ALT, "goto-definition"),
            (KeyCode::Char(','), KeyModifiers::ALT, "pop-definition-mark"),
        ];
        for (code, mods, cmd) in binds {
            self.builtins
                .insert(KeyEvent::new(*code, *mods), cmd.to_string());
        }

        // Tiling keybindings (C-x chords, registered as Lisp bindings)
        let tiling_binds: &[(&str, &str)] = &[
            ("C-x 2", "split-window-below"),
            ("C-x 3", "split-window-right"),
            ("C-x 0", "delete-window"),
            ("C-x 1", "delete-other-windows"),
            ("C-x o", "other-window"),
            ("C-x C-f", "find-file"),
            ("ESC .", "goto-definition"),
            ("ESC ,", "pop-definition-mark"),
        ];
        for (key, handler) in tiling_binds {
            self.default_lisp_bindings
                .insert(key.to_string(), handler.to_string());
        }
    }

    pub(super) fn run_command(&mut self, cmd: &str) {
        match cmd {
            "quit" => {
                self.completion = None;
                if self.should_prompt_on_quit() {
                    self.open_save_prompt(true);
                } else {
                    self.should_quit = true;
                    self.last_exit = super::EditorExit::Closed;
                }
            }
            "set-mark" => {
                self.completion = None;
                self.minibuffer = Some("Mark set".to_string());
                self.mark = Some(super::Mark {
                    buffer_id: self.active_buffer().id,
                    cursor: self.active_buffer().cursor,
                });
            }
            "move-left" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_left();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-right" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_right();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-up" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_up();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-down" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_down();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-buffer-end" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_to_buffer_end();
                self.sync_text_horizontal_scroll_to_viewport();
            }

            "move-line-start" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_to_line_start();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-line-end" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_to_line_end();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-page-forward" => {
                self.completion = None;
                self.minibuffer = None;
                self.move_page_forward();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-page-backward" => {
                self.completion = None;
                self.minibuffer = None;
                self.move_page_backward();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "recenter-cursor" => {
                self.completion = None;
                self.minibuffer = None;
                self.recenter_cursor();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-word-left" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_word_left();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "move-word-right" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_word_right();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "delete-char-before" => {
                if self.guard_read_only() {
                    return;
                }
                self.minibuffer = None;
                self.record_undo_snapshot();
                if !self.delete_active_region() {
                    self.clear_mark();
                    self.active_buffer_mut().delete_char_before();
                }
                self.sync_text_horizontal_scroll_to_viewport();
                self.refresh_completion();
            }
            "kill-region-or-word" => {
                if self.guard_read_only() {
                    return;
                }
                self.minibuffer = None;
                self.record_undo_snapshot();
                if !self.kill_active_region() {
                    self.active_buffer_mut().delete_word_before();
                }
                self.sync_text_horizontal_scroll_to_viewport();
                self.refresh_completion();
            }
            "copy-region" => {
                self.completion = None;
                self.minibuffer = None;
                if self.copy_active_region() {
                    self.clear_mark();
                } else {
                    self.minibuffer = Some("No active region".to_string());
                }
            }
            "yank" => {
                if self.guard_read_only() {
                    return;
                }
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                if let Some(text) = self.kill_ring.last().cloned() {
                    self.record_undo_snapshot();
                    self.active_buffer_mut().insert_str(&text);
                    self.sync_text_horizontal_scroll_to_viewport();
                } else {
                    self.minibuffer = Some("Kill ring is empty".to_string());
                }
            }
            "copy-selection-to-clipboard" => {
                self.completion = None;
                if !self.copy_active_region_to_clipboard() {
                    self.minibuffer = Some("No active region".to_string());
                }
            }
            "paste-from-clipboard" => {
                if self.guard_read_only() {
                    return;
                }
                self.completion = None;
                self.minibuffer = None;
                self.record_undo_snapshot();
                self.paste_from_system_clipboard();
            }
            "delete-to-line-end" => {
                if self.guard_read_only() {
                    return;
                }
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                self.record_undo_snapshot();
                self.active_buffer_mut().delete_to_line_end();
                self.sync_text_horizontal_scroll_to_viewport();
            }
            "save-buffer" => {
                self.completion = None;
                if self.needs_save_as_prompt() {
                    self.open_save_prompt(false);
                } else {
                    match self.save_active_buffer() {
                        Ok(path) => self.minibuffer = Some(format!("Saved {}", path.display())),
                        Err(error) => self.minibuffer = Some(format!("Error: {error:?}")),
                    }
                }
            }
            "complete" => {
                self.minibuffer = None;
                self.refresh_completion();
                if self.completion.is_none() {
                    self.record_undo_snapshot();
                    self.active_buffer_mut().indent_current_line();
                    self.sync_runtime_context();
                }
            }
            "execute-extended-command" => {
                self.completion = None;
                self.minibuffer = None;
                let candidates = self.collect_mx_candidates();
                self.minibuffer_input = Some(super::MinibufferMode::Mx {
                    input: String::new(),
                    candidates,
                    selected: 0,
                });
            }
            "switch-to-buffer" => {
                self.completion = None;
                self.minibuffer = None;
                let candidates = self.buffer_names_by_recency();
                self.minibuffer_input = Some(super::MinibufferMode::SwitchBuffer {
                    input: String::new(),
                    candidates,
                    selected: 0,
                });
            }
            "find-file" => {
                self.completion = None;
                self.minibuffer = None;
                self.minibuffer_input = Some(super::MinibufferMode::FindFile {
                    input: String::new(),
                    selected: 0,
                });
            }
            "search-forward" => {
                self.start_search(super::SearchDirection::Forward);
            }
            "search-backward" => {
                self.start_search(super::SearchDirection::Backward);
            }
            "goto-definition" => {
                self.goto_definition();
            }
            "pop-definition-mark" => {
                self.pop_definition_mark();
            }
            _ => {}
        }
        self.sync_runtime_context();
    }
}

pub(super) fn key_str(key: KeyEvent) -> String {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let prefix = match (ctrl, alt) {
        (true, true) => "C-M-",
        (true, false) => "C-",
        (false, true) => "M-",
        (false, false) => "",
    };
    match key.code {
        KeyCode::Char(c) => format!("{prefix}{}", c.to_ascii_lowercase()),
        KeyCode::Enter => format!("{prefix}RET"),
        KeyCode::Backspace => format!("{prefix}BS"),
        KeyCode::Esc => "ESC".to_string(),
        KeyCode::Up => "UP".to_string(),
        KeyCode::Down => "DOWN".to_string(),
        KeyCode::Left => "LEFT".to_string(),
        KeyCode::Right => "RIGHT".to_string(),
        _ => format!("{:?}", key.code),
    }
}
