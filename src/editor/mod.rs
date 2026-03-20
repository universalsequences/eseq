mod commands;
mod minibuffer;
mod natives;
mod widget_focus;
mod widget_interaction;

use std::collections::HashMap;
use std::path::PathBuf;
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
use commands::key_str;
use natives::register_editor_natives;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Both,
    UiOnly,
    TextOnly,
}

impl ViewMode {
    pub fn cycle(self) -> Self {
        match self {
            ViewMode::Both => ViewMode::UiOnly,
            ViewMode::UiOnly => ViewMode::TextOnly,
            ViewMode::TextOnly => ViewMode::Both,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Both => "both",
            ViewMode::UiOnly => "ui",
            ViewMode::TextOnly => "text",
        }
    }
}

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

struct CachedHitGrid {
    layout_revision: u64,
    scroll_top: u16,
    grid: crate::ui::hit::HitGrid,
}

#[derive(Debug, Clone)]
struct WidgetGesture {
    widget_id: u64,
    start_precise_col: f32,
    start_precise_row: f32,
    gesture_data: Option<Value>,
}

#[derive(Debug, Clone)]
struct WidgetClick {
    widget_id: u64,
    precise_col: f32,
    precise_row: f32,
    at: Instant,
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
    hit_grid_cache: Option<CachedHitGrid>,
    last_mouse_precise: Option<(f32, f32)>,
    eval_flash: Option<SExpFlash>,
    mark: Option<Mark>,
    kill_ring: Vec<String>,
    minibuffer_input: Option<MinibufferMode>,
    mode_registry: HashMap<String, MajorMode>,
    focused_widget_id: Option<u64>,
    widget_scroll_top: u16,
    active_widget_gesture: Option<WidgetGesture>,
    last_widget_click: Option<WidgetClick>,
    pub view_mode: ViewMode,
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
            hit_grid_cache: None,
            last_mouse_precise: None,
            eval_flash: None,
            mark: None,
            kill_ring: vec![],
            minibuffer_input: None,
            mode_registry: HashMap::new(),
            focused_widget_id: None,
            widget_scroll_top: 0,
            active_widget_gesture: None,
            last_widget_click: None,
            view_mode: ViewMode::Both,
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
        let flash = self.eval_flash.as_ref()?;
        if flash.buffer_id != self.active_buffer().id {
            self.eval_flash = None;
            return None;
        }
        if flash.expires_at < Instant::now() {
            self.eval_flash = None;
            return None;
        }
        Some(flash.range)
    }

    pub fn active_region_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let mark = self.mark.as_ref()?;
        if mark.buffer_id != self.active_buffer().id {
            return None;
        }
        Some(normalize_region(mark.cursor, self.active_buffer().cursor))
    }

    pub fn active_buffer(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    pub fn widget_scroll_top(&self) -> u16 {
        self.widget_scroll_top
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
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, name);
        buffer.set_text(initial);
        buffer.set_mode(mode);
        self.save_current_widget_tree();
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
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
        let path = path.into();
        let text = std::fs::read_to_string(&path)?;
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, &name);
        buffer.set_text(&text);
        buffer.set_path(path);
        buffer.set_mode(mode);
        buffer.dirty = false;
        self.save_current_widget_tree();
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
        self.clear_widget_focus();
        Ok(id)
    }

    pub fn open_or_create_file_buffer(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<BufferId, EditorError> {
        self.open_or_create_file_buffer_with_mode(path, BufferMode::ESeqLisp)
    }

    pub fn open_or_create_file_buffer_with_mode(
        &mut self,
        path: impl Into<PathBuf>,
        mode: BufferMode,
    ) -> Result<BufferId, EditorError> {
        let path = path.into();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(EditorError::Io(err)),
        };
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, &name);
        buffer.set_text(&text);
        buffer.set_path(path);
        buffer.set_mode(mode);
        buffer.dirty = false;
        self.save_current_widget_tree();
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
        self.clear_widget_focus();
        Ok(id)
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

        if self.handle_focused_widget_key(key) {
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
                self.active_widget_gesture = None;
                // Try click-to-activate on focusable widgets first
                if self.try_click_focusable_widget(
                    mouse, content_col, content_row,
                ) {
                    return;
                }
                if self.try_handle_widget_double_click(
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                ) {
                    self.remember_widget_click(content_col, content_row, precise_col, precise_row);
                    return;
                }
                self.begin_widget_gesture(
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                );
                if self.try_handle_widget_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                ) {
                    self.remember_widget_click(content_col, content_row, precise_col, precise_row);
                    return;
                }
                if self
                    .widget_node_at_screen(precise_col, precise_row, content_col, content_row)
                    .is_some()
                {
                    self.remember_widget_click(content_col, content_row, precise_col, precise_row);
                    return;
                }
                self.handle_text_click(
                    mouse,
                    content_col,
                    content_row,
                    content_width,
                    content_height,
                );
                self.last_widget_click = None;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let previous = self
                    .last_mouse_precise
                    .unwrap_or((precise_col, precise_row));
                if let Some(gesture) = self.active_widget_gesture.clone() {
                    if let Some(output) = self.dispatch_gesture_widget_mouse_event(
                        gesture,
                        mouse.kind,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    ) {
                        let _ = self.apply_widget_output(Some(output));
                    }
                } else {
                    self.try_handle_widget_drag_segment(
                        mouse,
                        content_col,
                        content_row,
                        previous,
                        (precise_col, precise_row),
                    );
                }
                self.last_mouse_precise = Some((precise_col, precise_row));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(gesture) = self.active_widget_gesture.take() {
                    let output = self.dispatch_gesture_widget_mouse_event(
                        gesture,
                        mouse.kind,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    );
                    let _ = self.apply_widget_output(output);
                }
                self.last_mouse_precise = None;
            }
            MouseEventKind::ScrollUp => {
                if self.try_handle_widget_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                ) {
                    return;
                }
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
                if self.try_handle_widget_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                ) {
                    return;
                }
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
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                let _ = self.try_handle_widget_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                );
            }
            _ => {}
        }
    }

    pub fn handle_touchpad_magnify(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta: f64,
    ) {
        self.handle_touchpad_magnify_impl(content_col, content_row, precise_col, precise_row, delta);
    }

    pub fn handle_touchpad_scroll(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) {
        self.handle_touchpad_scroll_impl(content_col, content_row, precise_col, precise_row, delta_x, delta_y);
    }

    // ── Internal methods ─────────────────────────────────────────────────────

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

    fn apply_widget_output(&mut self, output: Option<crate::widget_render::EventOutput>) -> bool {
        let Some(output) = output else {
            return false;
        };
        let result = self.runtime.invoke(output.callback, output.args);
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

    fn guard_read_only(&mut self) -> bool {
        if self.active_buffer().read_only {
            self.minibuffer = Some("Buffer is read-only".to_string());
            true
        } else {
            false
        }
    }
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

fn normalize_region(
    a: (usize, usize),
    b: (usize, usize),
) -> ((usize, usize), (usize, usize)) {
    if a < b { (a, b) } else { (b, a) }
}

fn filter_candidates(candidates: &[String], input: &str) -> Vec<String> {
    if input.is_empty() {
        return candidates.to_vec();
    }
    let lower = input.to_ascii_lowercase();
    candidates
        .iter()
        .filter(|c| c.to_ascii_lowercase().contains(&lower))
        .cloned()
        .collect()
}

fn format_value_for_minibuffer(value: &Value) -> String {
    let mut s = format_lisp_value(value);
    if s.len() > 240 {
        s.truncate(237);
        s.push_str("...");
    }
    s
}

// ── Test helpers (used by tests via super::) ─────────────────────────────

#[cfg(test)]
fn get_map_field_number(value: &Value, key: &str) -> Option<f64> {
    let Value::Map(map) = value else {
        return None;
    };
    match &*map.get(key)?.borrow() {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
fn get_map_field_keyword(value: &Value, key: &str) -> Option<String> {
    let Value::Map(map) = value else {
        return None;
    };
    match &*map.get(key)?.borrow() {
        Value::Keyword(k) => Some(k.clone()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
fn get_first_list_number(value: &Value, key: &str) -> Option<f64> {
    let Value::Map(map) = value else {
        return None;
    };
    let list_val = map.get(key)?;
    let Value::List(items) = &*list_val.borrow() else {
        return None;
    };
    match &*items.first()?.borrow() {
        Value::Number(n) => Some(*n),
        _ => None,
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

    // Tests are included from the original file — they reference super:: helpers above.
    include!("tests.rs");
}
