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

#[derive(Debug, Clone)]
pub struct PatchConnection {
    pub from_node: String,
    pub from_output: usize,
    pub to_node: String,
    pub to_input: usize,
    pub kind: ConnectionKind,
    pub segment: Option<CableSegmentInfo>,
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
    pub patch: Patch,
}

#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub nodes: Vec<PatchNode>,
    pub connections: Vec<PatchConnection>,
    pub macros: Vec<MacroPatch>,
    pub diagnostics: Vec<String>,
}
