use std::collections::HashSet;

use super::lisp::param_short_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatcherIntent {
    Instrument,
    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Builtin,
    Param,
    In,
    Out,
    History,
    Constant,
    MacroDefinition,
    MacroInstance,
    CodeIsland,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArgValue {
    Literal(String),
    SymbolRef(String),
    ConnectedExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamNodeInfo {
    pub name: String,
    pub modulatable: bool,
    /// The authored tensor binding behind `@options`, when projection resolved it
    /// into an options cable. This survives graph serialization so deleting that
    /// cable removes the attribute instead of exposing stale source text.
    pub options_tensor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineInput {
    RawParam(String),
    ModParam(String),
}

impl InlineInput {
    pub fn label(&self) -> String {
        match self {
            InlineInput::RawParam(name) => name.clone(),
            InlineInput::ModParam(name) => format!("{name}~"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceScopeId {
    Root,
    Macro { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceFormId {
    pub scope: SourceScopeId,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceExprId {
    pub form_id: SourceFormId,
    pub path: ExprPath,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ExprPath(pub Vec<ExprPathSegment>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprPathSegment {
    ListItem(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSourceShape {
    pub call: SourceExprId,
    pub positional_args: Vec<ArgSource>,
    pub attributes: Vec<AttributeSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgSource {
    pub semantic_index: usize,
    pub item_index: usize,
    pub expr: SourceExprId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeSource {
    pub key_item_index: usize,
    pub value_item_index: usize,
    /// Items the value spans. Always 1 except for bracketed arrays (`@shape [3 3]`), which
    /// lex as a run of tokens because `[`/`]` are not lexer delimiters.
    pub value_item_count: usize,
    pub key: String,
    pub value: SourceExprId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSource {
    pub owner: SourceOwner,
    pub expr: Option<SourceExprId>,
    pub call_shape: Option<CallSourceShape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceOwner {
    TopLevelForm {
        form_id: SourceFormId,
    },
    BindingValue {
        form_id: SourceFormId,
        binding: BindingTarget,
        value_path: ExprPath,
    },
    NestedExpr {
        expr: SourceExprId,
    },
    ArgumentSlot {
        call: SourceExprId,
        arg: ArgSource,
    },
    SymbolReference {
        call: SourceExprId,
        arg: ArgSource,
        symbol: String,
        resolved_binding: Option<BindingId>,
    },
    MacroParameter {
        binding: BindingId,
        index: usize,
    },
    Compound {
        parts: Vec<SourceOwner>,
    },
    CodeIsland {
        form_id: SourceFormId,
    },
    Created {
        created_id: String,
        generated_binding: Option<BindingId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingTarget {
    Symbol(String),
    Destructuring(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BindingId {
    pub scope: SourceScopeId,
    pub name: String,
    pub kind: BindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingKind {
    Def,
    Param,
    History,
    MacroParam,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSource {
    pub from_expr: Option<SourceExprId>,
    pub to_call: SourceExprId,
    pub target: ConnectionSourceTarget,
    pub previous_arg: SourceArgValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionSourceTarget {
    Argument(ArgSource),
    Attribute(AttributeSource),
}

impl ConnectionSourceTarget {
    pub fn expr(&self) -> &SourceExprId {
        match self {
            Self::Argument(arg) => &arg.expr,
            Self::Attribute(attribute) => &attribute.value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceArgValue {
    Literal(SourceExprId),
    SymbolReference {
        expr: SourceExprId,
        symbol: String,
        resolved_binding: Option<BindingId>,
    },
    NestedExpression(SourceExprId),
    Missing,
}

#[derive(Debug, Clone)]
pub struct PatchNode {
    pub id: String,
    pub op: String,
    pub kind: NodeKind,
    pub label: String,
    pub args: Vec<ArgValue>,
    pub outputs: Vec<String>,
    pub position: (f32, f32),
    pub width: Option<f32>,
    pub param: Option<ParamNodeInfo>,
    pub inline_inputs: Vec<Option<InlineInput>>,
    pub diagnostic: Option<String>,
    pub source: Option<NodeSource>,
    /// Projector-synthesized helper node (today: the hidden `(mod p)` accessor
    /// behind the `p~` sugar) rather than something the user authored.
    ///
    /// When a patch is projected from source this is implied by the node's
    /// `SourceOwner::NestedExpr` owner. A patch deserialized from a graph
    /// payload has no source data at all (spec §4.1a/§4.1c), so the fact has to
    /// be carried explicitly — it is semantic, not positional.
    pub synthesized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Forward,
    Feedback,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CableSegmentInfo {
    pub is_segmented: bool,
    pub segment_row: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputPresentation {
    Cable,
    InlineRawParam,
    InlineModParam,
}

/// The sole semantic inlet on a param node. A cable here declares
/// `@options <source-binding>` rather than a positional DSP input.
pub const PARAM_OPTIONS_INPUT: usize = 0;

pub fn is_tensor_options_source(node: &PatchNode) -> bool {
    matches!(node.op.as_str(), "tensor" | "tensor-param")
}

pub fn is_param_options_connection(patch: &Patch, connection: &PatchConnection) -> bool {
    connection.to_input == PARAM_OPTIONS_INPUT
        && patch
            .nodes
            .iter()
            .find(|node| node.id == connection.to_node)
            .is_some_and(|node| node.kind == NodeKind::Param)
}

pub fn options_connection_is_valid(patch: &Patch, connection: &PatchConnection) -> bool {
    if !is_param_options_connection(patch, connection) {
        return true;
    }
    connection.from_output == 0
        && patch
            .nodes
            .iter()
            .find(|node| node.id == connection.from_node)
            .is_some_and(is_tensor_options_source)
}

#[derive(Debug, Clone)]
pub struct PatchConnection {
    pub from_node: String,
    pub from_output: usize,
    pub to_node: String,
    pub to_input: usize,
    pub kind: ConnectionKind,
    pub segment: Option<CableSegmentInfo>,
    pub presentation: InputPresentation,
    pub presentation_override: Option<InputPresentation>,
    pub source: Option<ConnectionSource>,
    /// The spelling the author used to name the param this cable carries, when
    /// the cable has no `source` to read it from.
    ///
    /// A param reference typed as `name~` sugar is desugared in the model
    /// (`desugar_editor_mod_suffix_args`), which replaces the argument with a
    /// synthesized accessor and a pair of fresh cables — none of which point at
    /// a source expression. Without this, an inlet had nothing authored to echo
    /// and fell back to the shortest unambiguous spelling, so a typed
    /// `fm.harmonicity~` re-rendered as `harmonicity~`. Only the desugar sets
    /// it; a cable backed by real source text carries its spelling in `source`.
    pub authored_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutputPortRef {
    pub(super) node_id: String,
    pub(super) output_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputPortRef {
    pub(super) node_id: String,
    pub(super) input_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CableEndpoint {
    From,
    To,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OperatorPortShape {
    pub(super) input_count: usize,
    pub(super) output_count: usize,
}

#[derive(Debug, Clone)]
pub struct MacroPatch {
    pub name: String,
    pub params: Vec<String>,
    pub outputs: Vec<String>,
    pub patch: Patch,
    pub origin: MacroOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroOrigin {
    Local,
    Library {
        source_path: String,
        layout_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroSignature {
    pub params: Vec<String>,
    pub outputs: Vec<String>,
}

/// A host-modulator input def (`(def modN (in ch @name modN @modulator N))`)
/// that the projector hides from the canvas. The generator re-emits these from
/// the model so full regeneration does not drop them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostModulatorInput {
    pub name: String,
    pub channel: usize,
    pub slot: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub nodes: Vec<PatchNode>,
    pub connections: Vec<PatchConnection>,
    pub macros: Vec<MacroPatch>,
    pub diagnostics: Vec<String>,
    pub host_modulators: Vec<HostModulatorInput>,
    /// `(use-defmacro name)` headers the source declared, in source order.
    ///
    /// The generator normally rebuilds the import block from the `Library`
    /// entries in `macros`, but those only exist when the patch was parsed with
    /// a defmacro library in hand. Parsed without one (no library root on disk,
    /// or one of the library-less parse paths), every imported macro call
    /// degrades to an unknown-operator `Builtin` and regeneration would drop
    /// the header — silently breaking the file for the compiler, which
    /// materializes library defmacros from these headers alone. Keeping the
    /// raw names in the model makes the round-trip lossless either way.
    pub imports: Vec<String>,
}

pub fn refresh_patch_inline_inputs(patch: &mut Patch) {
    for node in &mut patch.nodes {
        node.inline_inputs = vec![None; node.args.len()];
    }

    let connections = patch.connections.clone();
    for connection in connections {
        if connection.presentation == InputPresentation::Cable {
            continue;
        }
        let inline = match connection.presentation {
            InputPresentation::InlineRawParam => inline_raw_param(patch, &connection),
            InputPresentation::InlineModParam => inline_mod_param(patch, &connection),
            InputPresentation::Cable => unreachable!(),
        };
        let Some(inline) = inline else {
            if let Some(connection) = patch.connections.iter_mut().find(|candidate| {
                candidate.from_node == connection.from_node
                    && candidate.from_output == connection.from_output
                    && candidate.to_node == connection.to_node
                    && candidate.to_input == connection.to_input
            }) {
                connection.presentation = InputPresentation::Cable;
                connection.presentation_override = None;
            }
            continue;
        };
        let Some(node) = patch
            .nodes
            .iter_mut()
            .find(|node| node.id == connection.to_node)
        else {
            continue;
        };
        if node.inline_inputs.len() <= connection.to_input {
            node.inline_inputs.resize(connection.to_input + 1, None);
        }
        node.inline_inputs[connection.to_input] = Some(inline);
    }
}

pub fn hidden_inline_node_ids(patch: &Patch) -> HashSet<String> {
    patch
        .connections
        .iter()
        .filter(|connection| connection.presentation == InputPresentation::InlineModParam)
        .filter(|connection| inline_mod_param(patch, connection).is_some())
        .map(|connection| connection.from_node.clone())
        .collect()
}

pub fn connection_touches_hidden_inline_node(
    connection: &PatchConnection,
    hidden_node_ids: &HashSet<String>,
) -> bool {
    hidden_node_ids.contains(&connection.from_node) || hidden_node_ids.contains(&connection.to_node)
}

fn inline_raw_param(patch: &Patch, connection: &PatchConnection) -> Option<InlineInput> {
    let source = patch
        .nodes
        .iter()
        .find(|node| node.id == connection.from_node)?;
    let param = source.param.as_ref()?;
    Some(InlineInput::RawParam(param_display_reference(
        patch,
        param,
        authored_symbol(connection),
    )))
}

fn inline_mod_param(patch: &Patch, connection: &PatchConnection) -> Option<InlineInput> {
    let (param, inbound) = inline_mod_accessor_param(patch, &connection.from_node)?;
    Some(InlineInput::ModParam(param_display_reference(
        patch,
        param,
        authored_symbol(inbound),
    )))
}

fn authored_symbol(connection: &PatchConnection) -> Option<&str> {
    if let Some(authored) = connection.authored_reference.as_deref() {
        return Some(authored);
    }
    match &connection.source.as_ref()?.previous_arg {
        SourceArgValue::SymbolReference { symbol, .. } => Some(symbol.as_str()),
        _ => None,
    }
}

/// How a reference to `param` should read on a consumer node's inlet.
///
/// Params are identified by their group-qualified name, but the source may
/// reference them either way — `attack` resolves to `amp.attack` as long as no
/// other group declares an `attack`. Echoing the authored spelling back keeps
/// retype/writeback round-trips lossless in both directions; a connection with
/// no authored symbol behind it (a sidecar-restored or freshly dragged cable)
/// falls back to the shortest form that is still unambiguous.
fn param_display_reference(patch: &Patch, param: &ParamNodeInfo, authored: Option<&str>) -> String {
    let short = param_short_name(&param.name);
    if let Some(authored) = authored
        && (authored == param.name || authored == short)
    {
        return authored.to_string();
    }
    let ambiguous = patch
        .nodes
        .iter()
        .filter_map(|node| node.param.as_ref())
        .filter(|other| !std::ptr::eq(*other, param))
        .any(|other| param_short_name(&other.name) == short);
    if ambiguous {
        param.name.clone()
    } else {
        short.to_string()
    }
}

/// Is `node_id` a projector-synthesized `(mod param)` accessor — the nested
/// expression behind the `param~` sugar? Such a node exists only to serve the
/// expression of the node that consumes it; the user never authored it and
/// never sees it on the canvas. Returns the modulatable param it reads.
fn inline_mod_accessor_param<'a>(
    patch: &'a Patch,
    node_id: &str,
) -> Option<(&'a ParamNodeInfo, &'a PatchConnection)> {
    let mod_node = patch.nodes.iter().find(|node| node.id == node_id)?;
    if mod_node.op != "mod" {
        return None;
    }
    // A user-authored `(def m (mod gain))` is a BindingValue, not a nested
    // expression: it is a real node and must never be garbage-collected.
    let is_synthesized = mod_node.synthesized
        || matches!(
            mod_node.source.as_ref().map(|source| &source.owner),
            Some(SourceOwner::NestedExpr { .. })
        );
    if !is_synthesized {
        return None;
    }
    let inbound = patch
        .connections
        .iter()
        .find(|candidate| candidate.to_node == mod_node.id && candidate.to_input == 0)?;
    let param = patch
        .nodes
        .iter()
        .find(|node| node.id == inbound.from_node)?
        .param
        .as_ref()?;
    param.modulatable.then_some((param, inbound))
}

/// Synthesized `param~` accessors that no longer have any consumer — their
/// only reason to exist died with the node whose expression contained them.
/// See docs/patch-vs-code-editor-spec.md §4.2b: synthesized helper nodes are
/// never persisted when orphaned.
pub fn orphaned_inline_mod_node_ids(patch: &Patch) -> HashSet<String> {
    patch
        .nodes
        .iter()
        .filter(|node| {
            !patch
                .connections
                .iter()
                .any(|connection| connection.from_node == node.id)
        })
        .filter(|node| inline_mod_accessor_param(patch, &node.id).is_some())
        .map(|node| node.id.clone())
        .collect()
}
