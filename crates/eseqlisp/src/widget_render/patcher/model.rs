use std::collections::HashSet;

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
    pub to_arg: ArgSource,
    pub previous_arg: SourceArgValue,
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
    source
        .param
        .as_ref()
        .map(|param| InlineInput::RawParam(param.name.clone()))
}

fn inline_mod_param(patch: &Patch, connection: &PatchConnection) -> Option<InlineInput> {
    let mod_node = patch
        .nodes
        .iter()
        .find(|node| node.id == connection.from_node)?;
    if mod_node.op != "mod" {
        return None;
    }
    if !matches!(
        mod_node.source.as_ref().map(|source| &source.owner),
        Some(SourceOwner::NestedExpr { .. })
    ) {
        return None;
    }
    let inbound = patch
        .connections
        .iter()
        .find(|candidate| candidate.to_node == mod_node.id && candidate.to_input == 0)?;
    let param = patch
        .nodes
        .iter()
        .find(|node| node.id == inbound.from_node)?;
    let param = param.param.as_ref()?;
    param
        .modulatable
        .then(|| InlineInput::ModParam(param.name.clone()))
}
