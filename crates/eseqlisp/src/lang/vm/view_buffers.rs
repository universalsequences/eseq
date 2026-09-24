//! Host-bound view buffers (instance-kinds spec §7).
//!
//! `effect-buffer` binds a buffer name that must be a literal at compile
//! time. A host that owns instances of a kind needs one buffer per instance,
//! named at runtime, rendering `(view self)`. [`VM::bind_view_buffer`] makes
//! exactly that: an ordinary top-level named effect whose body is a tiny
//! synthesized chunk (`EffectBegin; CallBoundView; EmitTree; EffectEnd`), so
//! read tracking, subtree caching, hidden-buffer deferral and error reporting
//! are the ones `effect-buffer` already has.
//!
//! The view is either an instance (the kind's current `:view` is looked up at
//! every render, so a hot-reloaded `def-kind` re-renders through the new
//! function) or an arbitrary callable with arguments.
//!
//! A binding may carry a **key scope**. While an effect renders into a scoped
//! target, every widget `:key` and every subtree key is prefixed with the
//! scope (`instance:3::weight-matrix`), so identical authored keys in two
//! instances' views never share a stable key, a widget id or a subtree cache
//! entry, even if both views were ever rendered into one tree.

use std::collections::HashMap;

use super::{
    Chunk, EffectTarget, HashSet, InstanceId, NodeId, OpCode, PendingUiUpdate, ReactiveNode, VM,
    VMError, Value, clone_value_for_snapshot,
};

/// What a bound view buffer renders.
#[derive(Clone, Debug)]
pub enum BoundView {
    /// `(view instance)`, with `view` resolved from the instance's kind at
    /// every render.
    Instance(InstanceId),
    /// `(callable args...)`.
    Call { callable: Value, args: Vec<Value> },
}

impl BoundView {
    fn snapshot(&self) -> Self {
        match self {
            Self::Instance(id) => Self::Instance(*id),
            Self::Call { callable, args } => Self::Call {
                callable: clone_value_for_snapshot(callable),
                args: args.iter().map(clone_value_for_snapshot).collect(),
            },
        }
    }
}

#[derive(Clone)]
struct BoundViewBuffer {
    target: String,
    chunk_idx: usize,
    view: BoundView,
    key_scope: Option<String>,
}

#[derive(Default)]
pub(crate) struct ViewBufferStore {
    by_node: HashMap<NodeId, BoundViewBuffer>,
    by_target: HashMap<String, NodeId>,
}

impl ViewBufferStore {
    pub(crate) fn snapshot(&self) -> Self {
        Self {
            by_node: self
                .by_node
                .iter()
                .map(|(id, bound)| {
                    (
                        *id,
                        BoundViewBuffer {
                            target: bound.target.clone(),
                            chunk_idx: bound.chunk_idx,
                            view: bound.view.snapshot(),
                            key_scope: bound.key_scope.clone(),
                        },
                    )
                })
                .collect(),
            by_target: self.by_target.clone(),
        }
    }
}

/// Join a key scope and an authored key.
pub(crate) fn scoped_key(scope: &str, key: &str) -> String {
    format!("{scope}::{key}")
}

