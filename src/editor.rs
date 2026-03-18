use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Buffer;
use crate::host::{BufferId, CompileKind, HostCommand, HostEvent};
use crate::mode::{
    BufferMode, CompletionItem, CompletionMatch, TokenSpan, completion_match, highlight_line,
};
use crate::runtime::Runtime;
use crate::text::{innermost_sexp_range_at_cursor, sexp_at_cursor};
use crate::vm::{Value, format_lisp_value};

#[derive(Default, Clone)]
pub struct EditorConfig {
    pub init_source: Option<String>,
}

#[derive(Debug)]
pub enum EditorError {
    Io(std::io::Error),
    Message(String),
}

impl From<std::io::Error> for EditorError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorExit {
    Cancelled,
    Closed,
    SavedAndClosed,
}

type LispBindings = HashMap<String, String>;

struct SavePrompt {
    input: String,
    quit_after_save: bool,
}

#[derive(Debug, Clone)]
pub struct CompletionState {
    pub start_col: usize,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone)]
pub struct SExpFlash {
    pub buffer_id: BufferId,
    pub range: ((usize, usize), (usize, usize)),
    pub expires_at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub buffer_id: BufferId,
    pub cursor: (usize, usize),
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub active: usize,
    pub minibuffer: Option<String>,

    pending_key: Option<KeyEvent>,
    builtins: HashMap<KeyEvent, String>,
    lisp_bindings: LispBindings,
    runtime: Runtime,
    needs_redraw: bool,
    should_quit: bool,
    last_exit: EditorExit,
    next_buffer_id: BufferId,
    save_prompt: Option<SavePrompt>,
    completion: Option<CompletionState>,
    eval_flash: Option<SExpFlash>,
    mark: Option<Mark>,
    kill_ring: Vec<String>,
}

impl Editor {
    pub fn new(mut runtime: Runtime, config: EditorConfig) -> Self {
        register_editor_natives(&mut runtime);

        let mut editor = Editor {
            buffers: vec![Buffer::new(0, "*scratch*")],
            active: 0,
            minibuffer: None,
            pending_key: None,
            builtins: HashMap::new(),
            lisp_bindings: HashMap::new(),
            runtime,
            needs_redraw: true,
            should_quit: false,
            last_exit: EditorExit::Closed,
            next_buffer_id: 1,
            save_prompt: None,
            completion: None,
            eval_flash: None,
            mark: None,
            kill_ring: vec![],
        };
        editor.bind_defaults();
        editor.load_init(config.init_source.as_deref());
        editor.refresh_runtime_side_effects();
        editor.sync_runtime_context();
        editor
    }

    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    pub fn clear_needs_redraw(&mut self) {
        self.needs_redraw = false;
    }

    pub fn mark_needs_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn clear_quit_request(&mut self) {
        self.should_quit = false;
        self.mark_needs_redraw();
    }

    pub fn prompt_text(&self) -> Option<String> {
        self.save_prompt
            .as_ref()
            .map(|prompt| format!(" Save as: {}", prompt.input))
    }

    pub fn completion_state(&self) -> Option<&CompletionState> {
        self.completion.as_ref()
    }

    pub fn active_highlight_spans(&self) -> Vec<Vec<TokenSpan>> {
        let symbols = self.runtime.completion_symbols();
        self.active_buffer()
            .lines
            .iter()
            .map(|line| {
                highlight_line(
                    self.active_buffer().mode,
                    line,
                    &symbols,
                    self.active_buffer(),
                )
            })
            .collect()
    }

