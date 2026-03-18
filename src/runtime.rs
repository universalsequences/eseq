use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::host::{BufferId, HostCommand};
use crate::layout::{LayoutEngine, LayoutNode, format_layout_tree_lines};
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
}

pub struct Runtime {
    vm: VM,
    pub(crate) shared: SharedBridgeState,
    symbol_metadata: HashMap<String, SymbolMetadata>,
    pub reactive_registry: ReactiveRegistry,
    rendered_layouts: Vec<Vec<String>>,
    pub current_layout: Option<LayoutNode>,
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
        Self {
            vm,
            shared,
            symbol_metadata: HashMap::new(),
            reactive_registry: ReactiveRegistry::new(),
            rendered_layouts: Vec::new(),
            current_layout: None,
        }
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
    }

    pub fn eval_str(&mut self, src: &str) -> Result<Option<Value>, crate::vm::VMError> {
        let result = self.vm.eval_str(src);
        if result.is_ok() {
            self.flush_widget_trees();
        }
        result
    }

    pub fn set_global_value(&mut self, name: &str, value: Value) {
        self.vm.set_global_value(name, value);
    }

    pub fn register_reactive(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) {
        let map = self.reactive_registry.register(name, fields, writable);
        self.vm.set_global_value(name, map);
        self.vm.reactive_namespaces.insert(name.to_string());
    }

    pub fn set_reactive(&mut self, namespace: &str, field: &str, value: Value) {
        self.reactive_registry.set(namespace, field, value);
    }

    pub fn run_reactive_cycle(&mut self) {
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

    pub fn completion_symbols(&self) -> Vec<String> {
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
        symbols
    }

    pub fn completion_metadata(&self) -> HashMap<String, SymbolMetadata> {
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
        metadata
    }

    pub fn take_status_message(&mut self) -> Option<String> {
        self.shared.borrow_mut().status_message.take()
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

    pub fn drain_rendered_layouts(&mut self) -> Vec<Vec<String>> {
        std::mem::take(&mut self.rendered_layouts)
    }

    fn flush_widget_trees(&mut self) {
        let trees = std::mem::take(&mut self.vm.pending_widget_trees);
        let engine = LayoutEngine::new(80, 24);
        for tree in trees {
            if let Some(layout) = engine.layout(&tree) {
                let lines = format_layout_tree_lines(&layout, 0);
                self.rendered_layouts.push(lines);
                self.current_layout = Some(layout);
            }
        }
    }
}