impl VM {
    /// Bind the buffer `target` to `view` and schedule its first render
    /// (the host's next reactive cycle runs it, unless the buffer is hidden,
    /// in which case it renders once shown). Rebinding a bound target
    /// replaces its view and scope in place. Returns the effect node.
    pub fn bind_view_buffer(
        &mut self,
        target: &str,
        view: BoundView,
        key_scope: Option<String>,
    ) -> NodeId {
        if let Some(node_id) = self.view_buffers.by_target.get(target).copied() {
            let scope_changed = self
                .view_buffers
                .by_node
                .get(&node_id)
                .is_some_and(|bound| bound.key_scope != key_scope);
            if scope_changed {
                // Scoped subtree keys hash into their root ids.
                self.clear_subtree_effects_for_named_target(target);
            }
            if let Some(bound) = self.view_buffers.by_node.get_mut(&node_id) {
                bound.view = view;
                bound.key_scope = key_scope;
            }
            self.dag.mark_dirty(node_id);
            return node_id;
        }
        let node_id = self.dag.alloc_id();
        let chunk_idx = self.chunks.len();
        self.chunks.push(Chunk {
            ops: vec![
                OpCode::EffectBegin(node_id),
                OpCode::CallBoundView(node_id),
                OpCode::EmitTree,
                OpCode::EffectEnd(node_id),
                OpCode::PushNil,
                OpCode::Return,
            ],
            constants: Vec::new(),
            strings: Vec::new(),
            symbols: Vec::new(),
            upvalues: Vec::new(),
            source_symbol: Some(format!("view-buffer {target}")),
            source_file: None,
            source_module: None,
        });
        // No owner buffer and no source file: a bound view belongs to the
        // host, so neither a buffer's layout reset nor a module reload
        // clears it. Only `unbind_view_buffer` does.
        self.dag.add_node(ReactiveNode::Effect {
            id: node_id,
            chunk_idx,
            callable: None,
            source_buffer_id: None,
            source_file: None,
            source_revision: None,
            target: EffectTarget::BufferName(target.to_string()),
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            symbol_dependencies: HashSet::new(),
            dirty: false,
        });
        self.view_buffers.by_node.insert(
            node_id,
            BoundViewBuffer {
                target: target.to_string(),
                chunk_idx,
                view,
                key_scope,
            },
        );
        self.view_buffers
            .by_target
            .insert(target.to_string(), node_id);
        self.dag.mark_dirty(node_id);
        node_id
    }

    /// Remove the binding of `target`: its effect, its keyed subtrees and any
    /// queued render. Returns whether it was bound.
    pub fn unbind_view_buffer(&mut self, target: &str) -> bool {
        let Some(node_id) = self.view_buffers.by_target.remove(target) else {
            return false;
        };
        let bound = self.view_buffers.by_node.remove(&node_id);
        self.dag.remove_node(node_id);
        self.drop_view_target_renders(target);
        // The synthesized chunk stays (chunk indices are positional); make
        // it inert so a stray run cannot render into a dead target.
        if let Some(chunk) = bound.and_then(|bound| self.chunks.get_mut(bound.chunk_idx))
            && chunk
                .ops
                .iter()
                .any(|op| matches!(op, OpCode::CallBoundView(id) if *id == node_id))
        {
            chunk.ops = vec![OpCode::PushNil, OpCode::Return];
        }
        true
    }

    /// Move a binding from `old` to `new` (an instance rename): the same
    /// effect re-renders into `new`. Fails when `old` is unbound or `new` is
    /// already bound.
    pub fn retarget_view_buffer(&mut self, old: &str, new: &str) -> bool {
        if old == new {
            return self.view_buffers.by_target.contains_key(old);
        }
        if self.view_buffers.by_target.contains_key(new) {
            return false;
        }
        let Some(node_id) = self.view_buffers.by_target.remove(old) else {
            return false;
        };
        self.drop_view_target_renders(old);
        if let Some(bound) = self.view_buffers.by_node.get_mut(&node_id) {
            bound.target = new.to_string();
        }
        if let Some(ReactiveNode::Effect { target, .. }) = self.dag.nodes.get_mut(&node_id) {
            *target = EffectTarget::BufferName(new.to_string());
        }
        self.view_buffers.by_target.insert(new.to_string(), node_id);
        self.dag.mark_dirty(node_id);
        true
    }

    /// Every bound target with what it renders, sorted by target.
    pub fn bound_view_buffers(&self) -> Vec<(String, BoundView)> {
        let mut bound: Vec<(String, BoundView)> = self
            .view_buffers
            .by_node
            .values()
            .map(|bound| (bound.target.clone(), bound.view.clone()))
            .collect();
        bound.sort_by(|a, b| a.0.cmp(&b.0));
        bound
    }

    /// The key scope of the effect target currently rendering, if any.
    pub(crate) fn current_key_scope(&self) -> Option<&str> {
        let EffectTarget::BufferName(name) = &self.current_effect_target else {
            return None;
        };
        let node_id = self.view_buffers.by_target.get(name)?;
        self.view_buffers.by_node.get(node_id)?.key_scope.as_deref()
    }