    pub fn active_sexp_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let buffer = self.active_buffer();
        innermost_sexp_range_at_cursor(&buffer.lines, buffer.cursor)
    }

    pub fn active_eval_flash_range(&mut self) -> Option<((usize, usize), (usize, usize))> {
        let Some((buffer_id, range, expires_at)) = self
            .eval_flash
            .as_ref()
            .map(|flash| (flash.buffer_id, flash.range, flash.expires_at))
        else {
            return None;
        };
        if expires_at <= Instant::now() {
            self.eval_flash = None;
            return None;
        }
        self.mark_needs_redraw();
        let active_id = self.active_buffer().id;
        (buffer_id == active_id).then_some(range)
    }

    pub fn active_region_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let mark = self.mark?;
        let buffer = self.active_buffer();
        if mark.buffer_id != buffer.id || mark.cursor == buffer.cursor {
            return None;
        }
        Some(normalize_region(mark.cursor, buffer.cursor))
    }

    pub fn active_buffer(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    pub fn open_scratch_buffer(&mut self, name: &str, initial: &str) -> BufferId {
        self.open_scratch_buffer_with_mode(name, initial, BufferMode::ESeqLisp)
    }

    pub fn open_scratch_buffer_with_mode(
        &mut self,
        name: &str,
        initial: &str,
        mode: BufferMode,
    ) -> BufferId {
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::from_text(id, name, initial);
        buffer.set_mode(mode);
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.mark_needs_redraw();
        self.sync_runtime_context();
        self.refresh_completion();
        id
    }

    pub fn open_file_buffer(&mut self, path: impl Into<PathBuf>) -> Result<BufferId, EditorError> {
        self.open_file_buffer_with_mode(path, BufferMode::ESeqLisp)
    }

    pub fn open_file_buffer_with_mode(
        &mut self,
        path: impl Into<PathBuf>,
        mode: BufferMode,
    ) -> Result<BufferId, EditorError> {
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::from_file(id, path)?;
        buffer.set_mode(mode);
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.mark_needs_redraw();
        self.sync_runtime_context();
        self.refresh_completion();
        Ok(id)
    }

    pub fn open_or_create_file_buffer(
        &mut self,
        path: impl Into<PathBuf>,
        initial: &str,
    ) -> Result<BufferId, EditorError> {
        self.open_or_create_file_buffer_with_mode(path, initial, BufferMode::ESeqLisp)
    }

    pub fn open_or_create_file_buffer_with_mode(
        &mut self,
        path: impl Into<PathBuf>,
        initial: &str,
        mode: BufferMode,
    ) -> Result<BufferId, EditorError> {
        let path = path.into();
        if Path::new(&path).exists() {
            self.open_file_buffer_with_mode(path, mode)
        } else {
            let id = self.alloc_buffer_id();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let mut buffer = Buffer::from_text(id, name, initial);
            buffer.set_path(path);
            buffer.set_mode(mode);
            buffer.dirty = false;
            self.buffers.push(buffer);
            self.active = self.buffers.len() - 1;
            self.mark_needs_redraw();
            self.sync_runtime_context();
            self.refresh_completion();
            Ok(id)
        }
    }

    pub fn set_active_buffer(&mut self, id: BufferId) {
        if let Some(index) = self.buffers.iter().position(|buffer| buffer.id == id) {
            self.active = index;
            self.mark_needs_redraw();
            self.sync_runtime_context();
            self.completion = None;
            self.clear_mark();
        }
    }

    pub fn handle_host_event(&mut self, event: HostEvent) {
        let message = match event {
            HostEvent::Status(msg) => msg,
            HostEvent::Error(msg) => format!("Error: {msg}"),
            HostEvent::CommandStarted { label } => format!("{label}..."),
            HostEvent::CommandFinished {
                label,
                success,
                message,
            } => {
                let outcome = if success { "finished" } else { "failed" };
                match message {
                    Some(message) => format!("{label} {outcome}: {message}"),
                    None => format!("{label} {outcome}"),
                }
            }
            HostEvent::CompileFinished {
                kind,
                success,
                name,
                diagnostics,
            } => {
                let label = match kind {
                    CompileKind::Instrument => "instrument",
                    CompileKind::Effect => "effect",
                };
                if success {
                    match name {
                        Some(name) => format!("Compiled {label} '{name}'"),
                        None => format!("Compiled {label}"),
                    }
                } else {
                    match diagnostics {
                        Some(diag) => format!("Compile failed ({label}): {diag}"),
                        None => format!("Compile failed ({label})"),
                    }
                }
            }
            HostEvent::BufferSaved { buffer_id, path } => {
                if let Some(buffer) = self
                    .buffers
                    .iter_mut()
                    .find(|buffer| buffer.id == buffer_id)
                {
                    buffer.set_path(path.clone());
                    buffer.dirty = false;
                }
                format!("Saved {}", path.display())
            }
        };
        self.minibuffer = Some(message);
        self.mark_needs_redraw();
        self.sync_runtime_context();
        self.completion = None;
    }

    pub fn drain_host_commands(&mut self) -> Vec<HostCommand> {
        self.runtime.drain_host_commands()
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self.runtime
    }

    pub fn into_runtime(self) -> Runtime {
        self.runtime
    }

    pub fn run_embedded(&mut self) -> Result<EditorExit, EditorError> {
        loop {
            if event::poll(std::time::Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) => self.handle_key(key),
                    Event::Resize(_, _) => self.mark_needs_redraw(),
                    _ => {}
                }
            }
            if self.should_quit {
                return Ok(self.last_exit);
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.mark_needs_redraw();

        if self.handle_save_prompt_key(key) {
            return;
        }

        if self.handle_completion_key(key) {
            return;
        }

        if let Some(prefix) = self.pending_key.take() {
            let chord = format!("{} {}", key_str(prefix), key_str(key));
            if let Some(handler) = self.lisp_bindings.get(&chord).cloned() {
                self.call_lisp_handler(&handler);
            }
            return;
        }

        if self.binding_has_prefix(&key_str(key)) {
            self.pending_key = Some(key);
            return;
        }

        if let Some(cmd) = self.builtins.get(&key).cloned() {
            self.run_command(&cmd);
            return;
        }

        if let Some(handler) = self.lisp_bindings.get(&key_str(key)).cloned() {
            self.call_lisp_handler(&handler);
            return;
        }

        match key.code {
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().insert_char(c);
                self.sync_runtime_context();
                self.refresh_completion();
            }
            KeyCode::Enter => {
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().insert_newline_with_indent();
                self.sync_runtime_context();
            }
            _ => {}
        }
    }

    fn bind_defaults(&mut self) {
        let binds: &[(KeyCode, KeyModifiers, &str)] = &[
            (KeyCode::Char('q'), KeyModifiers::CONTROL, "quit"),
            (KeyCode::Char('s'), KeyModifiers::CONTROL, "save-buffer"),
            (KeyCode::Char(' '), KeyModifiers::CONTROL, "set-mark"),
            (KeyCode::Char('a'), KeyModifiers::CONTROL, "move-line-start"),
            (KeyCode::Char('e'), KeyModifiers::CONTROL, "move-line-end"),
            (
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
                "kill-region-or-word",
            ),
            (KeyCode::Char('w'), KeyModifiers::ALT, "copy-region"),
            (KeyCode::Char('y'), KeyModifiers::CONTROL, "yank"),
            (
                KeyCode::Char('k'),
                KeyModifiers::CONTROL,
                "delete-to-line-end",
            ),
            (KeyCode::Tab, KeyModifiers::NONE, "complete"),
            (KeyCode::Left, KeyModifiers::CONTROL, "move-word-left"),
            (KeyCode::Right, KeyModifiers::CONTROL, "move-word-right"),
            (KeyCode::Char('b'), KeyModifiers::ALT, "move-word-left"),
            (KeyCode::Char('f'), KeyModifiers::ALT, "move-word-right"),
            (KeyCode::Left, KeyModifiers::NONE, "move-left"),
            (KeyCode::Right, KeyModifiers::NONE, "move-right"),
            (KeyCode::Up, KeyModifiers::NONE, "move-up"),
            (KeyCode::Down, KeyModifiers::NONE, "move-down"),
            (KeyCode::Backspace, KeyModifiers::NONE, "delete-char-before"),
        ];
        for (code, mods, cmd) in binds {
            self.builtins
                .insert(KeyEvent::new(*code, *mods), cmd.to_string());
        }
    }

    fn load_init(&mut self, override_source: Option<&str>) {
        let init_src = override_source
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| std::fs::read_to_string("init.lisp").unwrap_or_default());
        if init_src.trim().is_empty() {
            return;
        }
        let _ = self.runtime.eval_str(&init_src);
        self.refresh_runtime_side_effects();
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        }
    }

    fn run_command(&mut self, cmd: &str) {
        match cmd {
            "quit" => {
                self.completion = None;
                if self.needs_save_as_prompt() {
                    self.open_save_prompt(true);
                } else {
                    self.should_quit = true;
                    self.last_exit = EditorExit::Closed;
                }
            }
            "set-mark" => {
                self.completion = None;
                self.minibuffer = Some("Mark set".to_string());
                self.mark = Some(Mark {
                    buffer_id: self.active_buffer().id,
                    cursor: self.active_buffer().cursor,
                });
            }
            "move-left" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_left();
            }
            "move-right" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_right();
            }
            "move-up" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_up();
            }
            "move-down" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_down();
            }
            "move-buffer-end" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_to_buffer_end();
            }

            "move-line-start" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_to_line_start();
            }
            "move-line-end" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_to_line_end();
            }
            "move-word-left" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_word_left();
            }
            "move-word-right" => {
                self.completion = None;
                self.minibuffer = None;
                self.active_buffer_mut().move_word_right();
            }
            "delete-char-before" => {
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().delete_char_before();
                self.refresh_completion();
            }
            "kill-region-or-word" => {
                self.minibuffer = None;
                if !self.kill_active_region() {
                    self.active_buffer_mut().delete_word_before();
                }
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
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                if let Some(text) = self.kill_ring.last().cloned() {
                    self.active_buffer_mut().insert_str(&text);
                } else {
                    self.minibuffer = Some("Kill ring is empty".to_string());
                }
            }
            "delete-to-line-end" => {
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().delete_to_line_end();
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
                    self.active_buffer_mut().indent_current_line();
                    self.sync_runtime_context();
                }
            }
            _ => {}
        }
        self.sync_runtime_context();
    }

    fn call_lisp_handler(&mut self, fn_name: &str) {
        if fn_name == "eval-sexp" {
            self.start_eval_flash();
        }
        self.sync_runtime_context();
        self.minibuffer = None;
        let code = format!("({fn_name})");
        match self.runtime.eval_str(&code) {
            Ok(Some(result)) => self.minibuffer = Some(format_value_for_minibuffer(&result)),
            Ok(None) => self.minibuffer = Some("No result".to_string()),
            Err(e) => self.minibuffer = Some(format!("Error: {e:?}")),
        }
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        }
        self.refresh_runtime_side_effects();
        self.sync_runtime_context();
        self.completion = None;
    }

    fn save_active_buffer(&mut self) -> Result<PathBuf, EditorError> {
        let path = self.active_buffer_mut().save()?;
        self.last_exit = EditorExit::SavedAndClosed;
        Ok(path)
    }

    fn sync_runtime_context(&mut self) {
        let active = self.active_buffer();
        let mut shared = self.runtime.shared.borrow_mut();
        shared.current_buffer_id = Some(active.id);
        shared.current_buffer_name = active.name.clone();
        shared.current_buffer_path = active.path.clone();
        shared.current_buffer_text = active.text();
        shared.current_sexp = sexp_at_cursor(&active.lines, active.cursor);
    }

    fn handle_completion_key(&mut self, key: KeyEvent) -> bool {
        let Some(completion) = self.completion.as_mut() else {
            return false;
        };

        match key.code {
            KeyCode::Up => {
                if completion.selected > 0 {
                    completion.selected -= 1;
                }
                completion.ensure_visible();
                self.mark_needs_redraw();
                true
            }
            KeyCode::Down => {
                if completion.selected + 1 < completion.items.len() {
                    completion.selected += 1;
                }
                completion.ensure_visible();
                self.mark_needs_redraw();
                true
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.accept_completion();
                true
            }
            KeyCode::Esc => {
                self.completion = None;
                self.mark_needs_redraw();
                true
            }
            _ => false,
        }
    }

    fn accept_completion(&mut self) {
        let Some(completion) = self.completion.clone() else {
            return;
        };
        let Some(item) = completion.items.get(completion.selected) else {
            return;
        };
        let buffer = self.active_buffer_mut();
        let row = buffer.cursor.0;
        let end_col = buffer.cursor.1.min(buffer.lines[row].len());
        buffer.lines[row].replace_range(completion.start_col..end_col, &item.label);
        buffer.cursor.1 = completion.start_col + item.label.len();
        buffer.dirty = true;
        self.completion = None;
        self.sync_runtime_context();
    }

    fn refresh_completion(&mut self) {
        if self.save_prompt.is_some() {
            self.completion = None;
            return;
        }
        let symbols = self.runtime.completion_symbols();
        let metadata = self.runtime.completion_metadata();
        let previous = self
            .completion
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .map(|item| item.label.clone());
        self.completion = completion_match(
            self.active_buffer().mode,
            self.active_buffer(),
            &symbols,
            &metadata,
        )
        .map(
            |CompletionMatch {
                 start_col, items, ..
             }| {
                let selected = previous
                    .as_ref()
                    .and_then(|label| items.iter().position(|item| item.label == *label))
                    .unwrap_or(0);
                CompletionState {
                    start_col,
                    items,
                    selected,
                    scroll: 0,
                }
            },
        )
        .map(|mut state| {
            state.ensure_visible();
            state
        });
    }

    fn alloc_buffer_id(&mut self) -> BufferId {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        id
    }

    fn binding_has_prefix(&self, prefix: &str) -> bool {
        self.lisp_bindings.keys().any(|binding| {
            binding
                .strip_prefix(prefix)
                .map(|rest| rest.starts_with(' '))
                .unwrap_or(false)
        })
    }

    fn needs_save_as_prompt(&self) -> bool {
        self.active_buffer()
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem == "untitled")
            .unwrap_or(true)
    }

    fn open_save_prompt(&mut self, quit_after_save: bool) {
        let default_name = self
            .active_buffer()
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| {
                let stem = stem.to_string_lossy().to_string();
                if stem == "untitled" { None } else { Some(stem) }
            })
            .unwrap_or_default();
        self.save_prompt = Some(SavePrompt {
            input: default_name,
            quit_after_save,
        });
        self.sync_runtime_context();
    }

    fn handle_save_prompt_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.save_prompt.as_mut() else {
            return false;
        };

        match key.code {
            KeyCode::Esc => {
                self.save_prompt = None;
                self.minibuffer = Some("Save cancelled".to_string());
            }
            KeyCode::Enter => {
                let quit_after_save = prompt.quit_after_save;
                let input = prompt.input.trim().to_string();
                if input.is_empty() {
                    self.minibuffer = Some("Filename required".to_string());
                    return true;
                }
                let mut target = self
                    .active_buffer()
                    .path
                    .as_ref()
                    .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
                    .unwrap_or_default();
                let filename = if input.ends_with(".lisp") {
                    input
                } else {
                    format!("{input}.lisp")
                };
                target.push(filename);
                match self.active_buffer_mut().save_as(target) {
                    Ok(path) => {
                        self.minibuffer = Some(format!("Saved {}", path.display()));
                        self.save_prompt = None;
                        if quit_after_save {
                            self.should_quit = true;
                            self.last_exit = EditorExit::SavedAndClosed;
                        }
                    }
                    Err(error) => {
                        self.minibuffer = Some(format!("Error: {error}"));
                    }
                }
            }
            KeyCode::Backspace => {
                prompt.input.pop();
            }
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                prompt.input.push(c);
            }
            _ => {}
        }
        self.mark_needs_redraw();
        self.sync_runtime_context();
        true
    }

    fn refresh_runtime_side_effects(&mut self) {
        self.lisp_bindings = self.runtime.lisp_bindings();

        if let Some(path) = self.runtime.take_pending_save_as() {
            match self.active_buffer_mut().save_as(path) {
                Ok(path) => self.minibuffer = Some(format!("Saved {}", path.display())),
                Err(error) => self.minibuffer = Some(format!("Error: {error}")),
            }
        } else if self.runtime.take_pending_save() {
            match self.save_active_buffer() {
                Ok(path) => self.minibuffer = Some(format!("Saved {}", path.display())),
                Err(error) => self.minibuffer = Some(format!("Error: {error:?}")),
            }
        }
        self.completion = None;
    }

    fn start_eval_flash(&mut self) {
        let buffer = self.active_buffer();
        let Some(range) = innermost_sexp_range_at_cursor(&buffer.lines, buffer.cursor) else {
            self.eval_flash = None;
            return;
        };
        self.eval_flash = Some(SExpFlash {
            buffer_id: buffer.id,
            range,
            expires_at: Instant::now() + Duration::from_millis(350),
        });
    }

    fn clear_mark(&mut self) {
        self.mark = None;
    }

    fn copy_active_region(&mut self) -> bool {
        let Some((start, end)) = self.active_region_range() else {
            return false;
        };
        let text = self.active_buffer().slice_range(start, end);
        self.kill_ring.push(text);
        true
    }

    fn kill_active_region(&mut self) -> bool {
        let Some((start, end)) = self.active_region_range() else {
            return false;
        };
        let text = self.active_buffer().slice_range(start, end);
        self.kill_ring.push(text);
        self.active_buffer_mut().delete_range(start, end);
        self.clear_mark();
        true
    }
}

