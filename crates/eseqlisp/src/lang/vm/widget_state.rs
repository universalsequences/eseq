//! `defwidget` instance state (kind-bindings spec §7.3).
//!
//! A `:state` name the shader reads dotted (`step.active`) names a kind
//! and declares instance state: the widget takes an instance as that prop
//! (`:step s`). A dotted read of a singleton kind (`transport.playing`) needs
//! no `:state` entry at all. The plan made here allocates one float uniform
//! per field the shader reads (three for `:rgb`) after the scalar states;
//! at construction [`VM::bind_sdf_widget_instance_fields`] fills those
//! uniforms' `shader-state-*` props with `#'` bindings to the fields' slots,
//! so a field write repaints the widget without re-running the view.
//!
//! A `:state` name the shader reads bare stays scalar even when a kind has
//! that name (legacy widgets with `:state (scene)` or `(track)`); reading
//! one name both ways is an error.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::{INSTANCE_DOC_READ_NATIVE, InstanceId, InstanceKindSchema, InstanceStore};
use crate::lang::modules::is_qualified;
use crate::lang::sdf_codegen::{
    builtin_shader_symbols, collect_state_symbols, uniform_identifier_collision, walk_free_symbols,
};
use crate::parser::Expression;
use crate::vm::{BindingKind, VM, VMError, Value};
use crate::widget_render::sdf_widget::{
    MAX_SDF_STATE_UNIFORMS, SdfFieldSource, SdfInstanceField, SdfWidgetDef,
};

/// The state uniforms of one `defwidget`: scalar states first, then the
/// instance fields, in the order the shader first reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SdfStatePlan {
    pub scalars: Vec<String>,
    pub fields: Vec<SdfInstanceField>,
}

impl SdfStatePlan {
    /// One float uniform name per slot, in slot order.
    pub fn uniforms(&self) -> Vec<String> {
        let mut uniforms = self.scalars.clone();
        for field in &self.fields {
            uniforms.extend(field.uniform_names());
        }
        uniforms
    }

    pub fn float_count(&self) -> usize {
        self.scalars.len()
            + self
                .fields
                .iter()
                .map(SdfInstanceField::float_count)
                .sum::<usize>()
    }

    /// `seed 1, step.active 1, track.color 3`.
    pub fn allocation(&self) -> String {
        self.scalars
            .iter()
            .map(|name| format!("{name} 1"))
            .chain(
                self.fields
                    .iter()
                    .map(|field| format!("{} {}", field.uniform, field.float_count())),
            )
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The error for a plan over the uniform budget, or `None` within it.
    /// `who` names the widget (or `material`, `sdf->metal`).
    pub fn budget_error(&self, who: &str) -> Option<String> {
        let count = self.float_count();
        (count > MAX_SDF_STATE_UNIFORMS).then(|| {
            format!(
                "{who}: shader state needs {count} floats, over the budget of \
                 {MAX_SDF_STATE_UNIFORMS}: {}",
                self.allocation()
            )
        })
    }
}

/// The field `symbol` reads of `head` (`active` for `step.active` and
/// `step`), or `None` when `symbol` is not `head.<field>`.
fn field_of<'a>(symbol: &'a str, head: &str) -> Option<&'a str> {
    symbol
        .strip_prefix(head)?
        .strip_prefix('.')
        .filter(|field| !field.is_empty())
}

/// Every symbol with a `.` the shader reads (`step.active`), in first-read
/// order, skipping qualified names (`m/f`) and names whose head a `let`
/// binds.
fn collect_dotted_symbols(shader: &Expression) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk_free_symbols(
        shader,
        &mut vec![builtin_shader_symbols()],
        &mut |name, scopes| {
            let Some((head, _)) = name.split_once('.') else {
                return;
            };
            if head.is_empty()
                || is_qualified(name)
                || scopes.iter().any(|scope| scope.contains(head))
                || out.iter().any(|seen| seen == name)
            {
                return;
            }
            out.push(name.to_string());
        },
    );
    out
}

impl VM {
    /// The kind a `defwidget` names (kind-bindings spec §7.3): the kind with
    /// that exact id, else the one kind whose name part is `name`. `Err`
    /// holds the candidates: none for an unknown name, several for an
    /// ambiguous one.
    fn widget_state_kind(&self, name: &str) -> Result<&InstanceKindSchema, Vec<String>> {
        if let Some(schema) = self.instances.kinds.get(name) {
            return Ok(schema);
        }
        let mut matches: Vec<&InstanceKindSchema> = self
            .instances
            .kinds_named(name)
            .map(|(_, schema)| schema)
            .collect();
        if matches.len() == 1 {
            return Ok(matches.remove(0));
        }
        let mut candidates: Vec<String> =
            matches.iter().map(|schema| schema.kind.clone()).collect();
        candidates.sort_unstable();
        Err(candidates)
    }