    /// `key` inside the current key scope (unchanged outside one).
    pub(crate) fn scope_key_for_current_target(&self, key: String) -> String {
        match self.current_key_scope() {
            Some(scope) if !key.starts_with(&format!("{scope}::")) => scoped_key(scope, &key),
            _ => key,
        }
    }

    /// `CallBoundView`: render the bound view of `node_id`.
    pub(super) fn call_bound_view(&mut self, node_id: NodeId) -> Result<Value, VMError> {
        let Some(bound) = self.view_buffers.by_node.get(&node_id) else {
            return Ok(Value::Nil);
        };
        let (callable, args) = match &bound.view {
            BoundView::Instance(id) => {
                let id = *id;
                match self.instance_view_callable(id)? {
                    Some(view) => (view, vec![Value::Instance(id)]),
                    None => return Ok(Value::Nil),
                }
            }
            BoundView::Call { callable, args } => (callable.clone(), args.clone()),
        };
        Ok(self.invoke(callable, args)?.unwrap_or(Value::Nil))
    }

    /// Dirty the bound views of every live instance of `kind` (its schema,
    /// and so possibly its `:view`, was just (re)registered).
    pub(super) fn mark_instance_views_of_kind_dirty(&mut self, kind: &str) {
        if self.view_buffers.by_node.is_empty() {
            return;
        }
        let ids: HashSet<InstanceId> = self.live_instances_of_kind(kind).into_iter().collect();
        let nodes: Vec<NodeId> = self
            .view_buffers
            .by_node
            .iter()
            .filter(|(_, bound)| matches!(bound.view, BoundView::Instance(id) if ids.contains(&id)))
            .map(|(node_id, _)| *node_id)
            .collect();
        for node_id in nodes {
            self.dag.mark_dirty(node_id);
        }
    }

    /// Forget everything rendered for `target`: keyed subtree effects, their
    /// caches and queued trees.
    fn drop_view_target_renders(&mut self, target: &str) {
        self.clear_subtree_effects_for_named_target(target);
        let target = EffectTarget::BufferName(target.to_string());
        self.pending_widget_trees
            .retain(|pending: &PendingUiUpdate| *pending.target() != target);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        EffectTarget, InstanceKindSchema, PendingUiUpdate, VM, Value, register_core_natives,
    };
    use super::BoundView;

    const KIND: &str = "test/pkg:probe";

    fn eval(vm: &mut VM, code: &str) -> Option<Value> {
        vm.eval_str(code).unwrap_or_else(|e| panic!("{code}: {e:?}"))
    }

    fn view_vm(view_source: &str) -> VM {
        let mut vm = VM::new(Vec::new());
        register_core_natives(&mut vm);
        crate::widgets::register_widget_natives(&mut vm);
        eval(&mut vm, view_source);
        let view = vm.global_value("probe-view").expect("probe-view defined");
        vm.register_instance_kind(
            InstanceKindSchema::new(KIND)
                .field("x", Value::Number(1.0))
                .with_view(Some(view)),
        )
        .expect("register kind");
        vm
    }

    const VIEW: &str = r#"
        (def probe-view (s)
          (v-stack
            (box :key "weight-matrix" :width s.x :height 1)
            (subtree :key "panel" (box :width 2 :height 1))))
    "#;

    /// Full trees rendered since the last drain, by target, sorted.
    fn drain_full_trees(vm: &mut VM) -> Vec<(String, Value)> {
        let mut trees: Vec<(String, Value)> = vm
            .pending_widget_trees
            .drain(..)
            .filter_map(|update| match update {
                PendingUiUpdate::FullTree(tree) => match tree.target {
                    EffectTarget::BufferName(name) => Some((name, tree.tree)),
                    _ => None,
                },
                PendingUiUpdate::ReplaceSubtree { .. } => None,
            })
            .collect();
        trees.sort_by(|a, b| a.0.cmp(&b.0));
        trees
    }

    fn targets(trees: &[(String, Value)]) -> Vec<&str> {
        trees.iter().map(|(name, _)| name.as_str()).collect()
    }

