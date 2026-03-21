use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::audio::register_audio_natives;
use crate::host::{BufferId, HostCommand};
use crate::layout::{
    LayoutEngine, LayoutNode, format_layout_tree_lines, reuse_layout_node, same_layout_geometry,
};
use crate::reactive::ReactiveRegistry;
use crate::vm::{VM, Value, register_core_natives};
use crate::widgets::register_widget_natives;

pub type RuntimeError = String;
pub type NativeResult = Result<Value, RuntimeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolMetadata {
    pub signature: String,
    pub docs: String,
}

#[derive(Default)]
pub(crate) struct RuntimeBridgeState {
    pub current_buffer_id: Option<BufferId>,
    pub current_buffer_name: String,
    pub current_buffer_path: Option<PathBuf>,
    pub current_buffer_text: String,
    pub current_sexp: Option<String>,
    pub status_message: Option<String>,
    pub queued_commands: Vec<HostCommand>,
    pub lisp_bindings: HashMap<String, String>,
    pub pending_save: bool,
    pub pending_save_as: Option<PathBuf>,
    pub pending_load: bool,
    pub current_buffer_read_only: bool,
    pub pending_set_read_only: Option<bool>,
    pub current_buffer_mode: String,
    pub pending_mode_defs: Vec<(String, bool, Option<String>)>, // (name, read_only, on_enter)
    pub pending_mode_bindings: Vec<(String, String, String)>,   // (mode, key, handler)
    pub pending_set_mode: Option<String>,
    pub pending_open_file: Option<String>,
    pub pending_widget_tree: Option<Value>,
    pub pending_create_buffer: Option<String>,
    pub pending_switch_buffer: Option<String>,
    pub pending_set_text: Option<String>,
    pub pending_set_lines: Option<Vec<String>>,
    pub pending_goto_line: Option<usize>,
    pub current_line_number: usize,
    pub current_line_text: String,
    pub buffer_names: Vec<String>,
    pub pending_cycle_view_mode: bool,
    pub current_view_mode: String,
    // Tiling operations
    pub pending_split_right: bool,
    pub pending_split_below: bool,
    pub pending_delete_window: bool,
    pub pending_delete_other_windows: bool,
    pub pending_other_window: bool,
    pub pending_set_window_buffer: Option<String>,
    pub pending_window_hide_status: bool,
    pub pending_resize_window: Option<f64>,
}

pub(crate) type SharedBridgeState = Rc<RefCell<RuntimeBridgeState>>;

pub struct NativeContext {
    shared: SharedBridgeState,
}

impl NativeContext {
    pub(crate) fn new(shared: SharedBridgeState) -> Self {
        Self { shared }
    }

    pub fn current_buffer_id(&self) -> Option<BufferId> {
        self.shared.borrow().current_buffer_id
    }

    pub fn current_buffer_name(&self) -> String {
        self.shared.borrow().current_buffer_name.clone()
    }

    pub fn current_buffer_text(&self) -> String {
        self.shared.borrow().current_buffer_text.clone()
    }

    pub fn current_buffer_path(&self) -> Option<PathBuf> {
        self.shared.borrow().current_buffer_path.clone()
    }

    pub fn current_sexp(&self) -> Option<String> {
        self.shared.borrow().current_sexp.clone()
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.shared.borrow_mut().status_message = Some(status.into());
    }

    pub fn enqueue_command(&mut self, command: HostCommand) {
        self.shared.borrow_mut().queued_commands.push(command);
    }

    pub fn bind_key(&mut self, key: String, handler: String) {
        self.shared.borrow_mut().lisp_bindings.insert(key, handler);
    }

    pub fn request_save(&mut self) {
        self.shared.borrow_mut().pending_save = true;
    }

    pub fn request_save_as(&mut self, path: impl Into<PathBuf>) {
        self.shared.borrow_mut().pending_save_as = Some(path.into());
    }

    pub fn request_load(&mut self) {
        self.shared.borrow_mut().pending_load = true;
    }

    pub fn current_buffer_read_only(&self) -> bool {
        self.shared.borrow().current_buffer_read_only
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.shared.borrow_mut().pending_set_read_only = Some(read_only);
    }

    pub fn current_buffer_mode(&self) -> String {
        self.shared.borrow().current_buffer_mode.clone()
    }

    pub fn define_mode(&mut self, name: String, read_only: bool, on_enter: Option<String>) {
        self.shared
            .borrow_mut()
            .pending_mode_defs
            .push((name, read_only, on_enter));
    }

