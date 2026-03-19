use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::buffer::Buffer;
use crate::host::{BufferId, CompileKind, HostCommand, HostEvent};
use crate::mode::{
    BufferMode, CompletionItem, CompletionMatch, TokenSpan, completion_match, highlight_line,
};
use crate::runtime::Runtime;
use crate::text::{innermost_sexp_range_at_cursor, sexp_at_cursor};
use crate::vm::{Value, format_lisp_value};
use crate::widget_render::{self, MouseEventOutcome, handle_event, map_mouse_event};

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
enum MinibufferMode {
    Mx {
        input: String,
        candidates: Vec<String>,
        selected: usize,
    },
    SwitchBuffer {
        input: String,
        candidates: Vec<String>,
        selected: usize,
    },
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

#[derive(Debug, Clone)]
struct HighlightCache {
    buffer_id: BufferId,
    buffer_revision: u64,
    buffer_mode: BufferMode,
    runtime_symbol_revision: u64,
    spans: Rc<Vec<Vec<TokenSpan>>>,
}

#[derive(Debug, Clone)]
struct WidgetHitCache {
    layout_revision: u64,
    scroll_top: u16,
    cols: u16,
    rows: u16,
    cells: Vec<Option<crate::layout::LayoutNode>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub buffer_id: BufferId,
    pub cursor: (usize, usize),
}

#[derive(Debug, Clone)]
pub struct MajorMode {
    pub name: String,
    pub read_only: bool,
    pub keybindings: HashMap<String, String>,
    pub on_enter: Option<String>,
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
    highlight_cache: Option<HighlightCache>,
    widget_hit_cache: Option<WidgetHitCache>,
    last_mouse_precise: Option<(f32, f32)>,
    eval_flash: Option<SExpFlash>,
    mark: Option<Mark>,
    kill_ring: Vec<String>,
    minibuffer_input: Option<MinibufferMode>,
    mode_registry: HashMap<String, MajorMode>,
    focused_widget_id: Option<u64>,
    widget_scroll_top: u16,
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
            highlight_cache: None,
            widget_hit_cache: None,
            last_mouse_precise: None,
            eval_flash: None,
            mark: None,
            kill_ring: vec![],
            minibuffer_input: None,
            mode_registry: HashMap::new(),
            focused_widget_id: None,
            widget_scroll_top: 0,
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

    pub fn active_highlight_spans(&mut self) -> Rc<Vec<Vec<TokenSpan>>> {
        let buffer = self.active_buffer();
        let buffer_id = buffer.id;
        let buffer_revision = buffer.revision;
        let buffer_mode = buffer.mode.clone();
        let runtime_symbol_revision = self.runtime.symbol_revision();

        let is_fresh = self.highlight_cache.as_ref().is_some_and(|cache| {
            cache.buffer_id == buffer_id
                && cache.buffer_revision == buffer_revision
                && cache.buffer_mode == buffer_mode
                && cache.runtime_symbol_revision == runtime_symbol_revision
        });

        if !is_fresh {
            let symbols = self.runtime.completion_symbols();
            let buffer = self.active_buffer();
            let spans = buffer
                .lines
                .iter()
                .map(|line| highlight_line(&buffer.mode, line, &symbols, buffer))
                .collect();
            self.highlight_cache = Some(HighlightCache {
                buffer_id,
                buffer_revision,
                buffer_mode,
                runtime_symbol_revision,
                spans: Rc::new(spans),
            });
        }

        Rc::clone(
            &self
                .highlight_cache
                .as_ref()
                .expect("highlight cache")
                .spans,
        )
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

    pub fn widget_scroll_top(&self) -> u16 {
        if self.has_focusable_widgets() {
            self.widget_scroll_top
        } else {
            self.active_buffer().scroll_top as u16
        }
    }

    pub fn focused_widget_id(&self) -> Option<u64> {
        self.focused_widget_id
    }

    pub fn widget_layout(&self) -> Option<Arc<crate::layout::LayoutNode>> {
        self.runtime.current_layout.clone()
    }

    pub fn widget_layout_revision(&self) -> u64 {
        self.runtime.layout_revision()
    }

    pub fn take_dirty_widget_ids(&mut self) -> Vec<u64> {
        self.runtime.take_dirty_widget_ids()
    }

    pub fn set_layout_viewport(&mut self, cols: u16, rows: u16) {
        self.runtime.set_layout_viewport(cols, rows);
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
        self.save_current_widget_tree();
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::from_text(id, name, initial);
        buffer.set_mode(mode);
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.mark_needs_redraw();
        self.sync_runtime_context();
        self.refresh_completion();
        self.clear_widget_focus();
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
        self.save_current_widget_tree();
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::from_file(id, path)?;
        buffer.set_mode(mode);
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.mark_needs_redraw();
        self.sync_runtime_context();
        self.refresh_completion();
        self.clear_widget_focus();
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
            self.save_current_widget_tree();
            self.active = index;
            self.mark_needs_redraw();
            self.sync_runtime_context();
            self.completion = None;
            self.clear_mark();
            self.restore_buffer_widget_tree();
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
                    Event::Mouse(mouse) => {
                        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                        self.handle_mouse(
                            mouse,
                            1,
                            1,
                            cols.saturating_sub(2),
                            rows.saturating_sub(3),
                        );
                    }
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

        if self.handle_minibuffer_key(key) {
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

        if self.handle_focus_key(key) {
            return;
        }

        // Check mode-specific keybindings
        if let BufferMode::Named(ref mode_name) = self.active_buffer().mode {
            if let Some(handler) = self
                .mode_registry
                .get(mode_name)
                .and_then(|mode| mode.keybindings.get(&key_str(key)))
                .cloned()
            {
                self.call_lisp_handler(&handler);
                return;
            }
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
                if self.guard_read_only() {
                    return;
                }
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().insert_char(c);
                self.sync_runtime_context();
                self.refresh_completion();
            }
            KeyCode::Enter => {
                if self.guard_read_only() {
                    return;
                }
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().insert_newline_with_indent();
                self.sync_runtime_context();
            }
            _ => {}
        }
    }

    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
    ) {
        self.handle_mouse_precise(
            mouse,
            content_col,
            content_row,
            content_width,
            content_height,
            mouse.column as f32,
            mouse.row as f32,
        );
    }

