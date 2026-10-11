//! The command palette (docs/command-palette-spec.md §5–§7): M-x with a
//! window-level search panel. It lives in the minibuffer slot, so it takes
//! the keyboard from any tile; the backends draw its [`PaletteFrame`].
//!
//! The first stage lists commands: `:command` definitions (recently run
//! first on an empty query, matched on title and name while typing), then
//! anything else M-x can run, matched as a substring of its name. A command
//! with `:choices` opens a second stage that picks its argument — the field
//! shows the command's badge (`Theme`) and Backspace on an empty field goes
//! back.

use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use super::minibuffer::{ReadKey, classify_read_key};
use super::{Editor, MinibufferMode};
use crate::backend::{PALETTE_MAX_ROWS, PaletteFrame, PaletteRowFrame};
use crate::vm::Value;
use crate::widget_render::command_palette::{self, PaletteHit};

/// The command that opens the palette (bound to ⌘⇧P; M-x opens it too).
pub(super) const COMMAND_PALETTE: &str = "command-palette";
/// Recently run commands kept for the empty-query list.
const HISTORY_LIMIT: usize = 30;
/// Matches kept while typing; the list scrolls through these.
const MATCH_LIMIT: usize = 200;

#[derive(Debug, Clone)]
pub(super) struct PaletteState {
    /// The command list, then a choice stage on top while one is open.
    stages: Vec<Stage>,
    footer: Option<(String, String)>,
}

#[derive(Debug, Clone)]
struct Stage {
    kind: StageKind,
    entries: Vec<PaletteEntry>,
    input: String,
    /// Indices into `entries`, ranked for `input`.
    filtered: Vec<usize>,
    selected: usize,
    scroll: usize,
}

#[derive(Debug, Clone)]
enum StageKind {
    Commands,
    /// Picking the argument of `command`.
    Choices { command: String, prompt: String, values: Vec<Value> },
}

#[derive(Debug, Clone)]
struct PaletteEntry {
    /// What the row shows: a command's title, a choice's label, else the
    /// symbol.
    label: String,
    label_lower: String,
    /// The symbol M-x runs (commands stage); empty for a choice.
    name: String,
    name_lower: String,
    detail: Option<String>,
    submenu: bool,
    checked: bool,
    /// Curated: a `:command` or a choice, ranked above uncurated names and
    /// matched by subsequence too.
    curated: bool,
}

impl PaletteEntry {
    fn new(label: String, name: String, curated: bool) -> Self {
        Self {
            label_lower: label.to_lowercase(),
            name_lower: name.to_lowercase(),
            label,
            name,
            detail: None,
            submenu: false,
            checked: false,
            curated,
        }
    }
}

impl Stage {
    fn new(kind: StageKind, entries: Vec<PaletteEntry>) -> Self {
        Self { kind, entries, input: String::new(), filtered: Vec::new(), selected: 0, scroll: 0 }
    }

    /// Ranks `entries` for `input`. An empty query lists the curated
    /// entries — commands with recently run first (`history`), choices in
    /// their declared order.
    fn refilter(&mut self, history: &[String]) {
        self.selected = 0;
        self.scroll = 0;
        self.filtered = if self.input.is_empty() {
            let mut curated: Vec<usize> = (0..self.entries.len()).filter(|&idx| self.entries[idx].curated).collect();
            if matches!(self.kind, StageKind::Commands) {
                let recent_rank = |name: &str| history.iter().position(|recent| recent == name);
                curated.sort_by_key(|&idx| recent_rank(&self.entries[idx].name).unwrap_or(usize::MAX));
            }
            curated
        } else {
            let query = self.input.to_lowercase();
            let mut scored: Vec<(i32, usize)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, entry)| {
                    let score = if entry.curated {
                        let by_label = match_score(&entry.label_lower, &query);
                        let by_name = match_score(&entry.name_lower, &query).map(|score| score - 50);
                        by_label.max(by_name).map(|score| score + 2000)
                    } else {
                        // A subsequence over thousands of names is mostly
                        // noise: uncurated names match as substrings only.
                        substring_score(&entry.name_lower, &query)
                    };
                    score.map(|score| (score, idx))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            scored.truncate(MATCH_LIMIT);
            scored.into_iter().map(|(_, idx)| idx).collect()
        };
    }