    pub fn mode_bind_key(&mut self, mode: String, key: String, handler: String) {
        self.shared
            .borrow_mut()
            .pending_mode_bindings
            .push((mode, key, handler));
    }

    pub fn set_buffer_mode(&mut self, mode: String) {
        self.shared.borrow_mut().pending_set_mode = Some(mode);
    }

    pub fn open_file(&mut self, path: String) {
        self.shared.borrow_mut().pending_open_file = Some(path);
    }

    pub fn create_buffer(&mut self, name: String) {
        self.shared.borrow_mut().pending_create_buffer = Some(name);
    }

    pub fn switch_to_buffer(&mut self, name: String) {
        self.shared.borrow_mut().pending_switch_buffer = Some(name);
    }

    pub fn set_buffer_text(&mut self, text: String) {
        self.shared.borrow_mut().pending_set_text = Some(text);
    }

    pub fn set_buffer_lines(&mut self, lines: Vec<String>) {
        self.shared.borrow_mut().pending_set_lines = Some(lines);
    }

    pub fn render_widget(&mut self, tree: Value) {
        self.shared.borrow_mut().pending_widget_tree.replace(tree);
    }

    pub fn goto_line(&mut self, line: usize) {
        self.shared.borrow_mut().pending_goto_line = Some(line);
    }

    pub fn current_line_number(&self) -> usize {
        self.shared.borrow().current_line_number
    }

    pub fn current_line_text(&self) -> String {
        self.shared.borrow().current_line_text.clone()
    }

    pub fn buffer_names(&self) -> Vec<String> {
        self.shared.borrow().buffer_names.clone()
    }

    // ── Tiling operations ─────────────────────────────────────────────────

    pub fn split_window_right(&mut self) {
        self.shared.borrow_mut().pending_split_right = true;
    }

    pub fn split_window_below(&mut self) {
        self.shared.borrow_mut().pending_split_below = true;
    }

    pub fn delete_window(&mut self) {
        self.shared.borrow_mut().pending_delete_window = true;
    }

    pub fn delete_other_windows(&mut self) {
        self.shared.borrow_mut().pending_delete_other_windows = true;
    }

    pub fn other_window(&mut self) {
        self.shared.borrow_mut().pending_other_window = true;
    }

    pub fn set_window_buffer(&mut self, name: String) {
        self.shared.borrow_mut().pending_set_window_buffer = Some(name);
    }

    pub fn window_hide_status(&mut self) {
        self.shared.borrow_mut().pending_window_hide_status = true;
    }

    pub fn resize_window(&mut self, delta: f64) {
        self.shared.borrow_mut().pending_resize_window = Some(delta);
    }

    pub fn cycle_view_mode(&mut self) {
        self.shared.borrow_mut().pending_cycle_view_mode = true;
    }

    pub fn current_view_mode(&self) -> String {
        self.shared.borrow().current_view_mode.clone()
    }
}