    pub fn handle_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.last_mouse_precise = Some((precise_col, precise_row));
                // Try click-to-activate on focusable widgets first
                if self.try_click_focusable_widget(
                    mouse, content_col, content_row,
                ) {
                    return;
                }
                if self.try_handle_widget_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                ) {
                    return;
                }
                self.handle_text_click(
                    mouse,
                    content_col,
                    content_row,
                    content_width,
                    content_height,
                );
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let previous = self
                    .last_mouse_precise
                    .unwrap_or((precise_col, precise_row));
                self.try_handle_widget_drag_segment(
                    mouse,
                    content_col,
                    content_row,
                    previous,
                    (precise_col, precise_row),
                );
                self.last_mouse_precise = Some((precise_col, precise_row));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.last_mouse_precise = None;
            }
            MouseEventKind::ScrollUp => {
                if self.active_buffer().read_only && self.has_focusable_widgets() {
                    self.navigate_focus(KeyCode::Up);
                } else {
                    let buffer = self.active_buffer_mut();
                    if buffer.scroll_top > 0 {
                        buffer.scroll_top = buffer.scroll_top.saturating_sub(3);
                        buffer.cursor.0 = buffer.cursor.0.min(
                            buffer.scroll_top + content_height.saturating_sub(1) as usize,
                        );
                    }
                    self.mark_needs_redraw();
                }
            }
            MouseEventKind::ScrollDown => {
                if self.active_buffer().read_only && self.has_focusable_widgets() {
                    self.navigate_focus(KeyCode::Down);
                } else {
                    let buffer = self.active_buffer_mut();
                    let max_scroll = buffer.lines.len().saturating_sub(1);
                    buffer.scroll_top = (buffer.scroll_top + 3).min(max_scroll);
                    if buffer.cursor.0 < buffer.scroll_top {
                        buffer.cursor.0 = buffer.scroll_top;
                    }
                    self.mark_needs_redraw();
                }
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
                if self.guard_read_only() {
                    return;
                }
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().delete_char_before();
                self.refresh_completion();
            }
            "kill-region-or-word" => {
                if self.guard_read_only() {
                    return;
                }
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
                if self.guard_read_only() {
                    return;
                }
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
                if self.guard_read_only() {
                    return;
                }
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
            "execute-extended-command" => {
                self.completion = None;
                self.minibuffer = None;
                let candidates = self.collect_mx_candidates();
                self.minibuffer_input = Some(MinibufferMode::Mx {
                    input: String::new(),
                    candidates,
                    selected: 0,
                });
            }
            "switch-to-buffer" => {
                self.completion = None;
                self.minibuffer = None;
                let candidates: Vec<String> =
                    self.buffers.iter().map(|b| b.name.clone()).collect();
                self.minibuffer_input = Some(MinibufferMode::SwitchBuffer {
                    input: String::new(),
                    candidates,
                    selected: 0,
                });
            }
            _ => {}
        }
        self.sync_runtime_context();
    }

    fn call_lisp_handler(&mut self, fn_name: &str) {
        if fn_name == "eval-sexp" || fn_name == "eval-buffer-command" {
            self.eval_preview_handler(fn_name);
            return;
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

    fn eval_preview_handler(&mut self, fn_name: &str) {
        if fn_name == "eval-sexp" {
            self.start_eval_flash();
        }
        self.sync_runtime_context();
        self.minibuffer = None;

        let source = match fn_name {
            "eval-sexp" => {
                let buffer = self.active_buffer();
                sexp_at_cursor(&buffer.lines, buffer.cursor).unwrap_or_default()
            }
            "eval-buffer-command" => self.active_buffer().text(),
            _ => String::new(),
        };

        if source.trim().is_empty() {
            self.minibuffer = Some("No s-expression at cursor".to_string());
            self.completion = None;
            return;
        }

        match self.runtime.eval_str(&source) {
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

    fn load_active_buffer(&mut self) -> Result<PathBuf, EditorError> {
        let active = self.active_buffer();
        let path = active
            .path
            .clone()
            .ok_or_else(|| EditorError::Message("buffer is not file-backed".to_string()))?;
        let mode = active.mode.clone();
        let buffer = self.active_buffer_mut();
        let text = std::fs::read_to_string(&path)?;
        buffer.set_text(&text);
        buffer.set_path(path.clone());
        buffer.set_mode(mode);
        buffer.dirty = false;
        Ok(path)
    }

    fn sync_runtime_context(&mut self) {
        let active = self.active_buffer();
        let buffer_names: Vec<String> = self.buffers.iter().map(|b| b.name.clone()).collect();
        let mut shared = self.runtime.shared.borrow_mut();
        shared.current_buffer_id = Some(active.id);
        shared.current_buffer_name = active.name.clone();
        shared.current_buffer_path = active.path.clone();
        shared.current_buffer_text = active.text();
        shared.current_sexp = sexp_at_cursor(&active.lines, active.cursor);
        shared.current_buffer_read_only = active.read_only;
        shared.current_buffer_mode = active.mode.name().to_string();
        shared.current_line_number = active.cursor.0 + 1;
        shared.current_line_text = active
            .lines
            .get(active.cursor.0)
            .cloned()
            .unwrap_or_default();
        shared.buffer_names = buffer_names;
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
            &self.active_buffer().mode,
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

    fn try_click_focusable_widget(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
    ) -> bool {
        if !self.has_focusable_widgets() {
            return false;
        }
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        if mouse.column < content_col || mouse.row < content_row {
            return false;
        }
        let local_row = (mouse.row - content_row) + self.widget_scroll_top;
        let local_col = mouse.column - content_col;

        // Find the focusable widget at this position
        let mut focusable_nodes: Vec<(u64, u16, u16, u16, u16)> = Vec::new();
        collect_focusable_nodes(&layout, &mut focusable_nodes);

        for (id, row, col, width, height) in &focusable_nodes {
            if local_row >= *row
                && local_row < row + height
                && local_col >= *col
                && local_col < col + width
            {
                self.focused_widget_id = Some(*id);
                self.adjust_widget_scroll(*row);
                self.mark_needs_redraw();
                self.activate_focused();
                return true;
            }
        }
        false
    }

    fn has_focusable_widgets(&self) -> bool {
        self.runtime
            .current_layout
            .as_ref()
            .map(|layout| has_focusable_node(layout))
            .unwrap_or(false)
    }

    fn handle_focus_key(&mut self, key: KeyEvent) -> bool {
        if !self.active_buffer().read_only || !self.has_focusable_widgets() {
            return false;
        }

        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                self.navigate_focus(key.code);
                true
            }
            KeyCode::Enter => {
                self.activate_focused();
                true
            }
            _ => false,
        }
    }

    fn navigate_focus(&mut self, direction: KeyCode) {
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        let mut focusable_nodes: Vec<(u64, u16, u16, u16, u16)> = Vec::new();
        collect_focusable_nodes(&layout, &mut focusable_nodes);
        if focusable_nodes.is_empty() {
            return;
        }

        // Auto-focus first widget if nothing is focused
        if self.focused_widget_id.is_none() {
            self.focused_widget_id = Some(focusable_nodes[0].0);
            self.mark_needs_redraw();
            return;
        }

        let current_id = self.focused_widget_id.unwrap();
        let current_idx = focusable_nodes
            .iter()
            .position(|(id, _, _, _, _)| *id == current_id)
            .unwrap_or(0);

        let next_idx = match direction {
            KeyCode::Down | KeyCode::Right => {
                if current_idx + 1 < focusable_nodes.len() {
                    current_idx + 1
                } else {
                    0
                }
            }
            KeyCode::Up | KeyCode::Left => {
                if current_idx > 0 {
                    current_idx - 1
                } else {
                    focusable_nodes.len() - 1
                }
            }
            _ => current_idx,
        };

        self.focused_widget_id = Some(focusable_nodes[next_idx].0);
        self.adjust_widget_scroll(focusable_nodes[next_idx].1);
        self.mark_needs_redraw();
    }

    fn adjust_widget_scroll(&mut self, focused_row: u16) {
        let viewport_height = self.runtime.layout_rows();
        if viewport_height == 0 {
            return;
        }
        // Scroll up if focused row is above viewport
        if focused_row < self.widget_scroll_top {
            self.widget_scroll_top = focused_row;
        }
        // Scroll down if focused row is below viewport
        if focused_row >= self.widget_scroll_top + viewport_height {
            self.widget_scroll_top = focused_row - viewport_height + 1;
        }
    }

    fn activate_focused(&mut self) {
        let Some(focused_id) = self.focused_widget_id else {
            return;
        };
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        let Some(node) = find_node_by_id(&layout, focused_id) else {
            return;
        };
        // Look for :on-enter callback in props
        if let Some(callback) = node.props.get("on-enter").cloned() {
            let result = self.runtime.invoke(callback, vec![]);
            if let Some(status) = self.runtime.take_status_message() {
                self.minibuffer = Some(status);
            } else if let Err(error) = result {
                self.minibuffer = Some(format!("Error: {error:?}"));
            }
            self.refresh_runtime_side_effects();
            self.sync_runtime_context();
            self.mark_needs_redraw();
        }
    }

    fn save_current_widget_tree(&mut self) {
        if let Some(tree) = self.runtime.current_widget_tree() {
            self.active_buffer_mut().widget_tree = Some(tree);
        }
    }

    /// Clear widget layout and focus state for a buffer with no widget tree.
    fn clear_widget_focus(&mut self) {
        self.runtime.current_layout = None;
        self.focused_widget_id = None;
        self.widget_scroll_top = 0;
    }

    fn restore_buffer_widget_tree(&mut self) {
        let tree = self.active_buffer().widget_tree.clone();
        match tree {
            Some(tree) => {
                self.runtime.restore_widget_tree(tree);
                self.auto_focus_first_widget();
            }
            None => {
                self.clear_widget_focus();
            }
        }
    }

    fn auto_focus_first_widget(&mut self) {
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        let mut focusable_nodes: Vec<(u64, u16, u16, u16, u16)> = Vec::new();
        collect_focusable_nodes(&layout, &mut focusable_nodes);
        if let Some((id, _, _, _, _)) = focusable_nodes.first() {
            self.focused_widget_id = Some(*id);
        }
    }

    fn guard_read_only(&mut self) -> bool {
        if self.active_buffer().read_only {
            self.minibuffer = Some("Buffer is read-only".to_string());
            true
        } else {
            false
        }
    }

    fn handle_minibuffer_key(&mut self, key: KeyEvent) -> bool {
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
                        self.active = idx;
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

    fn execute_mx_command(&mut self, name: &str) {
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

    fn collect_mx_candidates(&mut self) -> Vec<String> {
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

    fn refresh_runtime_side_effects(&mut self) {
        self.lisp_bindings = self.runtime.lisp_bindings();

        if let Some(read_only) = self.runtime.take_pending_set_read_only() {
            self.active_buffer_mut().read_only = read_only;
        }

        // Process mode definitions
        for (name, read_only, on_enter) in self.runtime.take_pending_mode_defs() {
            self.mode_registry.insert(
                name.clone(),
                MajorMode {
                    name,
                    read_only,
                    keybindings: HashMap::new(),
                    on_enter,
                },
            );
        }

        // Process mode keybindings
        for (mode_name, key, handler) in self.runtime.take_pending_mode_bindings() {
            if let Some(mode) = self.mode_registry.get_mut(&mode_name) {
                mode.keybindings.insert(key, handler);
            }
        }

        // Process buffer operations first (create/switch must happen before set-mode)
        if let Some(path) = self.runtime.take_pending_open_file() {
            match self.open_file_buffer(&path) {
                Ok(_) => {
                    self.minibuffer = Some(format!("Opened {path}"));
                    self.clear_widget_focus();
                }
                Err(e) => self.minibuffer = Some(format!("Error: {e:?}")),
            }
        }

        if let Some(name) = self.runtime.take_pending_create_buffer() {
            self.open_scratch_buffer(&name, "");
        }

        if let Some(name) = self.runtime.take_pending_switch_buffer() {
            if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
                self.save_current_widget_tree();
                self.active = idx;
                self.mark_needs_redraw();
                self.sync_runtime_context();
                self.completion = None;
                self.clear_mark();
                self.restore_buffer_widget_tree();
            }
        }

        // Process set-buffer-mode (after buffer creation so it targets the new buffer)
        if let Some(mode_name) = self.runtime.take_pending_set_mode() {
            let mode_def = self.mode_registry.get(&mode_name).cloned();
            let buffer = self.active_buffer_mut();
            buffer.mode = BufferMode::Named(mode_name.clone());
            if let Some(mode_def) = &mode_def {
                buffer.read_only = mode_def.read_only;
            }
            // Call on_enter hook
            if let Some(on_enter) = mode_def.as_ref().and_then(|m| m.on_enter.clone()) {
                self.sync_runtime_context();
                let code = format!("({on_enter})");
                let _ = self.runtime.eval_str(&code);
                if let Some(status) = self.runtime.take_status_message() {
                    self.minibuffer = Some(status);
                }
            }
            // Auto-focus first focusable widget if mode has them
            self.auto_focus_first_widget();
        }

        if let Some(text) = self.runtime.take_pending_set_text() {
            self.active_buffer_mut().set_text(&text);
        }

        if let Some(lines) = self.runtime.take_pending_set_lines() {
            let buffer = self.active_buffer_mut();
            buffer.lines = if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            };
            buffer.cursor = (0, 0);
            buffer.scroll_top = 0;
            buffer.revision = buffer.revision.wrapping_add(1);
        }

        if let Some(line) = self.runtime.take_pending_goto_line() {
            let buffer = self.active_buffer_mut();
            let row = line.saturating_sub(1).min(buffer.lines.len().saturating_sub(1));
            buffer.cursor = (row, 0);
        }

        // Process widget tree rendering (stored per-buffer)
        if let Some(tree) = self.runtime.take_pending_widget_tree() {
            match tree {
                Value::Nil | Value::Bool(false) => {
                    self.active_buffer_mut().widget_tree = None;
                    self.runtime.clear_layout_effects();
                    self.focused_widget_id = None;
                }
                tree => {
                    self.active_buffer_mut().widget_tree = Some(tree.clone());
                    self.runtime.set_widget_tree(tree);
                    self.auto_focus_first_widget();
                }
            }
        }

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
        } else if self.runtime.take_pending_load() {
            match self.load_active_buffer() {
                Ok(path) => self.minibuffer = Some(format!("Loaded {}", path.display())),
                Err(error) => self.minibuffer = Some(format!("Error: {error:?}")),
            }
        }
        self.completion = None;
    }

    fn try_handle_widget_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        if mouse.column < content_col || mouse.row < content_row {
            return false;
        }

        let local_col = precise_col - content_col as f32;
        let local_row = precise_row - content_row as f32;
        let output = {
            let query_row = local_row.floor().max(0.0) as u16;
            let query_col = local_col.floor().max(0.0) as u16;
            let Some(node) = self.widget_node_at(query_row, query_col) else {
                return false;
            };
            self.dispatch_widget_mouse_event(&node, mouse.kind, content_col, content_row, precise_col, precise_row)
        };

        self.apply_widget_output(output)
    }

    fn try_handle_widget_drag_segment(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        start: (f32, f32),
        end: (f32, f32),
    ) {
        let start_local = (start.0 - content_col as f32, start.1 - content_row as f32);
        let end_local = (end.0 - content_col as f32, end.1 - content_row as f32);
        let start_node = self.widget_node_at(
            start_local.1.floor().max(0.0) as u16,
            start_local.0.floor().max(0.0) as u16,
        );
        let end_node = self.widget_node_at(
            end_local.1.floor().max(0.0) as u16,
            end_local.0.floor().max(0.0) as u16,
        );

        if let Some(node) = start_node.as_ref()
            && widget_render::widget_captures_drag(&node.widget_type)
        {
            let scroll = self.widget_scroll_top() as f32;
            let screen_row = node.rect.row as f32 - scroll;
            let clamped_col = end
                .0
                .clamp(
                    content_col as f32 + node.rect.col as f32,
                    content_col as f32
                        + node.rect.col as f32
                        + node.rect.width.saturating_sub(1) as f32,
                );
            let clamped_row = end
                .1
                .clamp(
                    content_row as f32 + screen_row,
                    content_row as f32
                        + screen_row
                        + node.rect.height.saturating_sub(1) as f32,
                );
            let output = self.dispatch_widget_mouse_event(
                node,
                mouse.kind,
                content_col,
                content_row,
                clamped_col,
                clamped_row,
            );
            let _ = self.apply_widget_output(output);
            return;
        }

        if same_widget_hit(start_node.as_ref(), end_node.as_ref()) {
            let _ =
                self.try_handle_widget_mouse_precise(mouse, content_col, content_row, end.0, end.1);
            return;
        }

        let steps = ((end.0 - start.0).abs().max((end.1 - start.1).abs()) * 2.0)
            .ceil()
            .max(1.0) as usize;
        let mut last_key = None;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let col = start.0 + (end.0 - start.0) * t;
            let row = start.1 + (end.1 - start.1) * t;
            let local_col = col - content_col as f32;
            let local_row = row - content_row as f32;
            let node = self.widget_node_at(
                local_row.floor().max(0.0) as u16,
                local_col.floor().max(0.0) as u16,
            );
            let key = node.as_ref().map(widget_hit_key);
            if key.is_some() && key != last_key {
                let _ =
                    self.try_handle_widget_mouse_precise(mouse, content_col, content_row, col, row);
            }
            last_key = key;
        }
    }

    fn widget_node_at(&mut self, row: u16, col: u16) -> Option<crate::layout::LayoutNode> {
        let revision = self.runtime.layout_revision();
        let scroll = self.widget_scroll_top();
        let layout = self.runtime.current_layout.as_ref()?;
        let cols = layout.rect.col.saturating_add(layout.rect.width);
        let rows = layout.rect.row.saturating_add(layout.rect.height);

        let needs_rebuild = self.widget_hit_cache.as_ref().is_none_or(|cache| {
            cache.layout_revision != revision
                || cache.scroll_top != scroll
                || cache.cols != cols
                || cache.rows != rows
        });
        if needs_rebuild {
            let mut cells = vec![None; cols as usize * rows as usize];
            fill_widget_hit_cells(layout, cols, rows, &mut cells);
            self.widget_hit_cache = Some(WidgetHitCache {
                layout_revision: revision,
                scroll_top: scroll,
                cols,
                rows,
                cells,
            });
        }

        // Offset the query row by scroll to map screen position to layout position
        let layout_row = row + scroll;
        let cache = self.widget_hit_cache.as_ref()?;
        if layout_row >= cache.rows || col >= cache.cols {
            return None;
        }
        cache.cells[layout_row as usize * cache.cols as usize + col as usize].clone()
    }

    fn handle_text_click(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
    ) {
        if mouse.column < content_col || mouse.row < content_row {
            return;
        }
        let local_col = mouse.column - content_col;
        let local_row = mouse.row - content_row;
        if local_col >= content_width || local_row >= content_height {
            return;
        }

        let buffer = self.active_buffer_mut();
        let absolute_row = buffer
            .scroll_top
            .saturating_add(local_row as usize)
            .min(buffer.lines.len().saturating_sub(1));
        let absolute_col = (local_col as usize).min(buffer.lines[absolute_row].len());
        buffer.cursor = (absolute_row, absolute_col);
        self.completion = None;
        self.minibuffer = None;
        self.clear_mark();
        self.sync_runtime_context();
        self.mark_needs_redraw();
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

    fn dispatch_widget_mouse_event(
        &self,
        node: &crate::layout::LayoutNode,
        mouse_kind: MouseEventKind,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<crate::widget_render::EventOutput> {
        let local_col = precise_col - content_col as f32;
        let local_row = precise_row - content_row as f32 + self.widget_scroll_top() as f32;
        match map_mouse_event(node, mouse_kind, local_col, local_row) {
            MouseEventOutcome::Ignore | MouseEventOutcome::Consume => None,
            MouseEventOutcome::Dispatch(widget_event) => handle_event(node, widget_event),
        }
    }

    fn apply_widget_output(&mut self, output: Option<crate::widget_render::EventOutput>) -> bool {
        let Some(output) = output else {
            return true;
        };
        let result = self.runtime.invoke(output.callback, vec![output.value]);
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        } else if let Err(error) = result {
            self.minibuffer = Some(format!("Error: {error:?}"));
        } else {
            self.minibuffer = None;
        }
        self.refresh_runtime_side_effects();
        self.completion = None;
        self.mark_needs_redraw();
        true
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

fn has_focusable_node(node: &crate::layout::LayoutNode) -> bool {
    if node.focusable {
        return true;
    }
    node.children.iter().any(has_focusable_node)
}

fn collect_focusable_nodes(
    node: &crate::layout::LayoutNode,
    result: &mut Vec<(u64, u16, u16, u16, u16)>,
) {
    if node.focusable {
        result.push((
            node.widget_id,
            node.rect.row,
            node.rect.col,
            node.rect.width,
            node.rect.height,
        ));
    }
    for child in &node.children {
        collect_focusable_nodes(child, result);
    }
}

fn find_node_by_id(
    node: &crate::layout::LayoutNode,
    id: u64,
) -> Option<crate::layout::LayoutNode> {
    if node.widget_id == id {
        return Some(node.clone());
    }
    for child in &node.children {
        if let Some(found) = find_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn filter_candidates(candidates: &[String], input: &str) -> Vec<String> {
    if input.is_empty() {
        return candidates.to_vec();
    }
    let input_lower = input.to_ascii_lowercase();
    candidates
        .iter()
        .filter(|c| c.to_ascii_lowercase().contains(&input_lower))
        .cloned()
        .collect()
}

fn normalize_region(
    start: (usize, usize),
    end: (usize, usize),
) -> ((usize, usize), (usize, usize)) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

fn fill_widget_hit_cells(
    node: &crate::layout::LayoutNode,
    cols: u16,
    rows: u16,
    cells: &mut [Option<crate::layout::LayoutNode>],
) {
    for child in &node.children {
        fill_widget_hit_cells(child, cols, rows, cells);
    }

    if widget_render::is_layout_widget_type(&node.widget_type) {
        return;
    }

    let max_row = node.rect.row.saturating_add(node.rect.height).min(rows);
    let max_col = node.rect.col.saturating_add(node.rect.width).min(cols);
    for row in node.rect.row..max_row {
        for col in node.rect.col..max_col {
            cells[row as usize * cols as usize + col as usize] = Some(node.clone());
        }
    }
}

fn same_widget_hit(
    left: Option<&crate::layout::LayoutNode>,
    right: Option<&crate::layout::LayoutNode>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => widget_hit_key(left) == widget_hit_key(right),
        (None, None) => true,
        _ => false,
    }
}

fn widget_hit_key(node: &crate::layout::LayoutNode) -> (String, u16, u16, u16, u16) {
    (
        node.widget_type.clone(),
        node.rect.row,
        node.rect.col,
        node.rect.width,
        node.rect.height,
    )
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
        "load-buffer",
        "(load-buffer)",
        "Load the current buffer from its path, discarding unsaved changes.",
        |_args, ctx| {
            ctx.request_load();
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

    runtime.register_native_with_docs(
        "set-read-only",
        "(set-read-only bool)",
        "Set the current buffer's read-only state.",
        |args, ctx| {
            let read_only = match args.first() {
                Some(Value::Bool(b)) => *b,
                Some(Value::Nil) => false,
                _ => true,
            };
            ctx.set_read_only(read_only);
            Ok(Value::Bool(read_only))
        },
    );

    runtime.register_native_with_docs(
        "toggle-read-only",
        "(toggle-read-only)",
        "Toggle the current buffer's read-only state.",
        |_args, ctx| {
            let new_val = !ctx.current_buffer_read_only();
            ctx.set_read_only(new_val);
            Ok(Value::Bool(new_val))
        },
    );

    runtime.register_native_with_docs(
        "buffer-read-only?",
        "(buffer-read-only?)",
        "Return whether the current buffer is read-only.",
        |_args, ctx| Ok(Value::Bool(ctx.current_buffer_read_only())),
    );

    runtime.register_native_with_docs(
        "define-mode",
        "(define-mode name :read-only bool :on-enter fn-name)",
        "Register a named major mode.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("define-mode expects a name string".to_string());
            };
            let mut read_only = false;
            let mut on_enter: Option<String> = None;
            let mut i = 1;
            while i < args.len() {
                match args.get(i) {
                    Some(Value::Keyword(k)) if k == "read-only" => {
                        read_only = matches!(args.get(i + 1), Some(Value::Bool(true)));
                        i += 2;
                    }
                    Some(Value::Keyword(k)) if k == "on-enter" => {
                        if let Some(Value::String(fn_name)) = args.get(i + 1) {
                            on_enter = Some(fn_name.clone());
                        }
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            ctx.define_mode(name.clone(), read_only, on_enter);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "mode-bind-key",
        "(mode-bind-key mode-name key handler)",
        "Add a keybinding to a registered mode.",
        |args, ctx| {
            let (Some(Value::String(mode)), Some(Value::String(key)), Some(Value::String(handler))) =
                (args.first(), args.get(1), args.get(2))
            else {
                return Err("mode-bind-key expects (string string string)".to_string());
            };
            ctx.mode_bind_key(mode.clone(), key.clone(), handler.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "set-buffer-mode",
        "(set-buffer-mode name)",
        "Activate a named mode on the current buffer.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("set-buffer-mode expects a name string".to_string());
            };
            ctx.set_buffer_mode(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "current-buffer-mode",
        "(current-buffer-mode)",
        "Return the current buffer's mode name.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_mode())),
    );

    runtime.register_native_with_docs(
        "create-buffer",
        "(create-buffer name)",
        "Create a new scratch buffer and switch to it.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("create-buffer expects a name string".to_string());
            };
            ctx.create_buffer(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "switch-to-buffer",
        "(switch-to-buffer name)",
        "Switch to a buffer by name.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("switch-to-buffer expects a name string".to_string());
            };
            ctx.switch_to_buffer(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "buffer-list",
        "(buffer-list)",
        "Return a list of buffer name strings.",
        |_args, ctx| {
            let names = ctx.buffer_names();
            Ok(Value::List(
                names
                    .into_iter()
                    .map(|n| Rc::new(std::cell::RefCell::new(Value::String(n))))
                    .collect(),
            ))
        },
    );

    runtime.register_native_with_docs(
        "set-buffer-text",
        "(set-buffer-text text)",
        "Replace the active buffer's contents.",
        |args, ctx| {
            let Some(Value::String(text)) = args.first() else {
                return Err("set-buffer-text expects a string".to_string());
            };
            ctx.set_buffer_text(text.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "set-buffer-lines",
        "(set-buffer-lines lines)",
        "Set the buffer contents from a list of line strings.",
        |args, ctx| {
            let Some(Value::List(items)) = args.first() else {
                return Err("set-buffer-lines expects a list".to_string());
            };
            let lines: Vec<String> = items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::String(s) => s.clone(),
                    other => format_lisp_value(other),
                })
                .collect();
            ctx.set_buffer_lines(lines);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "goto-line",
        "(goto-line n)",
        "Move cursor to line n (1-indexed).",
        |args, ctx| {
            let Some(Value::Number(n)) = args.first() else {
                return Err("goto-line expects a number".to_string());
            };
            ctx.goto_line(*n as usize);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "current-line-number",
        "(current-line-number)",
        "Return the current cursor line number (1-indexed).",
        |_args, ctx| Ok(Value::Number(ctx.current_line_number() as f64)),
    );

    runtime.register_native_with_docs(
        "current-line-text",
        "(current-line-text)",
        "Return the text of the current line.",
        |_args, ctx| Ok(Value::String(ctx.current_line_text())),
    );

    // ── Filesystem utilities ─────────────────────────────────────────────────

    runtime.register_native_with_docs(
        "list-directory",
        "(list-directory path)",
        "List directory entries as maps with :name, :directory, and :size keys.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("list-directory expects a path string".to_string());
            };
            let entries = std::fs::read_dir(path)
                .map_err(|e| format!("list-directory: {e}"))?;
            let mut result = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| format!("list-directory: {e}"))?;
                let metadata = entry.metadata().map_err(|e| format!("list-directory: {e}"))?;
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = metadata.is_dir();
                let size = metadata.len();
                let mut map = HashMap::new();
                map.insert(
                    "name".to_string(),
                    Rc::new(std::cell::RefCell::new(Value::String(name))),
                );
                map.insert(
                    "directory".to_string(),
                    Rc::new(std::cell::RefCell::new(Value::Bool(is_dir))),
                );
                map.insert(
                    "size".to_string(),
                    Rc::new(std::cell::RefCell::new(Value::Number(size as f64))),
                );
                result.push(Rc::new(std::cell::RefCell::new(Value::Map(map))));
            }
            Ok(Value::List(result))
        },
    );

    runtime.register_native_with_docs(
        "current-directory",
        "(current-directory)",
        "Return the current working directory as a string.",
        |_args, _ctx| {
            let cwd = std::env::current_dir()
                .map_err(|e| format!("current-directory: {e}"))?;
            Ok(Value::String(cwd.display().to_string()))
        },
    );

    runtime.register_native_with_docs(
        "path-join",
        "(path-join a b)",
        "Join two path components.",
        |args, _ctx| {
            let (Some(Value::String(a)), Some(Value::String(b))) =
                (args.first(), args.get(1))
            else {
                return Err("path-join expects two strings".to_string());
            };
            let result = PathBuf::from(a).join(b);
            Ok(Value::String(result.display().to_string()))
        },
    );

    runtime.register_native_with_docs(
        "path-parent",
        "(path-parent path)",
        "Return the parent directory of a path, or nil.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("path-parent expects a string".to_string());
            };
            Ok(match PathBuf::from(path).parent() {
                Some(parent) => Value::String(parent.display().to_string()),
                None => Value::Nil,
            })
        },
    );

    runtime.register_native_with_docs(
        "path-filename",
        "(path-filename path)",
        "Return the filename component of a path, or nil.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("path-filename expects a string".to_string());
            };
            Ok(match PathBuf::from(path).file_name() {
                Some(name) => Value::String(name.to_string_lossy().to_string()),
                None => Value::Nil,
            })
        },
    );

    runtime.register_native_with_docs(
        "file-exists?",
        "(file-exists? path)",
        "Return true if a file or directory exists at path.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("file-exists? expects a string".to_string());
            };
            Ok(Value::Bool(Path::new(path).exists()))
        },
    );

    runtime.register_native_with_docs(
        "directory?",
        "(directory? path)",
        "Return true if path is a directory.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("directory? expects a string".to_string());
            };
            Ok(Value::Bool(Path::new(path).is_dir()))
        },
    );

    runtime.register_native_with_docs(
        "read-file-to-string",
        "(read-file-to-string path)",
        "Read a file's contents as a string.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("read-file-to-string expects a string".to_string());
            };
            let contents = std::fs::read_to_string(path)
                .map_err(|e| format!("read-file-to-string: {e}"))?;
            Ok(Value::String(contents))
        },
    );

    runtime.register_native_with_docs(
        "render-widget",
        "(render-widget tree)",
        "Render a widget tree in the current buffer's overlay.",
        |args, ctx| {
            let Some(tree) = args.into_iter().next() else {
                return Err("render-widget expects a widget tree value".to_string());
            };
            ctx.render_widget(tree);
            Ok(Value::Nil)
        },
    );

    runtime.register_native_with_docs(
        "open-file",
        "(open-file path)",
        "Open a file into a new file-backed buffer and switch to it.",
        |args, ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("open-file expects a path string".to_string());
            };
            ctx.open_file(path.clone());
            Ok(Value::Bool(true))
        },
    );

    // ── String utilities ─────────────────────────────────────────────────────

    runtime.register_native_with_docs(
        "substring",
        "(substring s start [end])",
        "Extract a substring by character index.",
        |args, _ctx| {
            let Some(Value::String(s)) = args.first() else {
                return Err("substring expects a string".to_string());
            };
            let Some(Value::Number(start)) = args.get(1) else {
                return Err("substring expects a start index".to_string());
            };
            let start = (*start as usize).min(s.len());
            let end = match args.get(2) {
                Some(Value::Number(e)) => (*e as usize).min(s.len()),
                _ => s.len(),
            };
            Ok(Value::String(s.get(start..end).unwrap_or("").to_string()))
        },
    );

    runtime.register_native_with_docs(
        "string-starts-with?",
        "(string-starts-with? s prefix)",
        "Return true if string starts with prefix.",
        |args, _ctx| {
            let (Some(Value::String(s)), Some(Value::String(prefix))) =
                (args.first(), args.get(1))
            else {
                return Err("string-starts-with? expects two strings".to_string());
            };
            Ok(Value::Bool(s.starts_with(prefix.as_str())))
        },
    );

    runtime.register_native_with_docs(
        "string-ends-with?",
        "(string-ends-with? s suffix)",
        "Return true if string ends with suffix.",
        |args, _ctx| {
            let (Some(Value::String(s)), Some(Value::String(suffix))) =
                (args.first(), args.get(1))
            else {
                return Err("string-ends-with? expects two strings".to_string());
            };
            Ok(Value::Bool(s.ends_with(suffix.as_str())))
        },
    );

    runtime.register_native_with_docs(
        "string-trim",
        "(string-trim s)",
        "Remove leading and trailing whitespace.",
        |args, _ctx| {
            let Some(Value::String(s)) = args.first() else {
                return Err("string-trim expects a string".to_string());
            };
            Ok(Value::String(s.trim().to_string()))
        },
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
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("eseqlisp-{name}-{unique}.lisp"))
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

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
    fn alt_left_moves_to_previous_word() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 10);

        editor.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));

        assert_eq!(editor.active_buffer().cursor, (0, 8));
    }

    #[test]
    fn alt_right_moves_to_next_word() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "abc def ghi");
        editor.active_buffer_mut().cursor = (0, 0);

        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));

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

        assert_eq!(editor.active_buffer().text(), "(if test\n  :4t)");
        assert_eq!(editor.active_buffer().cursor, (1, 2));
    }

    #[test]
    fn enter_inserts_lisp_indentation() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(if test)");
        editor.active_buffer_mut().cursor = (0, 8);

        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(editor.active_buffer().text(), "(if test\n  )");
        assert_eq!(editor.active_buffer().cursor, (1, 2));
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
    fn load_buffer_reloads_active_file_from_disk() {
        let path = temp_file_path("load-buffer");
        fs::write(&path, "(+ 1 2)\n").unwrap();

        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_file_buffer(&path).unwrap();
        editor.active_buffer_mut().cursor = (0, 0);
        editor.active_buffer_mut().insert_char(';');
        assert!(editor.active_buffer().dirty);

        editor.runtime_mut().eval_str("(load-buffer)").unwrap();
        editor.refresh_runtime_side_effects();

        assert_eq!(editor.active_buffer().text(), "(+ 1 2)\n");
        assert!(!editor.active_buffer().dirty);
        let expected = format!("Loaded {}", path.display());
        assert_eq!(editor.minibuffer.as_deref(), Some(expected.as_str()));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_buffer_errors_for_non_file_backed_buffer() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*test*", "(+ 1 2)");

        editor.runtime_mut().eval_str("(load-buffer)").unwrap();
        editor.refresh_runtime_side_effects();

        let minibuffer = editor.minibuffer.unwrap_or_default();
        assert!(minibuffer.contains("buffer is not file-backed"));
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

    #[test]
    fn mouse_click_moves_cursor_in_text_view() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.open_scratch_buffer("*test*", "alpha\nbravo");

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 3, 2),
            1,
            1,
            20,
            10,
        );

        assert_eq!(editor.active_buffer().cursor, (1, 2));
    }

    #[test]
    fn mouse_drag_updates_slider_via_on_change_callback() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(20, 6);
        editor
            .runtime
            .eval_str(
                r#"
                (def level (state 0))
                (effect
                  (hslider
                    :min 0
                    :max 100
                    :value level
                    :on-change |v| (set! level v)))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(20, 6);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 9, 1),
            1,
            1,
            20,
            6,
        );

        let value = editor.runtime.eval_str("level").unwrap().unwrap();
        match value {
            Value::Number(n) => assert_eq!(n, 0.0),
            _ => panic!("expected numeric slider state"),
        }

        editor.handle_mouse(
            mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
            1,
            1,
            20,
            6,
        );

        let value = editor.runtime.eval_str("level").unwrap().unwrap();
        match value {
            Value::Number(n) => assert!(n > 90.0),
            _ => panic!("expected numeric slider state"),
        }
    }

    #[test]
    fn mouse_drag_updates_slider_via_bind_shorthand() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(20, 6);
        editor
            .runtime
            .eval_str(
                r#"
                (def level (state 0))
                (effect
                  (hslider
                    :min 0
                    :max 100
                    :bind level))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(20, 6);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1),
            1,
            1,
            20,
            6,
        );
        editor.handle_mouse(
            mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1),
            1,
            1,
            20,
            6,
        );

        let value = editor.runtime.eval_str("level").unwrap().unwrap();
        match value {
            Value::Number(n) => assert!(n > 90.0),
            _ => panic!("expected numeric slider state"),
        }
    }

    #[test]
    fn mouse_down_updates_knob_via_bind_shorthand() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(6, 6);
        editor
            .runtime
            .eval_str(
                r#"
                (def level (state 0))
                (effect
                  (knob
                    :size 2
                    :min 0
                    :max 100
                    :bind level))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(6, 6);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
            1,
            1,
            6,
            6,
        );

        let value = editor.runtime.eval_str("level").unwrap().unwrap();
        match value {
            Value::Number(n) => assert!(n >= 99.0),
            _ => panic!("expected numeric knob state"),
        }
    }

    #[test]
    fn knob_updates_shared_label_state_from_each_binding() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(40, 12);
        editor
            .runtime
            .eval_str(
                r#"
                (defstate steps '(20 30 40 50))
                (effect
                  (h-stack
                    (grid :cols 4 :col-width 4
                      (each steps |step|
                        (knob :min 0 :max 100 :bind step)))
                    (grid :cols 4 :col-width 4
                      (each steps |step|
                        (label (fmt "{:.0}" step))))))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(40, 12);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1),
            1,
            1,
            40,
            12,
        );

        let value = editor.runtime.eval_str("steps").unwrap().unwrap();
        let Value::List(items) = value else {
            panic!("expected steps list");
        };
        let first = items[0].borrow().clone();
        match first {
            Value::Number(n) => assert!(n >= 99.0),
            _ => panic!("expected numeric step"),
        }

        let layout = editor.runtime.current_layout.as_ref().expect("layout");
        let rendered = crate::layout::format_layout_tree_lines(layout, 0);
        assert!(
            rendered.iter().any(|line| line.contains("text=\"100\"")),
            "expected shared label text to reflect updated knob value: {rendered:?}"
        );
    }

    #[test]
    fn knob_drag_clamps_after_leaving_hit_rect() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(10, 10);
        editor
            .runtime
            .eval_str(
                r#"
                (def level (state 0))
                (effect
                  (knob
                    :size 4
                    :min 0
                    :max 100
                    :bind level))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(10, 10);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 4),
            1,
            1,
            10,
            10,
        );
        editor.handle_mouse(
            mouse_event(MouseEventKind::Drag(MouseButton::Left), 2, 0),
            1,
            1,
            10,
            10,
        );

        let value = editor.runtime.eval_str("level").unwrap().unwrap();
        match value {
            Value::Number(n) => assert!(n >= 99.0),
            _ => panic!("expected numeric knob state"),
        }
    }

    #[test]
    fn eval_sexp_replaces_previous_preview_effect_layout() {
        let init = r#"
            (def eval-sexp ()
              (let ((form (s-expression-at-cursor)))
                (if (= form "")
                  (status "No s-expression at cursor")
                  (let ((result (eval form)))
                    result))))
            (bind-key "C-x C-e" "eval-sexp")
        "#;
        let runtime = Runtime::new();
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init.to_string()),
            },
        );
        editor.open_scratch_buffer(
            "*test*",
            "(effect (h-stack (label \"hello\") (hslider :min 0 :max 100 :bind x)))",
        );
        editor.runtime.eval_str("(defstate x 0)").unwrap();
        editor.active_buffer_mut().cursor = (0, editor.active_buffer().lines[0].len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        editor
            .active_buffer_mut()
            .set_text("(effect (h-stack (hslider :min 0 :max 100 :bind x)))");
        editor.active_buffer_mut().cursor = (0, editor.active_buffer().lines[0].len());

        editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        editor.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));

        editor
            .runtime
            .set_reactive("APP", "unused", Value::Number(0.0));
        let layout = editor.runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.widget_type, "h-stack");
        assert_eq!(layout.children.len(), 1);
        assert_eq!(layout.children[0].widget_type, "hslider");
    }

    #[test]
    fn mouse_drag_updates_bound_step_field_from_each() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(40, 6);
        editor
            .runtime
            .eval_str(
                r#"
                (defstate pattern
                  (dict :steps
                    (list
                      (dict :velocity 20)
                      (dict :velocity 50))))
                (effect
                  (h-stack
                    (each pattern.steps |v|
                      (hslider :min 0 :max 100 :bind v.velocity))))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(40, 6);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 10, 1),
            1,
            1,
            40,
            6,
        );
        editor.handle_mouse(
            mouse_event(MouseEventKind::Drag(MouseButton::Left), 10, 1),
            1,
            1,
            40,
            6,
        );

        let value = editor.runtime.eval_str("pattern.steps").unwrap().unwrap();
        let Value::List(items) = value else {
            panic!("expected steps list");
        };
        let first = items[0].borrow().clone();
        let Value::Map(map) = first else {
            panic!("expected step map");
        };
        let velocity = map.get("velocity").expect("velocity").borrow().clone();
        match velocity {
            Value::Number(n) => assert!(n > 50.0),
            _ => panic!("expected numeric velocity"),
        }
    }

    #[test]
    fn mouse_drag_updates_bound_list_item_from_each() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(40, 12);
        editor
            .runtime
            .eval_str(
                r#"
                (defstate steps '(20 30 40 50 60))
                (effect
                  (h-stack
                    (each steps |step|
                      (vslider :min 0 :max 100 :bind step))))
                "#,
            )
            .unwrap();
        editor.set_layout_viewport(40, 12);

        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 5),
            1,
            1,
            40,
            12,
        );
        editor.handle_mouse(
            mouse_event(MouseEventKind::Drag(MouseButton::Left), 1, 2),
            1,
            1,
            40,
            12,
        );

        let value = editor.runtime.eval_str("steps").unwrap().unwrap();
        let Value::List(items) = value else {
            panic!("expected steps list");
        };
        match &*items[0].borrow() {
            Value::Number(n) => assert!(*n > 20.0),
            _ => panic!("expected numeric step"),
        }
    }

    #[test]
    fn reevaluating_defstate_and_effect_rebuilds_each_layout() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());

        editor
            .runtime
            .eval_str("(defstate steps '(1 2 3 4 5))")
            .unwrap();
        editor
            .runtime
            .eval_str(
                r#"
                (effect
                  (h-stack
                    (each steps |step|
                      (vslider :min 0 :max 100 :bind step))))
                "#,
            )
            .unwrap();

        let layout = editor.runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.children.len(), 5);

        editor
            .runtime
            .eval_str("(defstate steps '(1 2 3 4 5 6 7 8 9))")
            .unwrap();
        let result = editor.runtime.eval_str(
            r#"
                (effect
                  (h-stack
                    (each steps |step|
                      (vslider :min 0 :max 100 :bind step))))
                "#,
        );

        assert!(result.is_ok(), "effect re-eval failed: {result:?}");
        let layout = editor.runtime.current_layout.as_ref().expect("layout");
        assert_eq!(layout.children.len(), 9);
    }

    #[test]
    fn dired_mode_loads_and_refreshes() {
        let init = std::fs::read_to_string("init.lisp").unwrap_or_default();
        let runtime = Runtime::new();
        let mut editor = Editor::new(
            runtime,
            EditorConfig {
                init_source: Some(init),
            },
        );

        // Call dired-here and verify full state
        editor.call_lisp_handler("dired-here");

        assert_eq!(editor.active_buffer().name, "*dired*");
        assert!(
            editor.active_buffer().read_only,
            "dired buffer should be read-only, mode={:?}",
            editor.active_buffer().mode
        );
        assert!(
            matches!(editor.active_buffer().mode, BufferMode::Named(ref n) if n == "dired-mode"),
            "mode should be dired-mode, got {:?}",
            editor.active_buffer().mode
        );
        // Widget-based dired: check that a layout exists
        assert!(
            editor.widget_layout().is_some(),
            "dired should have a widget layout"
        );
        let layout = editor.widget_layout().unwrap();
        assert!(layout.children.len() > 2, "should list files as widget children");

        assert!(
            editor.focused_widget_id().is_some(),
            "should auto-focus first focusable widget"
        );
    }

    #[test]
    fn widget_interaction_survives_buffer_switch() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(20, 6);

        editor.runtime_mut().eval_str(
            r#"(def level (state 0))
               (effect (hslider :min 0 :max 100 :value level :on-change |v| (set! level v)))"#,
        ).unwrap();
        editor.set_layout_viewport(20, 6);
        assert!(editor.widget_layout().is_some());

        // Interact before switch — should work
        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1), 1, 1, 20, 6,
        );
        let _val = editor.runtime_mut().eval_str("level").unwrap().unwrap();

        // Switch away
        editor.open_scratch_buffer("*other*", "hello");
        assert!(editor.widget_layout().is_none());

        // Switch back
        let id = editor.buffers.iter().find(|b| b.name == "*scratch*").unwrap().id;
        editor.set_active_buffer(id);
        assert!(editor.widget_layout().is_some(), "layout should be restored");

        // Try to interact after switch back
        editor.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), 16, 1), 1, 1, 20, 6,
        );
        editor.handle_mouse(
            mouse_event(MouseEventKind::Drag(MouseButton::Left), 16, 1), 1, 1, 20, 6,
        );

        let val = editor.runtime_mut().eval_str("level").unwrap().unwrap();
        match val {
            Value::Number(n) => assert!(n > 0.0, "level should have changed, got {n}"),
            _ => panic!("expected number"),
        }
    }

    #[test]
    fn widget_tree_survives_buffer_switch() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor.set_layout_viewport(20, 6);

        // Create an effect layout in the scratch buffer
        editor.runtime_mut().eval_str(
            r#"(def level (state 0))
               (effect (hslider :min 0 :max 100 :value level :on-change |v| (set! level v)))"#,
        ).unwrap();
        editor.set_layout_viewport(20, 6);

        assert!(editor.widget_layout().is_some(), "should have layout before switch");
        let original_buffer_name = editor.active_buffer().name.clone();

        // Open a new buffer (simulating switch away)
        editor.open_scratch_buffer("*other*", "hello");
        assert_eq!(editor.active_buffer().name, "*other*");
        // Widget should be gone for this buffer
        assert!(editor.widget_layout().is_none(), "other buffer should have no layout");

        // Switch back
        let original_id = editor.buffers.iter().find(|b| b.name == original_buffer_name).unwrap().id;
        editor.set_active_buffer(original_id);
        assert_eq!(editor.active_buffer().name, original_buffer_name);

        // Widget should be restored
        assert!(
            editor.widget_layout().is_some(),
            "widget layout should be restored after switching back. widget_tree={:?}",
            editor.active_buffer().widget_tree.is_some()
        );
    }
}