    fn selected_entry(&self) -> Option<&PaletteEntry> {
        self.filtered.get(self.selected).map(|&idx| &self.entries[idx])
    }

    /// Moves the highlight; ±1 steps wrap, page moves clamp. Keeps the
    /// highlight inside the `page`-row scroll window.
    fn move_selection(&mut self, key: ReadKey, page: usize) {
        let count = self.filtered.len() as isize;
        if count == 0 {
            return;
        }
        let selected = self.selected as isize;
        self.selected = match key {
            ReadKey::Step(delta) => (selected + delta).rem_euclid(count),
            ReadKey::Page(pages) => (selected + pages * page as isize).clamp(0, count - 1),
            _ => selected,
        } as usize;
        self.scroll_to_selection(page);
    }

    fn scroll_to_selection(&mut self, page: usize) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + page {
            self.scroll = self.selected + 1 - page;
        }
    }

    /// Highlights the `row`th visible row. False when there is no such row
    /// or it is already highlighted.
    fn select_visible_row(&mut self, row: usize) -> bool {
        let idx = self.scroll + row;
        if idx >= self.filtered.len() || idx == self.selected {
            return false;
        }
        self.selected = idx;
        true
    }
}

impl PaletteState {
    fn top(&self) -> &Stage {
        self.stages.last().expect("the commands stage is never popped")
    }

    fn top_mut(&mut self) -> &mut Stage {
        self.stages.last_mut().expect("the commands stage is never popped")
    }

    fn frame(&self, page: usize) -> PaletteFrame {
        let stage = self.top();
        let rows = stage
            .filtered
            .iter()
            .skip(stage.scroll)
            .take(page)
            .map(|&idx| {
                let entry = &stage.entries[idx];
                PaletteRowFrame {
                    label: entry.label.clone(),
                    detail: entry.detail.clone(),
                    submenu: entry.submenu,
                    checked: entry.checked,
                }
            })
            .collect();
        let (badge, placeholder, empty) = match &stage.kind {
            StageKind::Commands => (None, "Search commands…".to_string(), "No matching commands"),
            StageKind::Choices { prompt, .. } => {
                (Some(prompt.clone()), format!("Select {prompt}…"), "No matches")
            }
        };
        PaletteFrame {
            badge,
            placeholder,
            input: stage.input.clone(),
            rows,
            selected: (!stage.filtered.is_empty()).then(|| stage.selected - stage.scroll),
            empty_text: stage.filtered.is_empty().then(|| empty.to_string()),
            footer: self.footer.clone(),
        }
    }
}

/// The lowest score a substring match earns.
const SUBSTRING_SCORE: i32 = 600;

/// How well `query` matches `text` as a substring (both lowercase): a
/// prefix beats a word start beats any other position.
fn substring_score(text: &str, query: &str) -> Option<i32> {
    let shortness = 100 - (text.len().min(100) as i32);
    if text.starts_with(query) {
        return Some(1000 + shortness);
    }
    let at = text.find(query)?;
    let word_start = text[..at].ends_with([' ', '-', '/', '.']);
    Some(if word_start { 800 } else { SUBSTRING_SCORE } + shortness)
}

/// [`substring_score`], falling back to an in-order subsequence that
/// rewards characters starting a word ("ct" → "change theme"). `None` when
/// the query's characters don't all appear in order.
fn match_score(text: &str, query: &str) -> Option<i32> {
    if let Some(score) = substring_score(text, query) {
        return Some(score);
    }
    let mut chars = text.chars();
    let mut score = 0;
    let mut prev: Option<char> = None;
    for wanted in query.chars() {
        loop {
            let ch = chars.next()?;
            let at_word_start = prev.is_none_or(|p| matches!(p, ' ' | '-' | '/' | '.'));
            prev = Some(ch);
            if ch == wanted {
                score += if at_word_start { 20 } else { 5 };
                break;
            }
        }
    }
    Some(score)
}