    fn collect(value: &Value, prop: &str, out: &mut Vec<Value>) {
        let Value::Map(map) = value else {
            return;
        };
        if let Some(found) = map.get(prop) {
            out.push(found.borrow().clone());
        }
        if let Some(children) = map.get("children")
            && let Value::List(children) = &*children.borrow()
        {
            for child in children {
                collect(&child.borrow(), prop, out);
            }
        }
    }

    fn stable_keys(tree: &Value) -> Vec<String> {
        let mut out = Vec::new();
        collect(tree, "__stable-key", &mut out);
        out.into_iter()
            .filter_map(|value| match value {
                Value::String(key) => Some(key),
                _ => None,
            })
            .collect()
    }

    fn widths(tree: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        collect(tree, "width", &mut out);
        out
    }

    #[test]
    fn instance_views_render_per_buffer_and_rerender_only_their_own_instance() {
        let mut vm = view_vm(VIEW);
        vm.create_instance(1, KIND).expect("create 1");
        vm.create_instance(2, KIND).expect("create 2");
        vm.bind_view_buffer("*probe · A*", BoundView::Instance(1), Some("instance:1".into()));
        vm.bind_view_buffer("*probe · B*", BoundView::Instance(2), Some("instance:2".into()));
        vm.process_dirty_reactive().expect("first render");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(targets(&trees), vec!["*probe · A*", "*probe · B*"]);

        // Same authored keys, scoped per instance.
        let keys_a = stable_keys(&trees[0].1);
        let keys_b = stable_keys(&trees[1].1);
        assert!(keys_a.contains(&"instance:1::weight-matrix".to_string()), "{keys_a:?}");
        assert!(keys_a.contains(&"instance:1::panel".to_string()), "{keys_a:?}");
        assert!(keys_b.contains(&"instance:2::weight-matrix".to_string()), "{keys_b:?}");
        assert!(keys_a.iter().all(|key| !keys_b.contains(key)));

        // A field write re-renders only the view that read it.
        vm.set_instance_field(2, "x", Value::Number(5.0)).expect("write");
        vm.process_dirty_reactive().expect("rerender");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(targets(&trees), vec!["*probe · B*"]);
        assert!(widths(&trees[0].1).contains(&Value::Number(5.0)));
    }

    #[test]
    fn rebinding_retargeting_and_unbinding() {
        let mut vm = view_vm(VIEW);
        vm.create_instance(1, KIND).expect("create");
        vm.bind_view_buffer("*old*", BoundView::Instance(1), Some("instance:1".into()));
        vm.process_dirty_reactive().expect("render");
        drain_full_trees(&mut vm);

        // Rename: the same effect renders into the new name only.
        assert!(vm.retarget_view_buffer("*old*", "*new*"));
        vm.process_dirty_reactive().expect("render new");
        assert_eq!(targets(&drain_full_trees(&mut vm)), vec!["*new*"]);
        assert_eq!(
            vm.bound_view_buffers()
                .into_iter()
                .map(|(target, view)| (target, matches!(view, BoundView::Instance(1))))
                .collect::<Vec<_>>(),
            vec![("*new*".to_string(), true)]
        );
        vm.set_instance_field(1, "x", Value::Number(3.0)).expect("write");
        vm.process_dirty_reactive().expect("render after write");
        assert_eq!(targets(&drain_full_trees(&mut vm)), vec!["*new*"]);

        // A retarget onto a bound name is refused.
        vm.bind_view_buffer("*other*", BoundView::Instance(1), None);
        assert!(!vm.retarget_view_buffer("*new*", "*other*"));

        // Unbind: nothing renders any more, even when its reads change.
        assert!(vm.unbind_view_buffer("*new*"));
        assert!(!vm.unbind_view_buffer("*new*"));
        vm.process_dirty_reactive().expect("drain other");
        drain_full_trees(&mut vm);
        vm.set_instance_field(1, "x", Value::Number(4.0)).expect("write");
        vm.process_dirty_reactive().expect("no render");
        assert_eq!(targets(&drain_full_trees(&mut vm)), vec!["*other*"]);
    }