fn normalize_region(
    start: (usize, usize),
    end: (usize, usize),
) -> ((usize, usize), (usize, usize)) {
    if start <= end { (start, end) } else { (end, start) }
}

impl CompletionState {
    const VISIBLE_ROWS: usize = 8;

    fn ensure_visible(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + Self::VISIBLE_ROWS {
            self.scroll = self.selected + 1 - Self::VISIBLE_ROWS;
        }
    }
}

fn format_value_for_minibuffer(value: &Value) -> String {
    let mut s = format_lisp_value(value);
    if s.len() > 240 {
        s.truncate(237);
        s.push_str("...");
    }
    s
}

fn register_editor_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "bind-key",
        "(bind-key key handler)",
        "Bind a key chord string to a Lisp function.",
        |args, ctx| {
            let (Some(Value::String(key)), Some(Value::String(handler))) =
                (args.first(), args.get(1))
            else {
                return Err("bind-key expects (string string)".to_string());
            };
            ctx.bind_key(key.clone(), handler.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "status",
        "(status message)",
        "Show a message in the minibuffer.",
        |args, ctx| {
            let Some(Value::String(message)) = args.first() else {
                return Err("status expects a string".to_string());
            };
            ctx.set_status(message.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "s-expression-at-cursor",
        "(s-expression-at-cursor)",
        "Return the current s-expression as a string.",
        |_args, ctx| {
            Ok(ctx
                .current_sexp()
                .map(Value::String)
                .unwrap_or(Value::String(String::new())))
        },
    );

    runtime.register_native_with_docs(
        "current-buffer-text",
        "(current-buffer-text)",
        "Return the active buffer contents.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_text())),
    );

    runtime.register_native_with_docs(
        "current-buffer-name",
        "(current-buffer-name)",
        "Return the active buffer name.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_name())),
    );

    runtime.register_native_with_docs(
        "current-buffer-path",
        "(current-buffer-path)",
        "Return the active buffer path or false.",
        |_args, ctx| {
            Ok(match ctx.current_buffer_path() {
                Some(path) => Value::String(path.display().to_string()),
                None => Value::Bool(false),
            })
        },
    );

    runtime.register_native_with_docs(
        "host-command",
        "(host-command name payload)",
        "Send a command to the host application.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("host-command expects a command name".to_string());
            };
            let payload = args.get(1).cloned().unwrap_or(Value::Bool(true));
            let buffer_id = ctx.current_buffer_id();
            let path = ctx.current_buffer_path();
            let source = ctx.current_buffer_text();

            match name.as_str() {
                "compile-instrument" => {
                    ctx.enqueue_command(HostCommand::CompileInstrument {
                        source,
                        suggested_name: extract_suggested_name(&payload),
                        buffer_id: buffer_id.unwrap_or(0),
                        path,
                    });
                }
                "compile-effect" => {
                    ctx.enqueue_command(HostCommand::CompileEffect {
                        source,
                        suggested_name: extract_suggested_name(&payload),
                        buffer_id: buffer_id.unwrap_or(0),
                        path,
                    });
                }
                _ => {
                    ctx.enqueue_command(HostCommand::Custom {
                        name: name.clone(),
                        payload,
                    });
                }
            }
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "save-buffer",
        "(save-buffer)",
        "Save the current buffer.",
        |_args, ctx| {
            ctx.request_save();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "save-buffer-as",
        "(save-buffer-as path)",
        "Save the current buffer to a new path.",
        |args, ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("save-buffer-as expects a path string".to_string());
            };
            ctx.request_save_as(path.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "eval-selection-or-sexp",
        "(eval-selection-or-sexp)",
        "Return the selected form or current s-expression as source.",
        |_args, ctx| {
            Ok(ctx
                .current_sexp()
                .map(Value::String)
                .unwrap_or(Value::Bool(false)))
        },
    );

    runtime.register_native_with_docs(
        "eval-buffer",
        "(eval-buffer)",
        "Return the whole buffer as source for evaluation.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_text())),
    );
}

fn extract_suggested_name(payload: &Value) -> Option<String> {
    let Value::Map(map) = payload else {
        return None;
    };
    let value = map.get("suggested-name").or_else(|| map.get("name"))?;
    match &*value.borrow() {
        Value::String(name) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

fn key_str(key: KeyEvent) -> String {
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

#[cfg(test)]
mod tests {
    use super::{Editor, EditorConfig, key_str};
    use crate::host::HostCommand;
    use crate::mode::BufferMode;
    use crate::runtime::Runtime;
    use crate::vm::Value;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    #[test]
    fn ctrl_char_keys_are_normalized_to_lowercase() {
        let key = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::CONTROL);
        assert_eq!(key_str(key), "C-c");
    }

    #[test]
    fn ctrl_c_ctrl_c_binding_enqueues_host_command() {
        let init = r#"
            (def compile-current ()
              (host-command "compile-current" (dict :source (current-buffer-text))))
            (bind-key "C-c C-c" "compile-current")
        "#;
        let runtime = Runtime::with_init_source(init);
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init.to_string()),
            },
        );
        editor.open_scratch_buffer("*test*", "(+ 1 2)");

        editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            HostCommand::Custom { name, .. } if name == "compile-current"
        ));
    }

    #[test]
    fn preloaded_runtime_bindings_are_visible_to_editor() {
        let init = r#"
            (def compile-current ()
              (host-command "compile-current" (dict :source (current-buffer-text))))
            (bind-key "C-c C-c" "compile-current")
        "#;
        let runtime = Runtime::with_init_source(init);
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(+ 1 2)");

        editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

        let commands = editor.drain_host_commands();
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            HostCommand::Custom { name, .. } if name == "compile-current"
        ));
    }

    #[test]
    fn ctrl_a_moves_to_start_of_line() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abcdef");
        editor.active_buffer_mut().cursor = (0, 4);

        editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().cursor, (0, 0));
    }

    #[test]
    fn ctrl_e_moves_to_end_of_line() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abcdef");
        editor.active_buffer_mut().cursor = (0, 1);

        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().cursor, (0, 6));
    }

    #[test]
    fn ctrl_left_moves_to_previous_word() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 10);

        editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().cursor, (0, 8));
    }

    #[test]
    fn ctrl_right_moves_to_next_word() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 0);

        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().cursor, (0, 3));
    }

    #[test]
    fn ctrl_w_deletes_previous_word() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 8);

        editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().text(), "abc ghi");
        assert_eq!(editor.active_buffer().cursor, (0, 4));
    }

    #[test]
    fn ctrl_w_kills_active_region() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 0);

        editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().text(), " def ghi");
        assert_eq!(editor.active_buffer().cursor, (0, 0));
        assert!(editor.active_region_range().is_none());
    }

    #[test]
    fn alt_w_copies_region_and_ctrl_y_yanks_it() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def");
        editor.active_buffer_mut().cursor = (0, 0);

        editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT));
        editor.active_buffer_mut().cursor = (0, 7);
        editor.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().text(), "abc defabc");
    }

    #[test]
    fn typing_clears_active_mark() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc");
        editor.active_buffer_mut().cursor = (0, 0);

        editor.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(editor.active_region_range().is_some());

        editor.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
        assert!(editor.active_region_range().is_none());
    }

    #[test]
    fn ctrl_k_deletes_rest_of_line() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 4);

        editor.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));

        assert_eq!(editor.active_buffer().text(), "abc ");
        assert_eq!(editor.active_buffer().cursor, (0, 4));
    }

    #[test]
    fn tab_accepts_completion_from_runtime_symbols() {
        let mut runtime = Runtime::new();
        runtime.register_native("seq-step", |_args, _ctx| Ok(Value::Bool(true)));
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(seq");
        editor.active_buffer_mut().cursor = (0, 4);

        editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));

        editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(editor.active_buffer().text(), "(seq-step");
    }

    #[test]
    fn tab_indents_current_line_when_no_completion_matches() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(if test\n:4t)");
        editor.active_buffer_mut().cursor = (1, 0);

        editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(editor.active_buffer().text(), "(if test\n    :4t)");
        assert_eq!(editor.active_buffer().cursor, (1, 4));
    }

    #[test]
    fn enter_inserts_lisp_indentation() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(if test)");
        editor.active_buffer_mut().cursor = (0, 8);

        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(editor.active_buffer().text(), "(if test\n    )");
        assert_eq!(editor.active_buffer().cursor, (1, 4));
    }

    #[test]
    fn scratch_mode_defaults_to_eseqlisp() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer_with_mode("*dsp*", "(param freq 440)", BufferMode::DGenLisp);

        assert_eq!(editor.active_buffer().mode, BufferMode::DGenLisp);
    }

    #[test]
    fn cursor_movement_closes_completion_popup() {
        let mut runtime = Runtime::new();
        runtime.register_native("seq-step", |_args, _ctx| Ok(Value::Bool(true)));
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(seq");
        editor.active_buffer_mut().cursor = (0, 4);

        editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        assert!(editor.completion_state().is_some());

        editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(editor.completion_state().is_none());
    }

    #[test]
    fn completion_scrolls_to_keep_selection_visible() {
        let mut runtime = Runtime::new();
        for name in [
            "seq-a", "seq-b", "seq-c", "seq-d", "seq-e", "seq-f", "seq-g", "seq-h", "seq-i",
        ] {
            runtime.register_native(name, |_args, _ctx| Ok(Value::Bool(true)));
        }
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(seq");
        editor.active_buffer_mut().cursor = (0, 4);

        editor.handle_key(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));
        for _ in 0..8 {
            editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }

        let completion = editor.completion_state().unwrap();
        assert_eq!(completion.selected, 8);
        assert_eq!(completion.scroll, 1);
    }

    #[test]
    fn tab_accepts_dotted_completion_from_runtime_maps() {
        let mut runtime = Runtime::new();
        let mut fields = HashMap::new();
        fields.insert(
            "feedback".to_string(),
            Rc::new(RefCell::new(Value::Number(0.0))),
        );
        runtime.set_global_value("MODUM_DELAY", Value::Map(fields));
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(MODUM_DELAY.");
        editor.active_buffer_mut().cursor = (0, "(MODUM_DELAY.".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));

        editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(editor.active_buffer().text(), "(MODUM_DELAY.feedback");
    }

    #[test]
    fn map_results_are_shown_in_minibuffer() {
        let init = r#"
            (def eval-sexp ()
              (eval (s-expression-at-cursor)))
            (bind-key "C-x C-e" "eval-sexp")
        "#;
        let mut runtime = Runtime::new();
        runtime.register_native("return-map", |_args, _ctx| {
            let mut map = HashMap::new();
            map.insert(
                "step".to_string(),
                Rc::new(RefCell::new(Value::Number(1.0))),
            );
            Ok(Value::Map(map))
        });
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init.to_string()),
            },
        );
        editor.open_scratch_buffer("*test*", "(return-map)");
        editor.active_buffer_mut().cursor = (0, "(return-map)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        let minibuffer = editor.minibuffer.unwrap_or_default();
        assert!(minibuffer.contains("step"));
    }

    #[test]
    fn eval_updates_after_buffer_contents_change() {
        let init = r#"
            (def eval-sexp ()
              (eval (s-expression-at-cursor)))
            (bind-key "C-x C-e" "eval-sexp")
        "#;
        let runtime = Runtime::new();
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init.to_string()),
            },
        );
        editor.open_scratch_buffer("*test*", "(+ 5 10)");
        editor.active_buffer_mut().cursor = (0, "(+ 5 10)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(editor.minibuffer.as_deref(), Some("15"));

        editor.active_buffer_mut().set_text("(+ 100 100)");
        editor.active_buffer_mut().cursor = (0, "(+ 100 100)".len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(editor.minibuffer.as_deref(), Some("200"));
    }

    #[test]
    fn movement_clears_minibuffer_message() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc");
        editor.minibuffer = Some("15".to_string());
        editor.active_buffer_mut().cursor = (0, 1);

        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert_eq!(editor.minibuffer, None);
    }
}
