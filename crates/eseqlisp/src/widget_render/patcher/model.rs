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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Forward,
    Feedback,
}

#[derive(Debug, Clone)]
pub struct PatchConnection {
    pub from_node: String,
    pub from_output: usize,
    pub to_node: String,
    pub to_input: usize,
    pub kind: ConnectionKind,
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