/// A `key_str` binding as the palette's right column shows it, in the
/// platform's notation (`S-s-p` → `⇧⌘P` on macOS); chords stay
/// space-separated (`C-x 2` → `⌃X 2`).
pub(super) fn binding_label(binding: &str) -> String {
    binding
        .split(' ')
        .map(|part| {
            let mut key = part;
            while key.len() > 2 && matches!(key.get(..2), Some("C-" | "M-" | "S-" | "s-")) {
                key = &key[2..];
            }
            let modifiers = &part[..part.len() - key.len()];
            // `key_str` spells keys it has no name for with `{:?}`.
            let key = match key {
                "Tab" => "⇥".to_string(),
                "BackTab" => "⇤".to_string(),
                "Home" => "↖".to_string(),
                "End" => "↘".to_string(),
                "PageUp" => "⇞".to_string(),
                "PageDown" => "⇟".to_string(),
                key if key.starts_with("F(") => key.replace(['(', ')'], ""),
                key => key.to_string(),
            };
            crate::widget_render::patcher::patcher_key_label(&format!("{modifiers}{key}"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Editor {
    /// Opens the palette on its commands stage (M-x, ⌘⇧P,
    /// `(command-palette)`).
    pub(super) fn open_command_palette(&mut self) {
        self.completion = None;
        self.minibuffer = None;
        command_palette::clear_hit_map();
        let bindings = self.binding_labels_by_handler();
        let commands: Vec<(String, String, bool, bool)> = self
            .runtime
            .commands()
            .into_iter()
            .map(|(name, header)| {
                let title = header.command.clone().unwrap_or_else(|| name.clone());
                (name, title, header.choices.is_some(), header.current.is_some())
            })
            .collect();
        let mut entries = Vec::new();
        for (name, title, has_choices, has_current) in &commands {
            let mut entry = PaletteEntry::new(title.clone(), name.clone(), true);
            entry.submenu = *has_choices;
            entry.detail = if *has_current {
                self.command_current_label(name)
            } else {
                bindings.get(name).cloned()
            };
            entries.push(entry);
        }
        let command_set: HashSet<&str> = commands.iter().map(|(name, ..)| name.as_str()).collect();
        for name in self.non_command_mx_names(&command_set) {
            let mut entry = PaletteEntry::new(name.clone(), name.clone(), false);
            entry.detail = bindings.get(&name).cloned();
            entries.push(entry);
        }
        let mut stage = Stage::new(StageKind::Commands, entries);
        stage.refilter(&self.palette_history);
        self.store_palette(Box::new(PaletteState { stages: vec![stage], footer: None }));
    }

    /// Moves an open palette onto `command`'s choice stage, with the
    /// commands stage beneath it for Backspace.
    pub(super) fn open_palette_choices(&mut self, command: &str) {
        let Some(MinibufferMode::Palette(mut state)) = self.minibuffer_input.take() else {
            return;
        };
        if self.runtime.definition_header(command).is_some_and(|header| header.choices.is_some()) {
            self.push_choice_stage(&mut state, command.to_string());
        } else {
            self.show_transient_message(format!("No command with :choices named {command}"));
        }
        self.store_palette(state);
    }

    /// The label of `command`'s current value: its choice's label when one
    /// matches, else the value itself.
    fn command_current_label(&mut self, command: &str) -> Option<String> {
        let current = self.runtime.command_current(command)?;
        let choices = self.runtime.command_choices(command).unwrap_or_default();
        Some(match choices.into_iter().find(|choice| choice.value == current) {
            Some(choice) => choice.label,
            None => match &current {
                Value::String(text) => text.clone(),
                other => self.runtime.format_value(other),
            },
        })
    }

    /// Opens `command`'s choice stage, highlighting its current value.
    fn push_choice_stage(&mut self, state: &mut PaletteState, command: String) {
        let choices = match self.runtime.command_choices(&command) {
            Ok(choices) => choices,
            Err(error) => {
                self.show_transient_message(format!("Error: {error}"));
                return;
            }
        };
        let current = self.runtime.command_current(&command);
        let prompt = self
            .runtime
            .definition_header(&command)
            .and_then(|header| header.choice_prompt())
            .unwrap_or_else(|| "Value".to_string());
        let mut values = Vec::with_capacity(choices.len());
        let entries = choices
            .into_iter()
            .map(|choice| {
                let mut entry = PaletteEntry::new(choice.label, String::new(), true);
                entry.detail = choice.detail;
                entry.checked = current.as_ref() == Some(&choice.value);
                values.push(choice.value);
                entry
            })
            .collect();
        let mut stage = Stage::new(StageKind::Choices { command, prompt, values }, entries);
        stage.refilter(&[]);
        if let Some(at) = stage.filtered.iter().position(|&idx| stage.entries[idx].checked) {
            stage.selected = at;
            stage.scroll_to_selection(self.palette_page());
        }
        state.stages.push(stage);
    }

    pub(super) fn handle_palette_key(&mut self, key: KeyEvent, mut state: Box<PaletteState>) {
        match classify_read_key(key) {
            ReadKey::Cancel => {
                self.close_palette();
                return;
            }
            ReadKey::Accept => {
                self.accept_palette_selection(state);
                return;
            }
            read @ (ReadKey::Step(_) | ReadKey::Page(_)) => {
                let page = self.palette_page();
                state.top_mut().move_selection(read, page);
            }
            ReadKey::Backspace => {
                let stage = state.top_mut();
                if stage.input.pop().is_some() {
                    stage.refilter(&self.palette_history);
                } else if state.stages.len() > 1 {
                    state.stages.pop();
                }
            }
            ReadKey::Insert(c) => {
                let stage = state.top_mut();
                stage.input.push(c);
                stage.refilter(&self.palette_history);
            }
            ReadKey::Ignore => {}
        }
        self.store_palette(state);
    }

    /// Puts `state` back in the minibuffer slot with its footer refreshed.
    fn store_palette(&mut self, mut state: Box<PaletteState>) {
        state.footer = self.palette_footer(&state);
        self.minibuffer_input = Some(MinibufferMode::Palette(state));
        self.mark_needs_redraw();
    }

    /// The palette is already out of the minibuffer slot: forget its drawn
    /// geometry and redraw without it.
    fn close_palette(&mut self) {
        self.minibuffer = None;
        command_palette::clear_hit_map();
        self.mark_needs_redraw();
    }

    /// Enter (or a click) on the highlighted row: a command with
    /// `:choices` opens its choice stage, any other command runs, and a
    /// choice runs its command with the chosen value. With nothing
    /// highlighted on the commands stage, the raw input runs as a name.
    fn accept_palette_selection(&mut self, mut state: Box<PaletteState>) {
        let stage = state.top();
        let selected = stage.selected_entry().cloned();
        match (&stage.kind, selected) {
            (StageKind::Commands, Some(entry)) if entry.submenu => {
                self.push_choice_stage(&mut state, entry.name);
                self.store_palette(state);
            }
            (StageKind::Commands, Some(entry)) => {
                self.close_palette();
                if entry.curated {
                    self.record_palette_use(&entry.name);
                }
                self.execute_mx_command(&entry.name);
            }
            (StageKind::Commands, None) => {
                let input = stage.input.clone();
                self.close_palette();
                if !input.is_empty() {
                    self.execute_mx_command(&input);
                }
            }
            (StageKind::Choices { command, values, .. }, Some(_)) => {
                let command = command.clone();
                let value = values[stage.filtered[stage.selected]].clone();
                self.close_palette();
                self.record_palette_use(&command);
                self.call_lisp_handler_with_args(&command, &[value]);
            }
            (StageKind::Choices { .. }, None) => self.store_palette(state),
        }
    }

    /// Pointer input while the palette is open: hover highlights a row, a
    /// click on it accepts it, the wheel moves the highlight, and a press
    /// outside closes the palette. Returns true when the event was consumed.
    pub(super) fn handle_palette_mouse(&mut self, mouse: MouseEvent, col: f32, row: f32) -> bool {
        if !self.palette_is_open() {
            return false;
        }
        let Some(hit) = command_palette::hit_map().map(|map| map.hit(col, row)) else {
            return false;
        };
        let page = self.palette_page();
        let changed = match (mouse.kind, hit) {
            (MouseEventKind::Down(_), PaletteHit::Outside) => {
                self.minibuffer_input = None;
                self.close_palette();
                return true;
            }
            (_, PaletteHit::Outside) => return false,
            (MouseEventKind::Up(MouseButton::Left), PaletteHit::Row(row)) => {
                let Some(MinibufferMode::Palette(mut state)) = self.minibuffer_input.take() else {
                    return false;
                };
                let stage = state.top_mut();
                if row + stage.scroll < stage.filtered.len() {
                    stage.select_visible_row(row);
                    self.accept_palette_selection(state);
                } else {
                    self.minibuffer_input = Some(MinibufferMode::Palette(state));
                }
                return true;
            }
            (kind, hit) => {
                let Some(MinibufferMode::Palette(state)) = &mut self.minibuffer_input else {
                    return false;
                };
                let stage = state.top_mut();
                let before = stage.selected;
                match (kind, hit) {
                    (MouseEventKind::Moved | MouseEventKind::Down(_), PaletteHit::Row(row)) => {
                        stage.select_visible_row(row);
                    }
                    (MouseEventKind::ScrollDown, _) => stage.move_selection(ReadKey::Step(1), page),
                    (MouseEventKind::ScrollUp, _) => stage.move_selection(ReadKey::Step(-1), page),
                    _ => {}
                }
                stage.selected != before
            }
        };
        if changed && let Some(MinibufferMode::Palette(state)) = self.minibuffer_input.take() {
            self.store_palette(state);
        }
        true
    }

    /// Rows the palette shows at once: what fit the window when it was last
    /// drawn.
    fn palette_page(&self) -> usize {
        command_palette::hit_map().map_or(PALETTE_MAX_ROWS, |map| map.capacity).max(1)
    }

    pub fn palette_is_open(&self) -> bool {
        matches!(self.minibuffer_input, Some(MinibufferMode::Palette(_)))
    }

    /// The palette as the backends draw it, while open.
    pub fn palette_frame(&self) -> Option<PaletteFrame> {
        match &self.minibuffer_input {
            Some(MinibufferMode::Palette(state)) => Some(state.frame(self.palette_page())),
            _ => None,
        }
    }

    /// The status-line echo of an open palette (the TUI's only view of it).
    pub(super) fn palette_prompt(state: &PaletteState) -> String {
        let stage = state.top();
        let prefix = match &stage.kind {
            StageKind::Commands => "M-x".to_string(),
            StageKind::Choices { prompt, .. } => format!("{prompt}:"),
        };
        match stage.selected_entry() {
            Some(entry) if !stage.input.is_empty() => format!("{prefix} {}  [{}]", stage.input, entry.label),
            _ => format!("{prefix} {}", stage.input),
        }
    }

    /// Where the palette keeps its recently-run list (one name per line);
    /// `None` keeps it in memory only. Reads the list now.
    pub fn set_palette_history_path(&mut self, path: Option<PathBuf>) {
        self.palette_history = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| {
                text.lines().filter(|line| !line.is_empty()).take(HISTORY_LIMIT).map(str::to_string).collect()
            })
            .unwrap_or_default();
        self.palette_history_path = path;
    }

    fn record_palette_use(&mut self, name: &str) {
        if self.palette_history.first().is_some_and(|recent| recent == name) {
            return;
        }
        self.palette_history.retain(|recent| recent != name);
        self.palette_history.insert(0, name.to_string());
        self.palette_history.truncate(HISTORY_LIMIT);
        let Some(path) = &self.palette_history_path else {
            return;
        };
        let mut text = self.palette_history.join("\n");
        text.push('\n');
        // Write beside, then rename: a crash mid-write never truncates it.
        let temp = path.with_extension("tmp");
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&temp, text))
            .and_then(|()| std::fs::rename(&temp, path));
        if let Err(error) = written {
            eprintln!("command palette: could not save history to {}: {error}", path.display());
        }
    }

    /// Display label of each handler's shortest key binding (ties broken
    /// by spelling, so the label is stable).
    fn binding_labels_by_handler(&self) -> std::collections::HashMap<String, String> {
        let mut shortest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let builtin_keys = self.builtins.iter().map(|(key, name)| (super::commands::key_str(*key), name));
        let lisp_keys = self.lisp_bindings.iter().map(|(key, name)| (key.clone(), name));
        for (key, handler) in builtin_keys.chain(lisp_keys) {
            let handler = crate::modules::strip_implicit(handler).to_string();
            let keep = shortest.get(&handler).is_none_or(|current| (key.len(), &key) < (current.len(), current));
            if keep {
                shortest.insert(handler, key);
            }
        }
        shortest.into_iter().map(|(handler, key)| (handler, binding_label(&key))).collect()
    }

    /// Signature and first doc line of the highlighted command (commands
    /// stage), or of the command a choice stage picks for.
    fn palette_footer(&mut self, state: &PaletteState) -> Option<(String, String)> {
        let stage = state.top();
        let name = match &stage.kind {
            StageKind::Commands => stage.selected_entry()?.name.clone(),
            StageKind::Choices { command, .. } => command.clone(),
        };
        if let Some(header) = self.runtime.definition_header(&name) {
            return Some((header.signature.clone(), header.summary().unwrap_or_default().to_string()));
        }
        let metadata = self.runtime.completion_metadata_for(&name)?;
        let summary = crate::lang::def_header::first_doc_line(&metadata.docs).unwrap_or_default().to_string();
        Some((metadata.signature, summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorConfig, Runtime};
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(editor: &mut Editor, text: &str) {
        for ch in text.chars() {
            editor.handle_key(key(KeyCode::Char(ch)));
        }
    }

    fn palette_editor() -> Editor {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(
                r#"(def ran nil)
                   (def pal-current-theme "b")
                   (def pal-themes () (list (dict :label "Alpha" :value "a" :detail "light")
                                            (dict :label "Beta" :value "b")))
                   (def pal-theme (theme) :doc "Switch the theme." :command "Change Theme"
                     :prompt "Theme" :choices pal-themes :current pal-current-theme
                     (set! ran theme))
                   (def pal-new-track () :doc "Add a track." :command "New Track" (set! ran "track"))
                   (def pal-export () :command "Export Song" (set! ran "export"))
                   (def theme-helper () 1)"#,
            )
            .unwrap();
        editor
    }

    fn labels(editor: &Editor) -> Vec<String> {
        editor.palette_frame().expect("open").rows.into_iter().map(|row| row.label).collect()
    }

    fn ran(editor: &Editor) -> Option<Value> {
        editor.runtime().global_value("ran")
    }

    #[test]
    fn prefix_beats_word_start_beats_substring_beats_subsequence() {
        let prefix = match_score("change theme", "chan").unwrap();
        let word = match_score("change theme", "them").unwrap();
        let substring = match_score("change theme", "hang").unwrap();
        let subsequence = match_score("change theme", "ct").unwrap();
        assert!(prefix > word && word > substring && substring > subsequence);
        assert!(match_score("change theme", "tc").is_none());
        assert!(substring_score("change theme", "ct").is_none());
    }

    #[test]
    fn bindings_render_in_platform_notation() {
        let macos = crate::ui::platform::CURRENT_SHORTCUT_PLATFORM
            == crate::ui::platform::ShortcutPlatform::MacOS;
        if macos {
            assert_eq!(binding_label("S-s-p"), "⇧⌘P");
            assert_eq!(binding_label("C-x 2"), "⌃X 2");
            assert_eq!(binding_label("C-Tab"), "⌃⇥");
            assert_eq!(binding_label("F(5)"), "F5");
        } else {
            assert_eq!(binding_label("C-x 2"), "Ctrl+X 2");
        }
    }

    #[test]
    fn empty_query_lists_commands_by_title_then_typing_ranks_commands_first() {
        let mut editor = palette_editor();
        editor.run_command("execute-extended-command");
        assert_eq!(labels(&editor), ["Change Theme", "Export Song", "New Track"]);
        let frame = editor.palette_frame().unwrap();
        assert!(frame.rows[0].submenu, "a :choices command shows ›");
        assert_eq!(frame.rows[0].detail.as_deref(), Some("Beta"), ":current shows its choice's label");
        assert_eq!(frame.footer, Some(("(pal-theme theme)".into(), "Switch the theme.".into())));

        type_text(&mut editor, "theme");
        let rows = labels(&editor);
        assert_eq!(rows[0], "Change Theme");
        assert!(rows.iter().any(|row| row == "theme-helper"), "{rows:?}");
        assert!(!rows.iter().any(|row| row == "New Track"), "{rows:?}");
        editor.handle_key(key(KeyCode::Esc));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "ran");
        assert!(!labels(&editor).iter().any(|row| row == "ran"), "plain variables are not commands");
    }

    #[test]
    fn a_choices_command_opens_a_badged_stage_and_runs_with_the_pick() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        editor.handle_key(key(KeyCode::Enter));
        let frame = editor.palette_frame().expect("still open on the choice stage");
        assert_eq!(frame.badge.as_deref(), Some("Theme"));
        assert_eq!(frame.placeholder, "Select Theme…");
        assert_eq!(labels(&editor), ["Alpha", "Beta"]);
        assert_eq!(frame.selected, Some(1), "opens on the current choice");
        assert!(frame.rows[1].checked && !frame.rows[0].checked);
        assert_eq!(frame.rows[0].detail.as_deref(), Some("light"));

        // Backspace on an empty field goes back to the commands.
        editor.handle_key(key(KeyCode::Backspace));
        assert_eq!(editor.palette_frame().unwrap().badge, None);
        editor.handle_key(key(KeyCode::Enter));

        type_text(&mut editor, "alp");
        editor.handle_key(key(KeyCode::Enter));
        assert!(editor.palette_frame().is_none());
        assert_eq!(ran(&editor), Some(Value::String("a".into())));
    }

    #[test]
    fn enter_runs_the_selection_closes_and_floats_it_to_the_top() {
        let mut editor = palette_editor();
        let history = std::env::temp_dir().join(format!("palette-history-{}", std::process::id()));
        let _ = std::fs::remove_file(&history);
        editor.set_palette_history_path(Some(history.clone()));
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "new");
        editor.handle_key(key(KeyCode::Enter));
        assert!(editor.palette_frame().is_none());
        assert_eq!(ran(&editor), Some(Value::String("track".into())));
        assert_eq!(std::fs::read_to_string(&history).unwrap(), "pal-new-track\n");

        // An uncurated name runs but stays out of the history.
        editor.run_command(COMMAND_PALETTE);
        type_text(&mut editor, "theme-helper");
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(std::fs::read_to_string(&history).unwrap(), "pal-new-track\n");

        editor.run_command(COMMAND_PALETTE);
        assert_eq!(labels(&editor)[0], "New Track");
        editor.handle_key(key(KeyCode::Esc));
        assert!(editor.palette_frame().is_none());

        // A fresh editor reads the history back.
        let mut fresh = palette_editor();
        fresh.set_palette_history_path(Some(history.clone()));
        fresh.run_command(COMMAND_PALETTE);
        assert_eq!(labels(&fresh)[0], "New Track");
        let _ = std::fs::remove_file(&history);
    }

    #[test]
    fn steps_wrap_but_pages_clamp() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        editor.handle_key(key(KeyCode::Up));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(2));
        editor.handle_key(key(KeyCode::Down));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(0));
        editor.handle_key(key(KeyCode::PageDown));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(2));
        editor.handle_key(key(KeyCode::PageDown));
        assert_eq!(editor.palette_frame().unwrap().selected, Some(2));
        editor.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert!(editor.palette_frame().is_none());
    }

    #[test]
    fn pointer_runs_a_clicked_row_and_a_press_outside_closes() {
        let mut editor = palette_editor();
        editor.run_command(COMMAND_PALETTE);
        let viewport = crate::widget_render::WidgetViewport::window(10.0, 20.0, 1400.0, 900.0, 0.0);
        command_palette::primitives(&editor.palette_frame().unwrap(), viewport);
        let map = command_palette::hit_map().expect("drawn");
        let at = |kind: MouseEventKind, col: f32, row: f32| MouseEvent {
            kind,
            column: col as u16,
            row: row as u16,
            modifiers: KeyModifiers::NONE,
        };
        let row_1 = map.list_top + map.row_height * 1.5;
        let col = map.panel.col + 5.0;
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Moved, col, row_1), col, row_1, 0);
        assert_eq!(editor.palette_frame().unwrap().selected, Some(1));
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Up(MouseButton::Left), col, row_1), col, row_1, 0);
        assert_eq!(ran(&editor), Some(Value::String("export".into())));
        assert!(editor.palette_frame().is_none());

        editor.run_command(COMMAND_PALETTE);
        command_palette::primitives(&editor.palette_frame().unwrap(), viewport);
        editor.handle_tiled_mouse_precise(at(MouseEventKind::Down(MouseButton::Left), 1.0, 1.0), 1.0, 1.0, 0);
        assert!(editor.palette_frame().is_none());
    }

    #[test]
    fn the_command_palette_native_opens_it_optionally_on_a_choice_stage() {
        let mut editor = palette_editor();
        editor.runtime_mut().eval_str("(command-palette)").unwrap();
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.palette_frame().unwrap().badge, None);
        editor.handle_key(key(KeyCode::Esc));

        editor.runtime_mut().eval_str(r#"(command-palette "pal-theme")"#).unwrap();
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.palette_frame().unwrap().badge.as_deref(), Some("Theme"));
        editor.handle_key(key(KeyCode::Backspace));
        assert_eq!(editor.palette_frame().unwrap().badge, None, "the commands stage is beneath");
    }
}