    /// Plan a `defwidget`'s state uniforms from its `:state` names and its
    /// macro-expanded shader (kind-bindings spec §7.3). Kinds resolve now,
    /// so a kind must be defined before the widget (host kinds load before
    /// any view). Errors name the widget. The budget is the caller's to
    /// check ([`SdfStatePlan::budget_error`]).
    pub fn plan_sdf_widget_state(
        &self,
        widget: &str,
        state_names: &[String],
        shader: &Expression,
    ) -> Result<SdfStatePlan, String> {
        let host_documents = self.instance_doc_native(INSTANCE_DOC_READ_NATIVE).is_some();
        let mut fields: Vec<SdfInstanceField> = Vec::new();
        // :state name -> kind id, resolved once per name.
        let mut instance_states: HashMap<&str, &str> = HashMap::new();
        for symbol in collect_dotted_symbols(shader) {
            // The longest :state name the symbol reads a field of.
            let state = state_names
                .iter()
                .filter(|name| field_of(&symbol, name).is_some())
                .max_by_key(|name| name.len());
            let (source, head, schema) = if let Some(state) = state {
                let schema = match instance_states.get(state.as_str()) {
                    Some(kind) => &self.instances.kinds[*kind],
                    None => {
                        let schema = self.widget_state_kind(state).map_err(|candidates| {
                            if candidates.is_empty() {
                                format!(
                                    "{widget}: {symbol} reads a field of :state '{state}', \
                                     but no kind is named '{state}' (define the kind before the widget)"
                                )
                            } else {
                                format!(
                                    "{widget}: :state '{state}' names several kinds ({}); use its kind id",
                                    candidates.join(", ")
                                )
                            }
                        })?;
                        instance_states.insert(state, &schema.kind);
                        schema
                    }
                };
                (SdfFieldSource::Prop(state.clone()), state.as_str(), schema)
            } else {
                // A singleton read without a :state entry: an exact kind id
                // prefix (`m:transport.playing`), else the name part.
                let exact = self
                    .instances
                    .kinds
                    .values()
                    .filter(|schema| field_of(&symbol, &schema.kind).is_some())
                    .max_by_key(|schema| schema.kind.len());
                let (head, schema) = match exact {
                    Some(schema) => (schema.kind.as_str(), schema),
                    None => {
                        let (head, _) = symbol.split_once('.').expect("dotted symbol");
                        match self.widget_state_kind(head) {
                            Ok(schema) => (head, schema),
                            // Not a kind: left to the shader compiler, as before.
                            Err(candidates) if candidates.is_empty() => continue,
                            Err(candidates) => {
                                return Err(format!(
                                    "{widget}: {symbol}: '{head}' names several kinds ({}); use its kind id",
                                    candidates.join(", ")
                                ));
                            }
                        }
                    }
                };
                if !schema.is_singleton() {
                    return Err(format!(
                        "{widget}: {symbol} reads kind '{}', which is not a singleton; \
                         add {head} to :state and pass the instance (:{head} x)",
                        schema.kind
                    ));
                }
                (SdfFieldSource::Singleton(schema.kind.clone()), head, schema)
            };
            let field = field_of(&symbol, head).expect("symbol reads a field of its head");
            if field.contains('.') {
                return Err(format!(
                    "{widget}: {symbol}: a shader reads one field ({head}.field), not a path"
                ));
            }
            let binding = InstanceStore::field_binding(schema, 0, field, host_documents)
                .map_err(|reason| format!("{widget}: {reason}"))?;
            fields.push(SdfInstanceField::new(
                source,
                schema.kind.clone(),
                field.to_string(),
                symbol.clone(),
                matches!(binding, BindingKind::InstanceRgb(_)),
            ));
        }

        // A name read both bare and dotted is ambiguous.
        let heads: HashSet<String> = instance_states
            .keys()
            .map(|name| name.to_string())
            .collect();
        if let Some(bare) = collect_state_symbols(shader, &heads).first() {
            let dotted = fields
                .iter()
                .find(|field| field.source == SdfFieldSource::Prop(bare.clone()))
                .map(|field| field.uniform.as_str())
                .unwrap_or_default();
            return Err(format!(
                "{widget}: :state '{bare}' is read both as a number ({bare}) and as an instance ({dotted})"
            ));
        }

        let mut scalar_names: HashSet<String> = self.state_bindings.keys().cloned().collect();
        scalar_names.extend(state_names.iter().cloned());
        for head in &heads {
            scalar_names.remove(head);
        }
        let plan = SdfStatePlan {
            scalars: collect_state_symbols(shader, &scalar_names),
            fields,
        };
        if let Some((first, second)) = uniform_identifier_collision(&plan.uniforms()) {
            return Err(format!(
                "{widget}: state '{first}' and '{second}' name the same shader uniform; rename one"
            ));
        }
        Ok(plan)
    }