pub struct Runtime {
    vm: VM,
    pub(crate) shared: SharedBridgeState,
    symbol_metadata: HashMap<String, SymbolMetadata>,
    symbol_revision: u64,
    cached_completion_symbols: Option<Vec<String>>,
    cached_completion_metadata: Option<HashMap<String, SymbolMetadata>>,
    pub reactive_registry: ReactiveRegistry,
    rendered_layouts: Vec<Vec<String>>,
    pub current_layout: Option<Arc<LayoutNode>>,
    layout_revision: u64,
    dirty_widget_ids: Vec<u64>,
    current_widget_tree: Option<Value>,
    layout_cols: u16,
    layout_rows: u16,
    layout_aspect: f32,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        let shared = Rc::new(RefCell::new(RuntimeBridgeState::default()));
        let mut vm = VM::new(vec![]);
        register_core_natives(&mut vm);
        register_widget_natives(&mut vm);
        let mut runtime = Self {
            vm,
            shared,
            symbol_metadata: HashMap::new(),
            symbol_revision: 0,
            cached_completion_symbols: None,
            cached_completion_metadata: None,
            reactive_registry: ReactiveRegistry::new(),
            rendered_layouts: Vec::new(),
            current_layout: None,
            layout_revision: 0,
            dirty_widget_ids: Vec::new(),
            current_widget_tree: None,
            layout_cols: 80,
            layout_rows: 24,
            layout_aspect: 1.0,
        };
        register_audio_natives(&mut runtime);
        runtime
    }

    pub fn with_init_source(init: impl AsRef<str>) -> Self {
        let mut runtime = Self::new();
        let src = init.as_ref();
        if !src.trim().is_empty() {
            let _ = runtime.eval_str(src);
        }
        runtime
    }

    pub fn register_native<F>(&mut self, name: &str, f: F)
    where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
    {
        self.register_native_impl(name, None, None, f);
    }

    pub fn register_native_with_docs<F>(
        &mut self,
        name: &str,
        signature: impl Into<String>,
        docs: impl Into<String>,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
    {
        self.register_native_impl(name, Some(signature.into()), Some(docs.into()), f);
    }

    fn register_native_impl<F>(
        &mut self,
        name: &str,
        signature: Option<String>,
        docs: Option<String>,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
    {
        let shared = self.shared.clone();
        self.vm.register_native(name, move |args| {
            let mut ctx = NativeContext::new(shared.clone());
            match f(args, &mut ctx) {
                Ok(value) => value,
                Err(error) => {
                    ctx.set_status(format!("Error: {error}"));
                    Value::Bool(false)
                }
            }
        });
        if let (Some(signature), Some(docs)) = (signature, docs) {
            self.symbol_metadata
                .insert(name.to_string(), SymbolMetadata { signature, docs });
        }
        self.invalidate_symbol_cache();
    }

    pub fn eval_str(&mut self, src: &str) -> Result<Option<Value>, crate::vm::VMError> {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_owner(current_buffer_id);
        if src.contains("(effect") {
            self.clear_layout_effects();
        }
        let result = self.vm.eval_str(src);
        if result.is_ok() {
            self.invalidate_symbol_cache();
            self.flush_widget_trees();
        }
        result
    }

    pub fn set_global_value(&mut self, name: &str, value: Value) {
        self.vm.set_global_value(name, value);
        self.invalidate_symbol_cache();
    }

    pub fn register_reactive(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) {
        let map = self.reactive_registry.register(name, fields, writable);
        self.vm.set_global_value(name, map);
        self.vm.reactive_namespaces.insert(name.to_string());
        if writable {
            self.vm
                .writable_reactive_namespaces
                .insert(name.to_string());
        } else {
            self.vm.writable_reactive_namespaces.remove(name);
        }
        self.invalidate_symbol_cache();
    }

    pub fn set_reactive(&mut self, namespace: &str, field: &str, value: Value) {
        self.reactive_registry.set(namespace, field, value);
    }

    pub fn set_layout_viewport(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if self.layout_cols == cols && self.layout_rows == rows {
            return;
        }
        self.layout_cols = cols;
        self.layout_rows = rows;
        // Viewport changes invalidate layout geometry even if the widget tree is unchanged.
        self.current_layout = None;
        self.dirty_widget_ids.clear();
        self.relayout_current_tree();
    }

    pub fn layout_aspect(&self) -> f32 {
        self.layout_aspect
    }

    pub fn set_layout_aspect(&mut self, aspect: f32) {
        if (self.layout_aspect - aspect).abs() < f32::EPSILON {
            return;
        }
        self.layout_aspect = aspect;
        self.current_layout = None;
        self.dirty_widget_ids.clear();
        self.relayout_current_tree();
    }

    pub fn invoke(
        &mut self,
        callable: Value,
        args: Vec<Value>,
    ) -> Result<Option<Value>, crate::vm::VMError> {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_owner(current_buffer_id);
        let result = self.vm.invoke(callable, args);
        if result.is_ok() {
            self.flush_widget_trees();
        }
        result
    }

    pub fn run_reactive_cycle(&mut self) {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_owner(current_buffer_id);
        let dirty = self.reactive_registry.drain_dirty();
        if dirty.is_empty() {
            return;
        }

        if self.vm.apply_reactive_changes(dirty).is_ok() {
            self.flush_widget_trees();
        }
    }

    pub fn global_names(&self) -> &[String] {
        self.vm.global_names()
    }

    pub fn symbol_metadata(&self) -> &HashMap<String, SymbolMetadata> {
        &self.symbol_metadata
    }

    pub fn completion_symbols(&mut self) -> Vec<String> {
        if let Some(symbols) = &self.cached_completion_symbols {
            return symbols.clone();
        }

        let mut symbols = self.vm.global_names().to_vec();
        for global in self.vm.global_names() {
            if let Some(Value::Map(map)) = self.vm.global_value(global) {
                let mut keys = map.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                symbols.extend(keys.into_iter().map(|key| format!("{global}.{key}")));
            }
        }
        symbols.sort();
        symbols.dedup();
        self.cached_completion_symbols = Some(symbols.clone());
        symbols
    }

    pub fn completion_metadata(&mut self) -> HashMap<String, SymbolMetadata> {
        if let Some(metadata) = &self.cached_completion_metadata {
            return metadata.clone();
        }

        let mut metadata = self.symbol_metadata.clone();
        for global in self.vm.global_names() {
            if let Some(Value::Map(map)) = self.vm.global_value(global) {
                let mut keys = map.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                for key in keys {
                    let label = format!("{global}.{key}");
                    metadata.entry(label).or_insert_with(|| SymbolMetadata {
                        signature: format!("{global}.{key}"),
                        docs: format!("Field '{key}' on runtime map '{global}'."),
                    });
                }
            }
        }
        self.cached_completion_metadata = Some(metadata.clone());
        metadata
    }

    pub fn symbol_revision(&self) -> u64 {
        self.symbol_revision
    }

    pub fn take_status_message(&mut self) -> Option<String> {
        self.shared.borrow_mut().status_message.take()
    }

    pub fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    pub fn take_dirty_widget_ids(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dirty_widget_ids)
    }

    pub(crate) fn drain_host_commands(&mut self) -> Vec<HostCommand> {
        let mut shared = self.shared.borrow_mut();
        std::mem::take(&mut shared.queued_commands)
    }

    pub(crate) fn lisp_bindings(&self) -> HashMap<String, String> {
        self.shared.borrow().lisp_bindings.clone()
    }

    pub(crate) fn take_pending_save(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let pending = shared.pending_save;
        shared.pending_save = false;
        pending
    }

    pub(crate) fn take_pending_save_as(&mut self) -> Option<PathBuf> {
        self.shared.borrow_mut().pending_save_as.take()
    }

    pub(crate) fn take_pending_load(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let pending = shared.pending_load;
        shared.pending_load = false;
        pending
    }

    pub(crate) fn take_pending_set_read_only(&mut self) -> Option<bool> {
        self.shared.borrow_mut().pending_set_read_only.take()
    }

    pub(crate) fn take_pending_mode_defs(&mut self) -> Vec<(String, bool, Option<String>)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_mode_defs)
    }

    pub(crate) fn take_pending_mode_bindings(&mut self) -> Vec<(String, String, String)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_mode_bindings)
    }

    pub(crate) fn take_pending_set_mode(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_set_mode.take()
    }

    pub(crate) fn take_pending_widget_tree(&mut self) -> Option<Value> {
        self.shared.borrow_mut().pending_widget_tree.take()
    }

    pub(crate) fn take_pending_open_file(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_open_file.take()
    }

    pub(crate) fn take_pending_create_buffer(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_create_buffer.take()
    }

    pub(crate) fn take_pending_switch_buffer(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_switch_buffer.take()
    }

    pub(crate) fn take_pending_set_text(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_set_text.take()
    }

    pub(crate) fn take_pending_set_lines(&mut self) -> Option<Vec<String>> {
        self.shared.borrow_mut().pending_set_lines.take()
    }

    pub(crate) fn take_pending_goto_line(&mut self) -> Option<usize> {
        self.shared.borrow_mut().pending_goto_line.take()
    }

    // ── Tiling pending operations ──────────────────────────────────────────

    pub(crate) fn take_pending_split_right(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_split_right;
        shared.pending_split_right = false;
        v
    }

    pub(crate) fn take_pending_split_below(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_split_below;
        shared.pending_split_below = false;
        v
    }

    pub(crate) fn take_pending_delete_window(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_delete_window;
        shared.pending_delete_window = false;
        v
    }

    pub(crate) fn take_pending_delete_other_windows(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_delete_other_windows;
        shared.pending_delete_other_windows = false;
        v
    }

    pub(crate) fn take_pending_other_window(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_other_window;
        shared.pending_other_window = false;
        v
    }

    pub(crate) fn take_pending_set_window_buffer(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_set_window_buffer.take()
    }

    pub(crate) fn take_pending_window_hide_status(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_window_hide_status;
        shared.pending_window_hide_status = false;
        v
    }

    pub(crate) fn take_pending_resize_window(&mut self) -> Option<f64> {
        self.shared.borrow_mut().pending_resize_window.take()
    }

    pub(crate) fn take_pending_cycle_view_mode(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let pending = shared.pending_cycle_view_mode;
        shared.pending_cycle_view_mode = false;
        pending
    }

    pub fn drain_rendered_layouts(&mut self) -> Vec<Vec<String>> {
        std::mem::take(&mut self.rendered_layouts)
    }

    pub fn current_widget_tree(&self) -> Option<Value> {
        self.current_widget_tree.clone()
    }

    /// Clear the current widget tree and layout without destroying reactive effects.
    /// Used when switching to a buffer/tile that has no widget tree.
    pub fn clear_current_widget_tree(&mut self) {
        self.current_widget_tree = None;
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_widget_ids.clear();
    }

    pub fn layout_rows(&self) -> u16 {
        self.layout_rows
    }

    pub fn set_widget_tree(&mut self, tree: Value) {
        // Replace the visual widget tree without destroying reactive effects.
        // Effects from other buffers must survive buffer switches.
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_widget_ids.clear();
        self.current_widget_tree = Some(tree);
        self.relayout_current_tree();
    }

    /// Restore a previously saved widget tree for display only,
    /// without clearing reactive effects.
    pub fn restore_widget_tree(&mut self, tree: Value) {
        self.current_widget_tree = Some(tree);
        self.relayout_current_tree();
        // Force layout revision bump so GPU caches rebuild
        self.layout_revision = self.layout_revision.wrapping_add(1);
    }

    pub fn clear_layout_effects(&mut self) {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.clear_effects_for_owner(current_buffer_id);
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_widget_ids.clear();
        self.current_widget_tree = None;
        self.rendered_layouts.clear();
    }

    fn flush_widget_trees(&mut self) {
        let trees = std::mem::take(&mut self.vm.pending_widget_trees);
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        for pending in trees {
            if pending.owner_buffer_id != current_buffer_id {
                continue;
            }
            self.current_widget_tree = Some(pending.tree);
            self.relayout_current_tree();
        }
    }

    fn relayout_current_tree(&mut self) {
        let Some(tree) = self.current_widget_tree.as_ref() else {
            let had_layout = self.current_layout.is_some();
            self.current_layout = None;
            self.dirty_widget_ids.clear();
            if had_layout {
                self.layout_revision = self.layout_revision.wrapping_add(1);
            }
            return;
        };
        let mut dirty_widget_ids = Vec::new();
        if let Some(existing) = self.current_layout.as_ref()
            && let Some(updated) = reuse_layout_node(existing.as_ref(), tree, &mut dirty_widget_ids)
        {
            self.rendered_layouts
                .push(format_layout_tree_lines(&updated, 0));
            self.current_layout = Some(Arc::new(updated));
            self.dirty_widget_ids = dirty_widget_ids;
            if !self.dirty_widget_ids.is_empty() {
                self.layout_revision = self.layout_revision.wrapping_add(1);
            }
            return;
        }
        let engine = LayoutEngine::new(self.layout_cols, self.layout_rows, self.layout_aspect);
        if let Some(layout) = engine.layout(tree) {
            let geometry_changed = self
                .current_layout
                .as_ref()
                .is_none_or(|existing| !same_layout_geometry(existing.as_ref(), &layout));
            self.rendered_layouts
                .push(format_layout_tree_lines(&layout, 0));
            self.current_layout = Some(Arc::new(layout));
            if geometry_changed {
                self.dirty_widget_ids.clear();
                self.layout_revision = self.layout_revision.wrapping_add(1);
            } else if let Some(layout) = self.current_layout.as_ref() {
                self.dirty_widget_ids = collect_shader_widget_ids(layout);
            }
        }
    }

    fn invalidate_symbol_cache(&mut self) {
        self.symbol_revision = self.symbol_revision.wrapping_add(1);
        self.cached_completion_symbols = None;
        self.cached_completion_metadata = None;
    }
}

fn collect_shader_widget_ids(node: &LayoutNode) -> Vec<u64> {
    let mut ids = Vec::new();
    collect_shader_widget_ids_recursive(node, &mut ids);
    ids
}

fn collect_shader_widget_ids_recursive(node: &LayoutNode, ids: &mut Vec<u64>) {
    if matches!(
        node.widget_type.as_str(),
        "slider" | "hslider" | "vslider" | "toggle" | "knob"
    ) {
        ids.push(node.widget_id);
    }
    for child in &node.children {
        collect_shader_widget_ids_recursive(child, ids);
    }
}
