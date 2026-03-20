use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Editor, MinibufferMode, filter_candidates};

impl Editor {
    pub(super) fn handle_minibuffer_key(&mut self, key: KeyEvent) -> bool {
        let Some(mode) = self.minibuffer_input.take() else {
            return false;
        };

        match mode {
            MinibufferMode::Mx {
                mut input,
                candidates,
                mut selected,
            } => match key.code {
                KeyCode::Esc => {
                    self.minibuffer = None;
                }
                KeyCode::Enter => {
                    let filtered = filter_candidates(&candidates, &input);
                    let name = if let Some(sel) = filtered.get(selected) {
                        sel.clone()
                    } else {
                        input.clone()
                    };
                    if !name.is_empty() {
                        self.execute_mx_command(&name);
                    }
                    return true;
                }
                KeyCode::Tab => {
                    let filtered = filter_candidates(&candidates, &input);
                    if !filtered.is_empty() {
                        selected = (selected + 1) % filtered.len();
                    }
                    self.minibuffer_input = Some(MinibufferMode::Mx {
                        input,
                        candidates,
                        selected,
                    });
                }
                KeyCode::Backspace => {
                    input.pop();
                    selected = 0;
                    self.minibuffer_input = Some(MinibufferMode::Mx {
                        input,
                        candidates,
                        selected,
                    });
                }
                KeyCode::Char(c)
                    if key.modifiers == KeyModifiers::NONE
                        || key.modifiers == KeyModifiers::SHIFT =>
                {
                    input.push(c);
                    selected = 0;
                    self.minibuffer_input = Some(MinibufferMode::Mx {
                        input,
                        candidates,
                        selected,
                    });
                }
                _ => {
                    self.minibuffer_input = Some(MinibufferMode::Mx {
                        input,
                        candidates,
                        selected,
                    });
                }
            },
            MinibufferMode::SwitchBuffer {
                mut input,
                candidates,
                mut selected,
            } => match key.code {
                KeyCode::Esc => {
                    self.minibuffer = None;
                }
                KeyCode::Enter => {
                    let filtered = filter_candidates(&candidates, &input);
                    let name = if let Some(sel) = filtered.get(selected) {
                        sel.clone()
                    } else {
                        input.clone()
                    };
                    if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
                        self.active_leaf_mut().buffer_idx = idx;
                        self.mark_needs_redraw();
                        self.sync_runtime_context();
                        self.completion = None;
                        self.clear_mark();
                        self.minibuffer = Some(format!("Switched to {name}"));
                    } else {
                        self.minibuffer = Some(format!("No buffer named '{name}'"));
                    }
                    return true;
                }
                KeyCode::Tab => {
                    let filtered = filter_candidates(&candidates, &input);
                    if !filtered.is_empty() {
                        selected = (selected + 1) % filtered.len();
                    }
                    self.minibuffer_input = Some(MinibufferMode::SwitchBuffer {
                        input,
                        candidates,
                        selected,
                    });
                }
                KeyCode::Backspace => {
                    input.pop();
                    selected = 0;
                    self.minibuffer_input = Some(MinibufferMode::SwitchBuffer {
                        input,
                        candidates,
                        selected,
                    });
                }
                KeyCode::Char(c)
                    if key.modifiers == KeyModifiers::NONE
                        || key.modifiers == KeyModifiers::SHIFT =>
                {
                    input.push(c);
                    selected = 0;
                    self.minibuffer_input = Some(MinibufferMode::SwitchBuffer {
                        input,
                        candidates,
                        selected,
                    });
                }
                _ => {
                    self.minibuffer_input = Some(MinibufferMode::SwitchBuffer {
                        input,
                        candidates,
                        selected,
                    });
                }
            },
        }
        self.mark_needs_redraw();
        true
    }

    pub(super) fn execute_mx_command(&mut self, name: &str) {
        // Check if it's a command that opens its own minibuffer
        if name == "switch-to-buffer" {
            self.run_command("switch-to-buffer");
            return;
        }
        // First try as a builtin command name
        let builtin_names: Vec<String> = self.builtins.values().cloned().collect();
        if builtin_names.contains(&name.to_string()) {
            self.run_command(name);
            return;
        }
        // Then try as a Lisp function
        self.call_lisp_handler(name);
    }

    pub(super) fn collect_mx_candidates(&mut self) -> Vec<String> {
        let mut names: Vec<String> = self.builtins.values().cloned().collect();
        let symbols = self.runtime.completion_symbols();
        names.extend(symbols);
        // Also include lisp binding handler names
        names.extend(self.lisp_bindings.values().cloned());
        names.sort();
        names.dedup();
        names
    }

    pub fn minibuffer_prompt(&self) -> Option<String> {
        match &self.minibuffer_input {
            Some(MinibufferMode::Mx {
                input,
                candidates,
                selected,
            }) => {
                let filtered = filter_candidates(candidates, input);
                let hint = filtered.get(*selected).map(|s| s.as_str()).unwrap_or("");
                if hint.is_empty() || input.is_empty() {
                    Some(format!("M-x {input}"))
                } else {
                    Some(format!("M-x {input}  [{hint}]"))
                }
            }
            Some(MinibufferMode::SwitchBuffer {
                input,
                candidates,
                selected,
            }) => {
                let filtered = filter_candidates(candidates, input);
                let hint = filtered.get(*selected).map(|s| s.as_str()).unwrap_or("");
                if hint.is_empty() || input.is_empty() {
                    Some(format!("Switch to buffer: {input}"))
                } else {
                    Some(format!("Switch to buffer: {input}  [{hint}]"))
                }
            }
            None => None,
        }
    }
}