    #[test]
    fn closure_views_take_arguments_and_kind_reload_rerenders_instances() {
        let mut vm = view_vm(VIEW);
        eval(&mut vm, "(def boxed (w h) (box :width w :height h))");
        let callable = vm.global_value("boxed").expect("boxed");
        vm.bind_view_buffer(
            "*call*",
            BoundView::Call {
                callable,
                args: vec![Value::Number(7.0), Value::Number(1.0)],
            },
            None,
        );
        vm.create_instance(1, KIND).expect("create");
        vm.bind_view_buffer("*inst*", BoundView::Instance(1), None);
        vm.process_dirty_reactive().expect("render");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(targets(&trees), vec!["*call*", "*inst*"]);
        assert_eq!(widths(&trees[0].1), vec![Value::Number(7.0)]);

        // Re-registering the kind with a new :view re-renders its
        // instances' buffers through it (hot reload), not the others.
        eval(&mut vm, "(def probe-view-2 (s) (box :width 11 :height 1))");
        let view = vm.global_value("probe-view-2").expect("view 2");
        vm.register_instance_kind(
            InstanceKindSchema::new(KIND)
                .field("x", Value::Number(1.0))
                .with_view(Some(view)),
        )
        .expect("re-register");
        vm.process_dirty_reactive().expect("rerender");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(targets(&trees), vec!["*inst*"]);
        assert_eq!(widths(&trees[0].1), vec![Value::Number(11.0)]);

        // A dropped instance's buffer renders empty until unbound.
        vm.drop_instance(1);
        vm.bind_view_buffer("*inst*", BoundView::Instance(1), None);
        vm.process_dirty_reactive().expect("stale render");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(trees, vec![("*inst*".to_string(), Value::Nil)]);
    }

    #[test]
    fn a_view_less_kind_reports_an_error_naming_the_kind() {
        let mut vm = VM::new(Vec::new());
        register_core_natives(&mut vm);
        crate::widgets::register_widget_natives(&mut vm);
        vm.register_instance_kind(InstanceKindSchema::new(KIND)).expect("register");
        vm.create_instance(1, KIND).expect("create");
        vm.bind_view_buffer("*v*", BoundView::Instance(1), None);
        let error = vm.process_dirty_reactive().expect_err("no view");
        assert!(format!("{error:?}").contains("has no :view"), "{error:?}");
    }

    #[test]
    fn lisp_binds_a_closure_to_a_computed_buffer_name() {
        let mut vm = view_vm(VIEW);
        eval(
            &mut vm,
            r#"
            (defstate w 3)
            (def boxed (extra) (box :width (+ w extra) :height 1))
            (bind-view-buffer (str "*dyn-" 1 "*") boxed 1)
            "#,
        );
        vm.process_dirty_reactive().expect("render");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(targets(&trees), vec!["*dyn-1*"]);
        assert_eq!(widths(&trees[0].1), vec![Value::Number(4.0)]);
        eval(&mut vm, "(set! w 5)");
        vm.process_dirty_reactive().expect("rerender");
        let trees = drain_full_trees(&mut vm);
        assert_eq!(targets(&trees), vec!["*dyn-1*"]);
        assert_eq!(widths(&trees[0].1), vec![Value::Number(6.0)]);
        assert_eq!(eval(&mut vm, "(unbind-view-buffer \"*dyn-1*\")"), Some(Value::Bool(true)));
        eval(&mut vm, "(set! w 6)");
        vm.process_dirty_reactive().expect("nothing");
        assert!(drain_full_trees(&mut vm).is_empty());
    }

    #[test]
    fn bindings_roll_back_with_a_state_snapshot() {
        let mut vm = view_vm(VIEW);
        vm.create_instance(1, KIND).expect("create");
        let snapshot = vm.snapshot_state();
        vm.bind_view_buffer("*v*", BoundView::Instance(1), None);
        assert_eq!(vm.bound_view_buffers().len(), 1);
        vm.restore_state(snapshot);
        assert!(vm.bound_view_buffers().is_empty());
    }
}
