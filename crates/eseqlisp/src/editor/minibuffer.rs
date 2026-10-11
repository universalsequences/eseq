use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Editor, MinibufferMode, SearchDirection, filter_candidates};

impl Editor {
    pub(super) fn handle_minibuffer_key(&mut self, key: KeyEvent) -> bool {
        let Some(mode) = self.minibuffer_input.take() else {
            return false;
        };

        match mode {
            MinibufferMode::CommandChoices { title, mut input, entries, mut selected } => {
                let labels: Vec<String> = entries.iter().map(|(label, _)| label.clone()).collect();
                let filtered = filter_candidates(&labels, &input);
                match key.code {
                    KeyCode::Esc => { self.minibuffer = None; return true; }
                    KeyCode::Enter => {
                        if let Some(label) = filtered.get(selected) {
                            if let Some((_, command)) = entries.iter().find(|(name, _)| name == label) {
                                self.runtime.enqueue_host_command(command.clone());
                            }
                        }
                        return true;
                    }
                    KeyCode::Tab | KeyCode::Down => { if !filtered.is_empty() { selected = (selected + 1) % filtered.len(); } }
                    KeyCode::Up => { if !filtered.is_empty() { selected = (selected + filtered.len() - 1) % filtered.len(); } }
                    KeyCode::Backspace => { input.pop(); selected = 0; }
                    KeyCode::Char(c) if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT => { input.push(c); selected = 0; }
                    _ => {}
                }
                self.minibuffer_input = Some(MinibufferMode::CommandChoices { title, input, entries, selected });
            }

            MinibufferMode::Palette(state) => self.handle_palette_key(key, state),
            MinibufferMode::Mx { mut input, candidates, mut selected } => {
                match step_completing_read(key, &mut input, &candidates, &mut selected) {
                    CompletingRead::Cancel => self.minibuffer = None,
                    CompletingRead::Accept(name) => {
                        if !name.is_empty() {
                            self.execute_mx_command(&name);
                        }
                        return true;
                    }
                    CompletingRead::Pending => {
                        self.minibuffer_input = Some(MinibufferMode::Mx { input, candidates, selected });
                    }
                }
            }
            MinibufferMode::DescribeFunction { mut input, candidates, mut selected } => {
                match step_completing_read(key, &mut input, &candidates, &mut selected) {
                    CompletingRead::Cancel => self.minibuffer = None,
                    CompletingRead::Accept(name) => {
                        if !name.is_empty() {
                            self.describe_function(&name);
                        }
                        return true;
                    }
                    CompletingRead::Pending => {
                        self.minibuffer_input =
                            Some(MinibufferMode::DescribeFunction { input, candidates, selected });
                    }
                }
            }
            MinibufferMode::SwitchBuffer { mut input, candidates, mut selected } => {
                match step_completing_read(key, &mut input, &candidates, &mut selected) {
                    CompletingRead::Cancel => self.minibuffer = None,
                    CompletingRead::Accept(name) => {
                        if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
                            self.active_leaf_mut().buffer_idx = idx;
                            self.record_buffer_access_by_idx(idx);
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
                    CompletingRead::Pending => {
                        self.minibuffer_input =
                            Some(MinibufferMode::SwitchBuffer { input, candidates, selected });
                    }
                }
            }
            MinibufferMode::FindFile {
                mut input,
                mut selected,
            } => match key.code {
                KeyCode::Esc => {
                    self.minibuffer = None;
                }
                KeyCode::Enter => {
                    let candidates = self.collect_find_file_candidates(&input);
                    let path_input = if candidates.iter().any(|candidate| candidate == &input) {
                        input.clone()
                    } else if let Some(sel) = candidates.get(selected) {
                        sel.clone()
                    } else {
                        input.clone()
                    };
                    if !path_input.is_empty() {
                        match self.open_or_create_file_buffer(self.resolve_file_input(&path_input))
                        {
                            Ok(_) => {
                                self.minibuffer = Some(format!("Opened {path_input}"));
                            }
                            Err(error) => {
                                self.minibuffer = Some(format!("Error: {error:?}"));
                            }
                        }
                    }
                    return true;
                }
                KeyCode::Tab => {
                    let candidates = self.collect_find_file_candidates(&input);
                    if !candidates.is_empty() {
                        selected = (selected + 1) % candidates.len();
                        if let Some(candidate) = candidates.get(selected) {
                            input = candidate.clone();
                        }
                    }
                    self.minibuffer_input = Some(MinibufferMode::FindFile { input, selected });
                }
                KeyCode::Backspace => {
                    input.pop();
                    selected = 0;
                    self.minibuffer_input = Some(MinibufferMode::FindFile { input, selected });
                }
                KeyCode::Char(c)
                    if key.modifiers == KeyModifiers::NONE
                        || key.modifiers == KeyModifiers::SHIFT =>
                {
                    input.push(c);
                    selected = 0;
                    self.minibuffer_input = Some(MinibufferMode::FindFile { input, selected });
                }
                _ => {
                    self.minibuffer_input = Some(MinibufferMode::FindFile { input, selected });
                }
            },
            MinibufferMode::Search { mut state } => match key.code {
                KeyCode::Esc => {
                    self.active_buffer_mut().cursor = state.origin;
                    self.minibuffer = None;
                    self.sync_text_horizontal_scroll_to_viewport();
                }
                KeyCode::Enter => {
                    self.minibuffer = None;
                    return true;
                }
                KeyCode::Backspace => {
                    state.input.pop();
                    self.apply_search_state(&mut state);
                    self.minibuffer_input = Some(MinibufferMode::Search { state });
                }
                KeyCode::Char('s') if key.modifiers == KeyModifiers::CONTROL => {
                    self.repeat_search(&mut state, SearchDirection::Forward);
                    self.minibuffer_input = Some(MinibufferMode::Search { state });
                }
                KeyCode::Char('r') if key.modifiers == KeyModifiers::CONTROL => {
                    self.repeat_search(&mut state, SearchDirection::Backward);
                    self.minibuffer_input = Some(MinibufferMode::Search { state });
                }
                KeyCode::Char(c)
                    if key.modifiers == KeyModifiers::NONE
                        || key.modifiers == KeyModifiers::SHIFT =>
                {
                    state.input.push(c);
                    self.apply_search_state(&mut state);
                    self.minibuffer_input = Some(MinibufferMode::Search { state });
                }
                _ => {
                    self.minibuffer = None;
                    return false;
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
        if name == "find-file" {
            self.run_command("find-file");
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

    /// Every name M-x offers: builtins, Lisp globals and bound handlers,
    /// sorted (the command palette is separate: `/`).
    pub fn collect_mx_candidates(&mut self) -> Vec<String> {
        let mut names: Vec<String> = self.builtins.values().cloned().collect();
        let symbols = self.runtime.completion_symbols();
        names.extend(symbols.iter().cloned());
        // Also include lisp binding handler names
        names.extend(self.lisp_bindings.values().cloned());
        names.sort();
        names.dedup();
        names
    }

    /// Builtins, bound handlers and callable globals that are not in
    /// `commands`, sorted: what the palette offers below the commands.
    pub(super) fn non_command_mx_names(&self, commands: &std::collections::HashSet<&str>) -> Vec<String> {
        let mut names: Vec<String> = self
            .builtins
            .values()
            .chain(self.lisp_bindings.values())
            .cloned()
            .chain(self.runtime.callable_global_names())
            .filter(|name| !commands.contains(name.as_str()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn minibuffer_prompt(&self) -> Option<String> {
        match &self.minibuffer_input {
            Some(MinibufferMode::CommandChoices { title, input, entries, selected }) => {
                let labels: Vec<String> = entries.iter().map(|(label, _)| label.clone()).collect();
                let filtered = filter_candidates(&labels, input);
                let hint = filtered.get(*selected).map(String::as_str).unwrap_or("No matching commands");
                Some(format!("{title}: {input}  [{hint}]"))
            }

            Some(MinibufferMode::Palette(state)) => Some(Self::palette_prompt(state)),
            Some(MinibufferMode::Mx { input, candidates, selected }) => {
                Some(completing_prompt("M-x", input, candidates, *selected))
            }
            Some(MinibufferMode::DescribeFunction { input, candidates, selected }) => {
                Some(completing_prompt("Describe function:", input, candidates, *selected))
            }
            Some(MinibufferMode::SwitchBuffer { input, candidates, selected }) => {
                Some(completing_prompt("Switch to buffer:", input, candidates, *selected))
            }
            Some(MinibufferMode::FindFile { input, selected }) => {
                let candidates = self.collect_find_file_candidates(input);
                let hint = candidates.get(*selected).map(|s| s.as_str()).unwrap_or("");
                if hint.is_empty() || input.is_empty() {
                    Some(format!("Find file: {input}"))
                } else {
                    Some(format!("Find file: {input}  [{hint}]"))
                }
            }
            Some(MinibufferMode::Search { state }) => {
                let prefix = match (state.direction, state.failed) {
                    (SearchDirection::Forward, false) => "I-search",
                    (SearchDirection::Backward, false) => "I-search backward",
                    (SearchDirection::Forward, true) => "Failing I-search",
                    (SearchDirection::Backward, true) => "Failing I-search backward",
                };
                Some(format!("{prefix}: {}", state.input))
            }
            None => None,
        }
    }

    fn minibuffer_default_directory(&self) -> PathBuf {
        self.active_buffer()
            .path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    fn resolve_file_input(&self, input: &str) -> PathBuf {
        if let Some(stripped) = input.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join(stripped);
            }
        }
        let path = PathBuf::from(input);
        if path.is_absolute() {
            path
        } else {
            self.minibuffer_default_directory().join(path)
        }
    }

    fn collect_find_file_candidates(&self, input: &str) -> Vec<String> {
        let (dir_input, needle) = match input.rsplit_once('/') {
            Some((dir, tail)) => (Some(format!("{dir}/")), tail.to_ascii_lowercase()),
            None => (None, input.to_ascii_lowercase()),
        };

        let search_dir = dir_input
            .as_deref()
            .map(|dir| self.resolve_file_input(dir))
            .unwrap_or_else(|| self.minibuffer_default_directory());
        let display_prefix = dir_input.unwrap_or_default();

        let Ok(entries) = std::fs::read_dir(&search_dir) else {
            return Vec::new();
        };

        let mut out = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_type = entry.file_type().ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                if !needle.is_empty() && !name.to_ascii_lowercase().contains(&needle) {
                    return None;
                }
                let suffix = if file_type.is_dir() { "/" } else { "" };
                Some(format!("{display_prefix}{name}{suffix}"))
            })
            .collect::<Vec<_>>();
        out.sort();
        out
    }
}

/// What a key did to a completing read (M-x, switch-to-buffer,
/// describe-function).
pub(super) enum CompletingRead {
    Pending,
    Cancel,
    /// The selected filtered candidate, or the raw input when none match.
    Accept(String),
}

/// What a key means to a list-picking prompt (completing reads and the
/// command palette), so they share one keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadKey {
    Cancel,
    Accept,
    /// Move the highlight by one, wrapping (±1).
    Step(isize),
    /// Move the highlight by a page, clamping (±1 = one page).
    Page(isize),
    Backspace,
    Insert(char),
    Ignore,
}

/// Esc / C-g cancel, Enter accepts, Tab / Down / C-n and BackTab / Up / C-p
/// step, PageUp / PageDown page, Backspace and plain characters edit.
pub(super) fn classify_read_key(key: KeyEvent) -> ReadKey {
    let ctrl = key.modifiers == KeyModifiers::CONTROL;
    match key.code {
        KeyCode::Esc => ReadKey::Cancel,
        KeyCode::Char('g') if ctrl => ReadKey::Cancel,
        KeyCode::Enter => ReadKey::Accept,
        KeyCode::Tab | KeyCode::Down => ReadKey::Step(1),
        KeyCode::Char('n') if ctrl => ReadKey::Step(1),
        KeyCode::BackTab | KeyCode::Up => ReadKey::Step(-1),
        KeyCode::Char('p') if ctrl => ReadKey::Step(-1),
        KeyCode::PageDown => ReadKey::Page(1),
        KeyCode::PageUp => ReadKey::Page(-1),
        KeyCode::Backspace => ReadKey::Backspace,
        KeyCode::Char(c) if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT => {
            ReadKey::Insert(c)
        }
        _ => ReadKey::Ignore,
    }
}

/// One key of a completing read over `candidates` (see [`classify_read_key`]).
pub(super) fn step_completing_read(
    key: KeyEvent,
    input: &mut String,
    candidates: &[String],
    selected: &mut usize,
) -> CompletingRead {
    let count = || filter_candidates(candidates, input).len();
    match classify_read_key(key) {
        ReadKey::Cancel => return CompletingRead::Cancel,
        ReadKey::Accept => {
            let filtered = filter_candidates(candidates, input);
            return CompletingRead::Accept(filtered.get(*selected).cloned().unwrap_or_else(|| input.clone()));
        }
        ReadKey::Step(delta) | ReadKey::Page(delta) => {
            let count = count();
            if count > 0 {
                *selected = (*selected as isize + delta).rem_euclid(count as isize) as usize;
            }
        }
        ReadKey::Backspace => {
            input.pop();
            *selected = 0;
        }
        ReadKey::Insert(c) => {
            input.push(c);
            *selected = 0;
        }
        ReadKey::Ignore => {}
    }
    CompletingRead::Pending
}

/// `prefix input  [selected candidate]`, without the hint while the input
/// is empty or nothing matches.
fn completing_prompt(prefix: &str, input: &str, candidates: &[String], selected: usize) -> String {
    let filtered = filter_candidates(candidates, input);
    match filtered.get(selected) {
        Some(hint) if !input.is_empty() => format!("{prefix} {input}  [{hint}]"),
        _ => format!("{prefix} {input}"),
    }
}