    /// Bind a constructed widget's instance field uniforms (kind-bindings
    /// spec §7.3): the `defwidget` constructor and a `box :background` call
    /// this with the widget's props. After a kind hot reload the plan is
    /// re-checked against the current kinds: a change to the fields the
    /// compiled shader reads (a field turned from `:rgb` to `:number`, a
    /// kind no longer a singleton) is an error asking to re-evaluate the
    /// `defwidget`.
    pub fn bind_sdf_widget_instance_fields(
        &mut self,
        def: &SdfWidgetDef,
        props: &mut HashMap<String, Rc<RefCell<Value>>>,
    ) -> Result<(), VMError> {
        if def.state.plan.fields.is_empty() {
            return Ok(());
        }
        self.recheck_sdf_widget_kinds(&def.name, def)?;
        self.sdf_instance_field_props(&def.name, &def.state.plan.fields, props)
    }

    /// Re-plan `def` when a kind changed since it was planned; the plan's
    /// fields must come out the same.
    fn recheck_sdf_widget_kinds(&self, widget: &str, def: &SdfWidgetDef) -> Result<(), VMError> {
        let generation = self.instance_kind_schema_generation();
        if def.state.kind_generation.get() == generation {
            return Ok(());
        }
        let plan = self
            .plan_sdf_widget_state(widget, &def.state.names, &def.sdf_expr)
            .map_err(VMError::Instance)?;
        let old = &def.state.plan.fields;
        if let Some(index) = (0..old.len().max(plan.fields.len()))
            .find(|&index| old.get(index) != plan.fields.get(index))
        {
            let kind = old
                .get(index)
                .or_else(|| plan.fields.get(index))
                .map(|field| field.kind.as_str())
                .unwrap_or_default();
            return Err(VMError::Instance(format!(
                "{widget}: kind '{kind}' changed since defwidget; re-evaluate it"
            )));
        }
        def.state.kind_generation.set(generation);
        Ok(())
    }

    /// The instance a field of `widget` reads: the singleton, or the
    /// instance prop (`None` while missing or nil). An instance of another
    /// kind, or any other value, is an error.
    fn sdf_field_instance(
        &self,
        widget: &str,
        field: &SdfInstanceField,
        props: &HashMap<String, Rc<RefCell<Value>>>,
    ) -> Result<Option<InstanceId>, VMError> {
        let name = match &field.source {
            SdfFieldSource::Singleton(kind) => return Ok(self.singleton_instance(kind)),
            SdfFieldSource::Prop(name) => name,
        };
        let wrong = |got: &Value| {
            VMError::Instance(format!(
                "{widget}: :{name} takes an instance of kind '{}'; got {}",
                field.kind,
                self.format_value(got)
            ))
        };
        let Some(cell) = props.get(name) else {
            return Ok(None);
        };
        let id = match &*cell.borrow() {
            Value::Nil => return Ok(None),
            Value::Instance(id) => *id,
            other => return Err(wrong(other)),
        };
        match self.instance_kind(id) {
            Some(kind) if kind != field.kind => Err(wrong(&Value::Instance(id))),
            Some(_) => Ok(Some(id)),
            None => Ok(None),
        }
    }

    /// Fill a widget's instance field uniforms: each field's
    /// `shader-state-*` prop becomes a binding to the field's slot
    /// (`#'s.active`), created and seeded as `#'` does, so the host sees
    /// the field observed and a write repaints the widget only. An `:rgb`
    /// field gets one binding per component slot. A missing or nil instance
    /// prop, or a dropped instance, leaves its uniforms at 0; an instance of
    /// another kind is an error.
    fn sdf_instance_field_props(
        &mut self,
        widget: &str,
        fields: &[SdfInstanceField],
        props: &mut HashMap<String, Rc<RefCell<Value>>>,
    ) -> Result<(), VMError> {
        // The instance of each source, looked up and checked once.
        let mut instances: Vec<(&SdfFieldSource, Option<InstanceId>)> = Vec::new();
        for field in fields {
            let id = match instances
                .iter()
                .find(|(source, _)| **source == field.source)
            {
                Some((_, id)) => *id,
                None => {
                    let id = self.sdf_field_instance(widget, field, props)?;
                    instances.push((&field.source, id));
                    id
                }
            };
            let Some(id) = id else {
                continue;
            };
            let refs = self.instance_field_refs(id, &field.field, widget)?;
            if refs.len() != field.prop_names.len() {
                return Err(VMError::Instance(format!(
                    "{widget}: kind '{}' changed since defwidget; re-evaluate it",
                    field.kind
                )));
            }
            for (name, binding) in field.prop_names.iter().zip(refs) {
                props.insert(name.clone(), Rc::new(RefCell::new(binding)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "widget_state_tests.rs"]
mod tests;
