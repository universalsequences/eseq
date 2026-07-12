//! Scheduler-owned process/channel runtime.
//!
//! Lisp authoring lives in `lisp_host`; this module owns the live musical-time
//! state: process instances, clocks, channels, patches, and pending process
//! emissions. It deliberately does not evaluate Lisp.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use eseqlisp::vm::Value;
use serde::{Deserialize, Serialize};

use crate::accumulator::ResolvedStep;
use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::lisp_host::EmittedAccumulatorEvent;
use crate::neural::ParamNodeId;
use crate::neural::{process_grid_boundaries, GridBoundaryClock};
use crate::scheduled_event::StepEvent;
use crate::sequencer::{StepParam, NUM_PARAMS};

pub const DEFAULT_PROCESS_PORT: &str = "__default";
/// Exact retained depth for both grid-step and fired-trigger reads. Keeping a
/// fixed, documented window makes scheduler memory independent of authored
/// process input while covering sixteen bars at sixteenth-note resolution.
pub const PROCESS_READ_HISTORY_DEPTH: usize = 256;

pub type ProcessResolvedValues = [f32; NUM_PARAMS];

#[derive(Clone, Debug, Default)]
pub struct ProcessTrackReadSnapshot {
    pub current: ProcessResolvedValues,
    /// Newest boundary first; index `n` implements `:steps-ago n`.
    pub steps: Vec<ProcessResolvedValues>,
    /// Newest fired trigger first; index `n` implements `:trigs-ago n`.
    pub trigs: Vec<ProcessResolvedValues>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessReadSnapshot {
    pub tracks: Arc<Vec<ProcessTrackReadSnapshot>>,
    pub process_values: HashMap<String, HashMap<String, Value>>,
    pub channels: HashMap<String, Value>,
    pub fields: HashMap<String, Value>,
    pub conductor_observe_tracks: Vec<usize>,
    pub conductor_play_tracks: Vec<usize>,
}

#[derive(Clone, Debug)]
struct TimedResolvedValues {
    beat: f64,
    values: ProcessResolvedValues,
}

#[derive(Clone, Debug)]
struct ResolvedTrackHistory {
    base: ProcessResolvedValues,
    current: ProcessResolvedValues,
    steps: VecDeque<TimedResolvedValues>,
    trigs: VecDeque<TimedResolvedValues>,
}

impl ResolvedTrackHistory {
    fn new(base: ProcessResolvedValues) -> Self {
        Self {
            base,
            current: base,
            steps: VecDeque::new(),
            trigs: VecDeque::new(),
        }
    }
}

pub fn resolved_values_from_step(
    resolved: ResolvedStep,
    step_params: &[f32; NUM_PARAMS],
) -> ProcessResolvedValues {
    let mut values = *step_params;
    values[StepParam::Duration.index()] = resolved.duration;
    values[StepParam::Velocity.index()] = resolved.velocity;
    values[StepParam::Speed.index()] = resolved.speed;
    values[StepParam::AuxA.index()] = resolved.aux_a;
    values[StepParam::AuxB.index()] = resolved.aux_b;
    values[StepParam::Transpose.index()] = resolved.transpose;
    values[StepParam::Pan.index()] = resolved.pan;
    values[StepParam::Chop.index()] = resolved.chop;
    values
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcessTimeExpr {
    Beats(f64),
    Inlet(String),
}

impl ProcessTimeExpr {
    pub fn beats(&self, inlets: &HashMap<String, ProcessInletValue>) -> f64 {
        match self {
            Self::Beats(beats) => beats.max(1e-9),
            Self::Inlet(name) => inlets
                .get(name)
                .and_then(ProcessInletValue::literal_number)
                .unwrap_or(1.0)
                .max(1e-9),
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ProcessInstanceId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessInletKind {
    Float,
    Int,
    Gate,
    Track,
    Field,
    Any,
}

impl Default for ProcessInletKind {
    fn default() -> Self {
        Self::Any
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessSeedPolicy {
    #[default]
    Locked,
    PerCycle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessTargetHint {
    StepParam { param: String },
    ParamTag { tag: String },
    InstrumentParam { param: String },
    EffectParam { effect: String, param: String },
    MidiFxParam { fx: String, param: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessTargetKind {
    StepParam,
    DeviceParam,
    InstrumentParam,
    EffectParam,
    MidiFxParam,
    ProcessInlet,
    RackSlotParam,
    RackSlotInstrumentParam,
}

impl ProcessTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StepParam => "step-param",
            Self::DeviceParam => "device-param",
            Self::InstrumentParam => "instrument-param",
            Self::EffectParam => "effect-param",
            Self::MidiFxParam => "midi-fx-param",
            Self::ProcessInlet => "process-inlet",
            Self::RackSlotParam => "rack-slot-param",
            Self::RackSlotInstrumentParam => "rack-slot-instrument-param",
        }
    }

    pub fn matches_hint(self, hint: &ProcessTargetHint) -> bool {
        match self {
            Self::StepParam => matches!(hint, ProcessTargetHint::StepParam { .. }),
            Self::DeviceParam => matches!(
                hint,
                ProcessTargetHint::ParamTag { .. }
                    | ProcessTargetHint::InstrumentParam { .. }
                    | ProcessTargetHint::EffectParam { .. }
                    | ProcessTargetHint::MidiFxParam { .. }
            ),
            Self::InstrumentParam => matches!(hint, ProcessTargetHint::InstrumentParam { .. }),
            Self::EffectParam => matches!(hint, ProcessTargetHint::EffectParam { .. }),
            Self::MidiFxParam => matches!(hint, ProcessTargetHint::MidiFxParam { .. }),
            Self::ProcessInlet => false,
            Self::RackSlotParam | Self::RackSlotInstrumentParam => false,
        }
    }

    pub fn matches_target(self, target: &ParamTarget) -> bool {
        match self {
            Self::StepParam => matches!(target, ParamTarget::StepParam { .. }),
            Self::DeviceParam => matches!(
                target,
                ParamTarget::InstrumentParam { .. }
                    | ParamTarget::EffectParam { .. }
                    | ParamTarget::MidiFxParam { .. }
            ),
            Self::InstrumentParam => matches!(target, ParamTarget::InstrumentParam { .. }),
            Self::EffectParam => matches!(target, ParamTarget::EffectParam { .. }),
            Self::MidiFxParam => matches!(target, ParamTarget::MidiFxParam { .. }),
            Self::ProcessInlet => matches!(target, ParamTarget::ProcessInlet { .. }),
            Self::RackSlotParam => matches!(target, ParamTarget::RackSlotParam { .. }),
            Self::RackSlotInstrumentParam => {
                matches!(target, ParamTarget::RackSlotInstrumentParam { .. })
            }
        }
    }
}

impl ProcessTargetHint {
    pub fn target_kind(&self) -> ProcessTargetKind {
        match self {
            Self::StepParam { .. } => ProcessTargetKind::StepParam,
            Self::ParamTag { .. } => ProcessTargetKind::DeviceParam,
            Self::InstrumentParam { .. } => ProcessTargetKind::InstrumentParam,
            Self::EffectParam { .. } => ProcessTargetKind::EffectParam,
            Self::MidiFxParam { .. } => ProcessTargetKind::MidiFxParam,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessPortDef {
    pub name: String,
    pub target: Option<ProcessTargetHint>,
    #[serde(default)]
    pub binding_mode: ProcessPortBindingMode,
    #[serde(default)]
    pub target_kind: Option<ProcessTargetKind>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessPortBindingMode {
    #[default]
    Fixed,
    Mappable,
    Connectable,
}

impl ProcessPortDef {
    pub fn default_with_target(target: ProcessTargetHint) -> Self {
        Self {
            name: DEFAULT_PROCESS_PORT.to_string(),
            target: Some(target),
            binding_mode: ProcessPortBindingMode::Fixed,
            target_kind: None,
        }
    }

    pub fn with_target(name: impl Into<String>, target: ProcessTargetHint) -> Self {
        Self {
            name: name.into(),
            target: Some(target),
            binding_mode: ProcessPortBindingMode::Fixed,
            target_kind: None,
        }
    }

    pub fn mappable(
        name: impl Into<String>,
        target_kind: Option<ProcessTargetKind>,
        target: Option<ProcessTargetHint>,
    ) -> Self {
        Self {
            name: name.into(),
            target,
            binding_mode: ProcessPortBindingMode::Mappable,
            target_kind,
        }
    }

    pub fn process_inlet(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            target: None,
            binding_mode: ProcessPortBindingMode::Connectable,
            target_kind: Some(ProcessTargetKind::ProcessInlet),
        }
    }

    pub fn default_mappable(
        target_kind: Option<ProcessTargetKind>,
        target: Option<ProcessTargetHint>,
    ) -> Self {
        Self::mappable(DEFAULT_PROCESS_PORT, target_kind, target)
    }

    pub fn effective_target_kind(&self) -> Option<ProcessTargetKind> {
        self.target_kind
            .or_else(|| self.target.as_ref().map(ProcessTargetHint::target_kind))
    }

    pub fn is_mappable(&self) -> bool {
        self.binding_mode == ProcessPortBindingMode::Mappable
    }

    pub fn is_connectable(&self) -> bool {
        self.binding_mode == ProcessPortBindingMode::Connectable
    }

    pub fn allows_parameter_mapping_target(&self, target: &ParamTarget) -> bool {
        self.is_mappable()
            && !matches!(target, ParamTarget::ProcessInlet { .. })
            && self
                .effective_target_kind()
                .map(|kind| kind.matches_target(target))
                .unwrap_or(true)
    }

    pub fn allows_connection_target(&self, target: &ParamTarget) -> bool {
        self.is_connectable()
            && matches!(target, ParamTarget::ProcessInlet { .. })
            && self
                .effective_target_kind()
                .map(|kind| kind.matches_target(target))
                .unwrap_or(false)
    }

    pub fn allows_binding_target(&self, target: &ParamTarget) -> bool {
        self.allows_parameter_mapping_target(target) || self.allows_connection_target(target)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamTarget {
    StepParam {
        param: String,
    },
    InstrumentParam {
        param: String,
        param_id: Option<ParamNodeId>,
    },
    EffectParam {
        slot: usize,
        effect: String,
        param: String,
        param_id: Option<ParamNodeId>,
    },
    MidiFxParam {
        slot: usize,
        fx: String,
        param: String,
    },
    ProcessInlet {
        process: String,
        inlet: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance_id: Option<ProcessInstanceId>,
    },
    RackSlotParam {
        slot: usize,
        param: String,
    },
    RackSlotInstrumentParam {
        slot: usize,
        param: String,
        param_id: Option<ParamNodeId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessTargetOp {
    Set,
    Add,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessTargetWrite {
    pub port: String,
    pub target: Option<ProcessTargetHint>,
    pub op: ProcessTargetOp,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessAccumulatorMode {
    Wrap,
    Clip,
    Bounce,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessAccumulatorSpec {
    pub amount_inlet: String,
    pub reset_inlet: Option<String>,
    pub range: Option<(f32, f32)>,
    pub mode: ProcessAccumulatorMode,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessLane {
    pub values: Vec<f32>,
}

impl ProcessLane {
    pub fn value_at(&self, step: usize, default: f32) -> f32 {
        self.values.get(step).copied().unwrap_or(default)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackProcessSlot {
    pub instance_id: ProcessInstanceId,
    #[serde(default)]
    pub instance_name: Option<String>,
    pub class_name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Slot belongs to the project-level default layer shared by every track.
    /// Project slots share configuration (knobs, lanes) but never runtime
    /// state: their runtime ids are keyed `(instance, track)` at fire time.
    #[serde(default)]
    pub project_layer: bool,
    #[serde(default)]
    pub inlets: BTreeMap<String, ProcessLiteral>,
    #[serde(default)]
    pub lanes: BTreeMap<String, ProcessLane>,
    #[serde(default)]
    pub bindings: BTreeMap<String, Option<ParamTarget>>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackProcessChain {
    #[serde(default)]
    pub slots: Vec<TrackProcessSlot>,
}

/// Forked lanes for one track, keyed by durable project-slot identity and inlet.
pub type ProjectLaneOverrides = BTreeMap<ProcessInstanceId, BTreeMap<String, ProcessLane>>;

pub fn project_slot_identity_id(slot: &TrackProcessSlot) -> ProcessInstanceId {
    ProcessInstanceId(if let Some(name) = slot.instance_name.as_deref() {
        named_process_runtime_id(&slot.class_name, name)
    } else {
        slot.instance_id.0
    })
}

pub fn apply_project_lane_overrides(
    chain: &mut TrackProcessChain,
    overrides: &ProjectLaneOverrides,
) {
    for slot in &mut chain.slots {
        let Some(lanes) = overrides.get(&project_slot_identity_id(slot)) else {
            continue;
        };
        for (inlet, lane) in lanes {
            slot.lanes.insert(inlet.clone(), lane.clone());
        }
    }
}

/// A track's effective chain at fire time: project-layer slots run first,
/// then the track's own slots (the project layer is a policy composed at
/// snapshot capture, never stamped into per-track storage).
pub fn compose_effective_process_chain(
    project: &TrackProcessChain,
    track: &TrackProcessChain,
) -> TrackProcessChain {
    if project.slots.is_empty() {
        return track.clone();
    }
    let mut slots = Vec::with_capacity(project.slots.len() + track.slots.len());
    slots.extend(project.slots.iter().cloned());
    slots.extend(track.slots.iter().cloned());
    TrackProcessChain { slots }
}

fn process_param_index_by_tag_or_name(
    descriptor: &EffectDescriptor,
    tag_or_name: &str,
) -> Option<usize> {
    descriptor
        .params
        .iter()
        .position(|param| param.has_tag_or_name(tag_or_name))
}

fn slot_param_node_id(slot: &EffectSlotSnapshot, param_idx: usize) -> Option<ParamNodeId> {
    let raw_idx = slot.param_node_indices.get(param_idx).copied()?;
    ParamNodeId::from_slot_param(slot.node_id, slot.modulator_node_id, raw_idx)
}

fn refresh_effect_binding_param_id(
    slot_idx: usize,
    effect_name: &str,
    param_name: &str,
    param_id: &mut Option<ParamNodeId>,
    effect_descriptors: &[EffectDescriptor],
    effect_slots: &[EffectSlotSnapshot],
) {
    let Some(desc) = effect_descriptors.get(slot_idx) else {
        return;
    };
    if !desc.name.eq_ignore_ascii_case(effect_name) {
        return;
    }
    let Some(param_idx) = process_param_index_by_tag_or_name(desc, param_name) else {
        return;
    };
    let Some(slot) = effect_slots.get(slot_idx) else {
        return;
    };
    if let Some(updated) = slot_param_node_id(slot, param_idx) {
        *param_id = Some(updated);
    }
}

pub fn refresh_track_process_chain_binding_param_ids(
    chain: &mut TrackProcessChain,
    instrument_descriptor: Option<&EffectDescriptor>,
    instrument_slot: Option<&EffectSlotSnapshot>,
    effect_descriptors: &[EffectDescriptor],
    effect_slots: &[EffectSlotSnapshot],
) {
    for slot in &mut chain.slots {
        for binding in slot.bindings.values_mut().flatten() {
            match binding {
                ParamTarget::InstrumentParam { param, param_id } => {
                    let (Some(desc), Some(slot)) = (instrument_descriptor, instrument_slot) else {
                        continue;
                    };
                    let Some(param_idx) = process_param_index_by_tag_or_name(desc, param) else {
                        continue;
                    };
                    if let Some(updated) = slot_param_node_id(slot, param_idx) {
                        *param_id = Some(updated);
                    }
                }
                ParamTarget::EffectParam {
                    slot,
                    effect,
                    param,
                    param_id,
                } => refresh_effect_binding_param_id(
                    *slot,
                    effect,
                    param,
                    param_id,
                    effect_descriptors,
                    effect_slots,
                ),
                _ => {}
            }
        }
    }
}

pub fn refresh_track_process_chain_effect_binding_param_ids_for_slot(
    chain: &mut TrackProcessChain,
    slot_idx: usize,
    descriptor: &EffectDescriptor,
    effect_slot: &EffectSlotSnapshot,
) {
    for process_slot in &mut chain.slots {
        for binding in process_slot.bindings.values_mut().flatten() {
            let ParamTarget::EffectParam {
                slot,
                effect,
                param,
                param_id,
            } = binding
            else {
                continue;
            };
            if *slot != slot_idx {
                continue;
            }
            if !descriptor.name.eq_ignore_ascii_case(effect) {
                continue;
            }
            let Some(param_idx) = process_param_index_by_tag_or_name(descriptor, param) else {
                continue;
            };
            if let Some(updated) = slot_param_node_id(effect_slot, param_idx) {
                *param_id = Some(updated);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessInletDef {
    pub name: String,
    pub kind: ProcessInletKind,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub default: Value,
    pub lane: bool,
    pub doc: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessOutletDef {
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct ProcessStateDef {
    pub name: String,
    pub initial: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessListenDef {
    pub name: String,
    pub source: ProcessEventSource,
    pub handler_source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEventSource {
    TrackFires(usize),
    SeqFires(String),
    Channel(String),
    Outlet(ProcessOutletRef),
}

impl ProcessEventSource {
    fn matches_process_source(&self, source: &ProcessSourceRef) -> bool {
        match (self, source) {
            (Self::TrackFires(left), ProcessSourceRef::TrackFires(right)) => left == right,
            (Self::SeqFires(left), ProcessSourceRef::SeqFires(right)) => left == right,
            (Self::Channel(left), ProcessSourceRef::Channel(right)) => left == right,
            (Self::Outlet(left), ProcessSourceRef::Outlet(right)) => left == right,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcessDef {
    pub id: u64,
    pub name: String,
    /// File being evaluated when the definition was registered. The UI uses
    /// this to navigate from an attached slot back to its authored source.
    pub source_path: Option<String>,
    pub doc: Option<String>,
    pub inlets: Vec<ProcessInletDef>,
    pub outlets: Vec<ProcessOutletDef>,
    pub state: Vec<ProcessStateDef>,
    pub every: Option<ProcessTimeExpr>,
    pub seed_policy: ProcessSeedPolicy,
    pub ports: Vec<ProcessPortDef>,
    pub accumulator: Option<ProcessAccumulatorSpec>,
    pub run_source: Option<String>,
    pub listens: Vec<ProcessListenDef>,
}

#[derive(Clone, Debug)]
pub enum ProcessInletValue {
    Literal(Value),
    Outlet(ProcessOutletRef),
    Channel(String),
}

impl ProcessInletValue {
    fn literal_number(&self) -> Option<f64> {
        match self {
            Self::Literal(Value::Number(value)) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AuthoredHandleId(pub u64);

#[derive(Clone, Debug)]
pub struct AuthoredProcessInstance {
    pub handle_id: AuthoredHandleId,
    pub name: Option<String>,
    pub class_name: String,
    pub inlets: HashMap<String, ProcessInletValue>,
    pub bindings: BTreeMap<String, Option<ParamTarget>>,
    pub running: bool,
    pub anonymous: bool,
    pub one_shot: bool,
    pub every: Option<ProcessTimeExpr>,
    pub run_source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AuthoredChannel {
    pub handle_id: AuthoredHandleId,
    pub name: Option<String>,
    pub initial: Option<Value>,
    pub message_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProcessOutletRef {
    pub process_handle_id: AuthoredHandleId,
    pub outlet: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProcessSourceRef {
    Outlet(ProcessOutletRef),
    Channel(String),
    TrackFires(usize),
    SeqFires(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProcessTargetRef {
    Inlet {
        process_handle_id: AuthoredHandleId,
        inlet: String,
    },
    Channel(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuthoredPatch {
    pub source: ProcessSourceRef,
    pub target: ProcessTargetRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredConductorAttachment {
    pub process_handle_id: AuthoredHandleId,
    pub observe_tracks: Vec<usize>,
    pub play_tracks: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessAuthoringSnapshot {
    pub defs: Vec<ProcessDef>,
    pub instances: Vec<AuthoredProcessInstance>,
    pub channels: Vec<AuthoredChannel>,
    pub patches: Vec<AuthoredPatch>,
    pub conductors: Vec<AuthoredConductorAttachment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProcessLiteral {
    Number(f64),
    Bool(bool),
    Nil,
    String(String),
    Symbol(String),
    Keyword(String),
    List(Vec<ProcessLiteral>),
    Map(BTreeMap<String, ProcessLiteral>),
}

impl ProcessLiteral {
    pub fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Number(value) => Ok(Self::Number(*value)),
            Value::Bool(value) => Ok(Self::Bool(*value)),
            Value::Nil => Ok(Self::Nil),
            Value::String(value) => Ok(Self::String(value.clone())),
            Value::Symbol(value) => Ok(Self::Symbol(value.clone())),
            Value::Keyword(value) => Ok(Self::Keyword(value.clone())),
            Value::List(items) => items
                .iter()
                .map(|item| Self::from_value(&item.borrow()))
                .collect::<Result<Vec<_>, _>>()
                .map(Self::List),
            Value::Map(map) => map
                .iter()
                .map(|(key, value)| Ok((key.clone(), Self::from_value(&value.borrow())?)))
                .collect::<Result<BTreeMap<_, _>, String>>()
                .map(Self::Map),
            Value::Closure(_, _)
            | Value::Function(_)
            | Value::NativeFunction(_)
            | Value::NodeRef(_)
            | Value::ReactiveRef { .. }
            | Value::HostHandle { .. } => Err(format!(
                "process authoring literal cannot publish {}",
                eseqlisp::vm::format_lisp_value(value)
            )),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Number(value) => Value::Number(*value),
            Self::Bool(value) => Value::Bool(*value),
            Self::Nil => Value::Nil,
            Self::String(value) => Value::String(value.clone()),
            Self::Symbol(value) => Value::Symbol(value.clone()),
            Self::Keyword(value) => Value::Keyword(value.clone()),
            Self::List(items) => Value::List(
                items
                    .iter()
                    .map(|item| std::rc::Rc::new(std::cell::RefCell::new(item.to_value())))
                    .collect(),
            ),
            Self::Map(map) => Value::Map(
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            std::rc::Rc::new(std::cell::RefCell::new(value.to_value())),
                        )
                    })
                    .collect(),
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PublishedProcessAuthoringSnapshot {
    pub defs: Vec<PublishedProcessDef>,
    pub instances: Vec<PublishedAuthoredProcessInstance>,
    pub channels: Vec<PublishedAuthoredChannel>,
    pub patches: Vec<AuthoredPatch>,
    pub conductors: Vec<AuthoredConductorAttachment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedProcessDef {
    pub id: u64,
    pub name: String,
    pub source_path: Option<String>,
    pub doc: Option<String>,
    pub inlets: Vec<PublishedProcessInletDef>,
    pub outlets: Vec<ProcessOutletDef>,
    pub state: Vec<PublishedProcessStateDef>,
    pub every: Option<ProcessTimeExpr>,
    pub seed_policy: ProcessSeedPolicy,
    pub ports: Vec<ProcessPortDef>,
    pub accumulator: Option<ProcessAccumulatorSpec>,
    pub run_source: Option<String>,
    pub listens: Vec<ProcessListenDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedProcessInletDef {
    pub name: String,
    pub kind: ProcessInletKind,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub default: ProcessLiteral,
    pub lane: bool,
    pub doc: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedProcessStateDef {
    pub name: String,
    pub initial: ProcessLiteral,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedAuthoredProcessInstance {
    pub handle_id: AuthoredHandleId,
    pub name: Option<String>,
    pub class_name: String,
    pub inlets: HashMap<String, PublishedProcessInletValue>,
    pub bindings: BTreeMap<String, Option<ParamTarget>>,
    pub running: bool,
    pub anonymous: bool,
    pub one_shot: bool,
    pub every: Option<ProcessTimeExpr>,
    pub run_source: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PublishedProcessInletValue {
    Literal(ProcessLiteral),
    Outlet(ProcessOutletRef),
    Channel(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedAuthoredChannel {
    pub handle_id: AuthoredHandleId,
    pub name: Option<String>,
    pub initial: Option<ProcessLiteral>,
    pub message_only: bool,
}

impl ProcessAuthoringSnapshot {
    pub fn to_published(&self) -> Result<PublishedProcessAuthoringSnapshot, String> {
        Ok(PublishedProcessAuthoringSnapshot {
            defs: self
                .defs
                .iter()
                .map(|def| {
                    Ok(PublishedProcessDef {
                        id: def.id,
                        name: def.name.clone(),
                        source_path: def.source_path.clone(),
                        doc: def.doc.clone(),
                        inlets: def
                            .inlets
                            .iter()
                            .map(|inlet| {
                                Ok(PublishedProcessInletDef {
                                    name: inlet.name.clone(),
                                    kind: inlet.kind.clone(),
                                    min: inlet.min,
                                    max: inlet.max,
                                    default: ProcessLiteral::from_value(&inlet.default)?,
                                    lane: inlet.lane,
                                    doc: inlet.doc.clone(),
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()?,
                        outlets: def.outlets.clone(),
                        state: def
                            .state
                            .iter()
                            .map(|state| {
                                Ok(PublishedProcessStateDef {
                                    name: state.name.clone(),
                                    initial: ProcessLiteral::from_value(&state.initial)?,
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()?,
                        every: def.every.clone(),
                        seed_policy: def.seed_policy,
                        ports: def.ports.clone(),
                        accumulator: def.accumulator.clone(),
                        run_source: def.run_source.clone(),
                        listens: def.listens.clone(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            instances: self
                .instances
                .iter()
                .map(|instance| {
                    Ok(PublishedAuthoredProcessInstance {
                        handle_id: instance.handle_id,
                        name: instance.name.clone(),
                        class_name: instance.class_name.clone(),
                        inlets: instance
                            .inlets
                            .iter()
                            .map(|(name, value)| {
                                Ok((
                                    name.clone(),
                                    PublishedProcessInletValue::from_runtime(value)?,
                                ))
                            })
                            .collect::<Result<HashMap<_, _>, String>>()?,
                        bindings: instance.bindings.clone(),
                        running: instance.running,
                        anonymous: instance.anonymous,
                        one_shot: instance.one_shot,
                        every: instance.every.clone(),
                        run_source: instance.run_source.clone(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            channels: self
                .channels
                .iter()
                .map(|channel| {
                    Ok(PublishedAuthoredChannel {
                        handle_id: channel.handle_id,
                        name: channel.name.clone(),
                        initial: channel
                            .initial
                            .as_ref()
                            .map(ProcessLiteral::from_value)
                            .transpose()?,
                        message_only: channel.message_only,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            patches: self.patches.clone(),
            conductors: self.conductors.clone(),
        })
    }
}

impl PublishedProcessInletValue {
    fn from_runtime(value: &ProcessInletValue) -> Result<Self, String> {
        match value {
            ProcessInletValue::Literal(value) => {
                Ok(Self::Literal(ProcessLiteral::from_value(value)?))
            }
            ProcessInletValue::Outlet(value) => Ok(Self::Outlet(value.clone())),
            ProcessInletValue::Channel(value) => Ok(Self::Channel(value.clone())),
        }
    }

    fn to_runtime(&self) -> ProcessInletValue {
        match self {
            Self::Literal(value) => ProcessInletValue::Literal(value.to_value()),
            Self::Outlet(value) => ProcessInletValue::Outlet(value.clone()),
            Self::Channel(value) => ProcessInletValue::Channel(value.clone()),
        }
    }
}

impl PublishedProcessAuthoringSnapshot {
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
            && self.instances.is_empty()
            && self.channels.is_empty()
            && self.patches.is_empty()
            && self.conductors.is_empty()
    }

    pub fn to_runtime(&self) -> ProcessAuthoringSnapshot {
        ProcessAuthoringSnapshot {
            defs: self
                .defs
                .iter()
                .map(|def| ProcessDef {
                    id: def.id,
                    name: def.name.clone(),
                    source_path: def.source_path.clone(),
                    doc: def.doc.clone(),
                    inlets: def
                        .inlets
                        .iter()
                        .map(|inlet| ProcessInletDef {
                            name: inlet.name.clone(),
                            kind: inlet.kind.clone(),
                            min: inlet.min,
                            max: inlet.max,
                            default: inlet.default.to_value(),
                            lane: inlet.lane,
                            doc: inlet.doc.clone(),
                        })
                        .collect(),
                    outlets: def.outlets.clone(),
                    state: def
                        .state
                        .iter()
                        .map(|state| ProcessStateDef {
                            name: state.name.clone(),
                            initial: state.initial.to_value(),
                        })
                        .collect(),
                    every: def.every.clone(),
                    seed_policy: def.seed_policy,
                    ports: def.ports.clone(),
                    accumulator: def.accumulator.clone(),
                    run_source: def.run_source.clone(),
                    listens: def.listens.clone(),
                })
                .collect(),
            instances: self
                .instances
                .iter()
                .map(|instance| AuthoredProcessInstance {
                    handle_id: instance.handle_id,
                    name: instance.name.clone(),
                    class_name: instance.class_name.clone(),
                    inlets: instance
                        .inlets
                        .iter()
                        .map(|(name, value)| (name.clone(), value.to_runtime()))
                        .collect(),
                    bindings: instance.bindings.clone(),
                    running: instance.running,
                    anonymous: instance.anonymous,
                    one_shot: instance.one_shot,
                    every: instance.every.clone(),
                    run_source: instance.run_source.clone(),
                })
                .collect(),
            channels: self
                .channels
                .iter()
                .map(|channel| AuthoredChannel {
                    handle_id: channel.handle_id,
                    name: channel.name.clone(),
                    initial: channel.initial.as_ref().map(ProcessLiteral::to_value),
                    message_only: channel.message_only,
                })
                .collect(),
            patches: self.patches.clone(),
            conductors: self.conductors.clone(),
        }
    }
}

pub fn merge_authoring_snapshots(
    mut base: ProcessAuthoringSnapshot,
    overlay: ProcessAuthoringSnapshot,
) -> ProcessAuthoringSnapshot {
    for def in overlay.defs {
        if let Some(existing) = base
            .defs
            .iter_mut()
            .find(|entry| entry.id == def.id || entry.name == def.name)
        {
            *existing = def;
        } else {
            base.defs.push(def);
        }
    }
    for instance in overlay.instances {
        if let Some(name) = instance.name.as_deref() {
            base.instances
                .retain(|entry| entry.name.as_deref() != Some(name));
        } else {
            base.instances
                .retain(|entry| entry.handle_id != instance.handle_id);
        }
        base.instances.push(instance);
    }
    for channel in overlay.channels {
        if let Some(name) = channel.name.as_deref() {
            base.channels
                .retain(|entry| entry.name.as_deref() != Some(name));
        } else {
            base.channels
                .retain(|entry| entry.handle_id != channel.handle_id);
        }
        base.channels.push(channel);
    }
    base.patches.extend(overlay.patches);
    for conductor in overlay.conductors {
        base.conductors
            .retain(|entry| entry.process_handle_id != conductor.process_handle_id);
        base.conductors.push(conductor);
    }
    base
}

#[derive(Clone, Debug)]
struct ProcessInstance {
    runtime_id: u64,
    handle_id: AuthoredHandleId,
    name: Option<String>,
    class_name: String,
    inlets: HashMap<String, ProcessInletValue>,
    outlets: HashMap<String, Value>,
    state: HashMap<String, Value>,
    running: bool,
    anonymous: bool,
    one_shot: bool,
    every: Option<ProcessTimeExpr>,
    run_source: Option<String>,
    listens: Vec<ProcessListenDef>,
    clock: Option<GridBoundaryClock>,
    one_shot_target_beat: Option<f64>,
}

#[derive(Clone, Debug)]
struct ChannelState {
    name: String,
    value: Option<Value>,
    message_only: bool,
    field_publications: VecDeque<TimedFieldValue>,
}

#[derive(Clone, Debug)]
struct TimedFieldValue {
    beat: f64,
    value: Value,
}

#[derive(Clone, Debug)]
pub struct ProcessRunInvocation {
    pub runtime_id: u64,
    pub source: String,
    pub beat: f64,
    pub sample_time: u64,
    pub inlets: HashMap<String, Value>,
    pub state: HashMap<String, Value>,
    pub event: Option<Value>,
    pub step_context: Option<ProcessStepEventContext>,
    pub ports: Vec<ProcessPortDef>,
    pub reads: ProcessReadSnapshot,
    pub seed: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessRunResult {
    pub runtime_id: u64,
    pub beat: f64,
    pub sample_time: u64,
    pub state: HashMap<String, Value>,
    pub outputs: Vec<ProcessOutput>,
    pub emissions: Vec<EmittedAccumulatorEvent>,
    pub commands: Vec<ProcessRunCommand>,
    pub target_writes: Vec<ProcessTargetWrite>,
    pub transpose: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcessRunCommand {
    TargetWrite(ProcessTargetWrite),
    VetoBaseEvent,
    Ratchet(ProcessRatchetRequest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessRatchetMode {
    Subdivide,
    Repeat,
}

impl Default for ProcessRatchetMode {
    fn default() -> Self {
        Self::Subdivide
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRatchetRequest {
    pub times: u32,
    pub mode: ProcessRatchetMode,
    pub span_beats: Option<f32>,
    pub shape: Option<Value>,
    pub shape_context: ProcessRatchetShapeContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRatchetShapeContext {
    pub runtime_id: u64,
    pub beat: f64,
    pub inlets: HashMap<String, Value>,
    pub state: HashMap<String, Value>,
    pub event: Option<Value>,
    pub step_context: ProcessStepEventContext,
    pub ports: Vec<ProcessPortDef>,
    pub random_state: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessRatchetEvent {
    pub offset_beats: f32,
    pub resolved: ResolvedStep,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessStepEventContext {
    pub track: usize,
    pub step: usize,
    pub cycle: u64,
    pub beat: f64,
    pub sample_time: u64,
    pub step_beats: f32,
    pub resolved: ResolvedStep,
}

#[derive(Clone, Debug)]
pub struct ProcessOutput {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessInletWrite {
    pub op: ProcessTargetOp,
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct ProcessScheduledEmission {
    pub process_runtime_id: u64,
    pub beat: f64,
    pub event: EmittedAccumulatorEvent,
}

#[derive(Clone, Debug)]
pub struct ProcessScheduledItem {
    pub process_runtime_id: u64,
    pub beat: f64,
    pub event: ProcessScheduledEvent,
}

#[derive(Clone, Debug)]
pub enum ProcessScheduledEvent {
    Emission(EmittedAccumulatorEvent),
    Step(ProcessScheduledStepEvent),
}

#[derive(Clone, Debug)]
pub struct ProcessScheduledStepEvent {
    pub event: StepEvent,
    pub midi_fx_params: Vec<ProcessMidiFxParamOverride>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessMidiFxParamOverride {
    pub slot: usize,
    pub fx: String,
    pub param: String,
    pub param_idx: usize,
    pub value: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessRuntime {
    defs: HashMap<String, ProcessDef>,
    instances: Vec<ProcessInstance>,
    handle_to_runtime: HashMap<AuthoredHandleId, u64>,
    channels: HashMap<String, ChannelState>,
    patches: Vec<AuthoredPatch>,
    pending_events: Vec<PendingProcessEvent>,
    step_process_states: HashMap<ProcessInstanceId, HashMap<String, Value>>,
    step_process_runtime_ids: HashSet<u64>,
    pending_step_inlet_writes: HashMap<(usize, ProcessInstanceId, String), Vec<ProcessInletWrite>>,
    resolved_track_history: Vec<ResolvedTrackHistory>,
    resolved_track_snapshot_cache: Option<(u64, Arc<Vec<ProcessTrackReadSnapshot>>)>,
    /// Named aliases are exact. Class aliases are retained only while unique;
    /// ambiguous class reads resolve inertly instead of selecting by visit order.
    step_process_aliases: HashMap<String, Option<u64>>,
    conductors: Vec<AuthoredConductorAttachment>,
    pending_conductor_ticks: VecDeque<PendingConductorTick>,
    global_transpose: f32,
}

#[derive(Clone, Debug)]
struct PendingConductorTick {
    beat: f64,
    sample_time: u64,
    fired_tracks: HashSet<usize>,
}

#[derive(Clone, Debug)]
struct PendingProcessEvent {
    process_runtime_id: u64,
    beat: f64,
    event: ProcessScheduledEvent,
}

impl ProcessRuntime {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.pending_events.is_empty()
    }

    pub fn global_transpose(&self) -> f32 {
        self.global_transpose
    }

    pub fn ensure_resolved_track_bases(&mut self, bases: &[ProcessResolvedValues]) {
        let old_len = self.resolved_track_history.len();
        if self.resolved_track_history.len() > bases.len() {
            self.resolved_track_history.truncate(bases.len());
        }
        for (track, base) in bases.iter().copied().enumerate() {
            if let Some(history) = self.resolved_track_history.get_mut(track) {
                history.base = base;
                continue;
            }
            self.resolved_track_history
                .push(ResolvedTrackHistory::new(base));
        }
        if old_len != self.resolved_track_history.len() {
            self.resolved_track_snapshot_cache = None;
        }
    }

    /// Reset all resolved reads to pattern base values. This is intentionally
    /// separate from transport realignment: stop/start preserves musical state,
    /// while a pattern change follows the default accumulator reset policy.
    pub fn reset_resolved_track_history(&mut self, bases: &[ProcessResolvedValues]) {
        self.resolved_track_history = bases
            .iter()
            .copied()
            .map(ResolvedTrackHistory::new)
            .collect();
        self.resolved_track_snapshot_cache = None;
        for channel in self.channels.values_mut() {
            channel.field_publications.clear();
        }
    }

    pub fn record_track_step_boundary(&mut self, track: usize, beat: f64) {
        let Some(history) = self.resolved_track_history.get_mut(track) else {
            return;
        };
        history.steps.push_back(TimedResolvedValues {
            beat,
            values: history.current,
        });
        while history.steps.len() > PROCESS_READ_HISTORY_DEPTH {
            history.steps.pop_front();
        }
        if self
            .resolved_track_snapshot_cache
            .as_ref()
            .is_some_and(|(cached_beat, _)| beat <= f64::from_bits(*cached_beat) + 1e-9)
        {
            self.resolved_track_snapshot_cache = None;
        }
    }

    pub fn record_track_fire(
        &mut self,
        track: usize,
        beat: f64,
        sample_time: u64,
        values: ProcessResolvedValues,
    ) {
        let Some(history) = self.resolved_track_history.get_mut(track) else {
            return;
        };
        history.current = values;
        history
            .trigs
            .push_back(TimedResolvedValues { beat, values });
        while history.trigs.len() > PROCESS_READ_HISTORY_DEPTH {
            history.trigs.pop_front();
        }
        if self
            .resolved_track_snapshot_cache
            .as_ref()
            .is_some_and(|(cached_beat, _)| beat < f64::from_bits(*cached_beat) - 1e-9)
        {
            self.resolved_track_snapshot_cache = None;
        }
        if self
            .conductors
            .iter()
            .any(|conductor| conductor.observe_tracks.contains(&track))
        {
            if let Some(tick) = self
                .pending_conductor_ticks
                .iter_mut()
                .find(|tick| (tick.beat - beat).abs() <= 1e-9)
            {
                tick.sample_time = tick.sample_time.max(sample_time);
                tick.fired_tracks.insert(track);
            } else {
                self.pending_conductor_ticks
                    .push_back(PendingConductorTick {
                        beat,
                        sample_time,
                        fired_tracks: HashSet::from([track]),
                    });
            }
        }
    }

    pub fn read_snapshot(&mut self, before_beat: f64) -> ProcessReadSnapshot {
        // Strictly earlier beats enforce the previous-tick rule even when the
        // scheduler happens to visit the publishing track first at a shared
        // sample boundary.
        let trig_is_visible = |beat: f64| beat < before_beat - 1e-9;
        // A boundary at the invocation beat contains the value held through
        // the step that just ended, so it is part of previous-step state.
        let step_is_visible = |beat: f64| beat <= before_beat + 1e-9;
        let cache_key = before_beat.to_bits();
        let tracks = if let Some((_, tracks)) = self
            .resolved_track_snapshot_cache
            .as_ref()
            .filter(|(beat, _)| *beat == cache_key)
        {
            Arc::clone(tracks)
        } else {
            let tracks = Arc::new(
                self.resolved_track_history
                    .iter()
                    .map(|history| {
                        let trigs = history
                            .trigs
                            .iter()
                            .rev()
                            .filter(|entry| trig_is_visible(entry.beat))
                            .map(|entry| entry.values)
                            .collect::<Vec<_>>();
                        let current = trigs.first().copied().unwrap_or(history.base);
                        let steps = history
                            .steps
                            .iter()
                            .rev()
                            .filter(|entry| step_is_visible(entry.beat))
                            .map(|entry| entry.values)
                            .collect::<Vec<_>>();
                        ProcessTrackReadSnapshot {
                            current,
                            steps,
                            trigs,
                        }
                    })
                    .collect(),
            );
            self.resolved_track_snapshot_cache = Some((cache_key, Arc::clone(&tracks)));
            tracks
        };
        let channels = self
            .channels
            .iter()
            .filter_map(|(name, channel)| channel.value.clone().map(|value| (name.clone(), value)))
            .collect();
        let fields = self
            .channels
            .iter()
            .filter_map(|(name, channel)| {
                channel
                    .field_publications
                    .iter()
                    .rev()
                    .find(|entry| entry.beat < before_beat - 1e-9)
                    .map(|entry| (name.clone(), entry.value.clone()))
            })
            .collect();
        let mut process_values = HashMap::new();
        let mut standalone_class_counts = HashMap::<&str, usize>::new();
        for instance in &self.instances {
            *standalone_class_counts
                .entry(instance.class_name.as_str())
                .or_default() += 1;
        }
        for instance in &self.instances {
            let mut values = instance.state.clone();
            values.extend(instance.outlets.clone());
            if let Some(name) = instance.name.as_ref() {
                process_values.insert(name.clone(), values.clone());
            }
            if standalone_class_counts
                .get(instance.class_name.as_str())
                .copied()
                == Some(1)
            {
                process_values.insert(instance.class_name.clone(), values);
            }
        }
        for (alias, runtime_id) in &self.step_process_aliases {
            let Some(runtime_id) = runtime_id else {
                continue;
            };
            if let Some(state) = self
                .step_process_states
                .get(&ProcessInstanceId(*runtime_id))
            {
                process_values.insert(alias.clone(), state.clone());
            }
        }
        ProcessReadSnapshot {
            tracks,
            process_values,
            channels,
            fields,
            conductor_observe_tracks: Vec::new(),
            conductor_play_tracks: Vec::new(),
        }
    }

    pub fn conductor_read_snapshot(
        &mut self,
        beat: f64,
        observe_tracks: &[usize],
        play_tracks: &[usize],
    ) -> ProcessReadSnapshot {
        let mut snapshot = self.read_snapshot(beat);
        let tracks = Arc::make_mut(&mut snapshot.tracks);
        for track in observe_tracks {
            let (Some(history), Some(track_snapshot)) = (
                self.resolved_track_history.get(*track),
                tracks.get_mut(*track),
            ) else {
                continue;
            };
            let trigs = history
                .trigs
                .iter()
                .rev()
                .filter(|entry| entry.beat <= beat + 1e-9)
                .map(|entry| entry.values)
                .collect::<Vec<_>>();
            track_snapshot.current = trigs.first().copied().unwrap_or(history.base);
            track_snapshot.trigs = trigs;
        }
        snapshot.conductor_observe_tracks = observe_tracks.to_vec();
        snapshot.conductor_play_tracks = play_tracks.to_vec();
        snapshot
    }

    pub fn reset_transport(&mut self, total_beats: f64) {
        self.pending_events.clear();
        self.pending_step_inlet_writes.clear();
        self.pending_conductor_ticks.clear();
        for instance in &mut self.instances {
            if let Some(clock) = &mut instance.clock {
                clock.realign(total_beats);
            }
        }
    }

    pub fn clear_scene_pending(&mut self) {
        self.pending_events.clear();
        self.pending_step_inlet_writes.clear();
        self.pending_conductor_ticks.clear();
    }

    pub fn defer_step_process_inlet_write(
        &mut self,
        track: usize,
        instance_id: ProcessInstanceId,
        inlet: impl Into<String>,
        write: ProcessInletWrite,
    ) {
        self.pending_step_inlet_writes
            .entry((track, instance_id, inlet.into()))
            .or_default()
            .push(write);
    }

    pub fn take_step_process_inlet_writes(
        &mut self,
        track: usize,
        chain: &TrackProcessChain,
    ) -> BTreeMap<usize, BTreeMap<String, Vec<ProcessInletWrite>>> {
        let pending = std::mem::take(&mut self.pending_step_inlet_writes);
        let mut current = BTreeMap::<usize, BTreeMap<String, Vec<ProcessInletWrite>>>::new();
        for ((pending_track, instance_id, inlet), writes) in pending {
            if pending_track == track {
                if let Some(slot_idx) = chain
                    .slots
                    .iter()
                    .position(|slot| slot.instance_id == instance_id)
                {
                    current
                        .entry(slot_idx)
                        .or_default()
                        .entry(inlet)
                        .or_default()
                        .extend(writes);
                    continue;
                }
                continue;
            }
            self.pending_step_inlet_writes
                .insert((pending_track, instance_id, inlet), writes);
        }
        current
    }

    pub fn sync_authoring(&mut self, authoring: ProcessAuthoringSnapshot, total_beats: f64) {
        self.defs = authoring
            .defs
            .iter()
            .cloned()
            .map(|def| (def.name.clone(), def))
            .collect();
        self.sync_channels(authoring.channels);
        self.sync_instances(authoring.instances, total_beats);
        self.patches = authoring.patches;
        self.conductors = authoring.conductors;
    }

    fn sync_channels(&mut self, channels: Vec<AuthoredChannel>) {
        let mut next = HashMap::new();
        for channel in channels {
            let Some(name) = channel.name else {
                continue;
            };
            let existing = self.channels.remove(&name);
            let (value, field_publications) = existing
                .map(|existing| (existing.value, existing.field_publications))
                .unwrap_or_default();
            let value = value.or(channel.initial.clone());
            next.insert(
                name.clone(),
                ChannelState {
                    name,
                    value,
                    message_only: channel.message_only,
                    field_publications,
                },
            );
        }
        self.channels = next;
    }

    fn sync_instances(&mut self, instances: Vec<AuthoredProcessInstance>, total_beats: f64) {
        let mut existing = std::mem::take(&mut self.instances);
        let mut next = Vec::with_capacity(instances.len());
        let mut handle_to_runtime = HashMap::new();
        for authored in instances {
            let runtime_id = runtime_instance_id(&authored);
            handle_to_runtime.insert(authored.handle_id, runtime_id);
            let mut instance = if let Some(pos) = existing
                .iter()
                .position(|instance| instance.runtime_id == runtime_id)
            {
                existing.swap_remove(pos)
            } else {
                ProcessInstance {
                    runtime_id,
                    handle_id: authored.handle_id,
                    name: authored.name.clone(),
                    class_name: authored.class_name.clone(),
                    inlets: HashMap::new(),
                    outlets: HashMap::new(),
                    state: HashMap::new(),
                    running: authored.running,
                    anonymous: authored.anonymous,
                    one_shot: authored.one_shot,
                    every: None,
                    run_source: None,
                    listens: Vec::new(),
                    clock: None,
                    one_shot_target_beat: None,
                }
            };
            instance.handle_id = authored.handle_id;
            instance.name = authored.name.clone();
            instance.class_name = authored.class_name.clone();
            instance.running = authored.running;
            instance.anonymous = authored.anonymous;
            instance.one_shot = authored.one_shot;
            instance.inlets =
                defaulted_inlets(self.defs.get(&authored.class_name), authored.inlets);
            let every = authored.every.clone().or_else(|| {
                self.defs
                    .get(&authored.class_name)
                    .and_then(|def| def.every.clone())
            });
            let previous_every = instance.every.clone();
            let resolution = every
                .as_ref()
                .map(|expr| expr.beats(&instance.inlets).max(1e-9));
            let should_reclock = !authored.one_shot
                && match (instance.clock, resolution) {
                    (Some(clock), Some(next_resolution)) => {
                        (clock.resolution_beats - next_resolution).abs() > 1e-9
                    }
                    (None, Some(_)) | (Some(_), None) => true,
                    (None, None) => false,
                };
            instance.every = every;
            if instance.one_shot {
                instance.clock = None;
                if previous_every != instance.every || instance.one_shot_target_beat.is_none() {
                    instance.one_shot_target_beat = resolution.map(|delay| total_beats + delay);
                }
            } else {
                instance.one_shot_target_beat = None;
            }
            if should_reclock {
                instance.clock = resolution.map(|resolution| {
                    let mut clock = GridBoundaryClock::new(resolution);
                    clock.realign(total_beats);
                    clock
                });
            }
            instance.run_source = authored.run_source.clone().or_else(|| {
                self.defs
                    .get(&authored.class_name)
                    .and_then(|def| def.run_source.clone())
            });
            instance.listens = self
                .defs
                .get(&authored.class_name)
                .map(|def| def.listens.clone())
                .unwrap_or_default();
            instance.state = reconciled_state(
                std::mem::take(&mut instance.state),
                self.defs.get(&authored.class_name),
            );
            initialize_outlets(&mut instance, self.defs.get(&authored.class_name));
            next.push(instance);
        }
        self.instances = next;
        self.handle_to_runtime = handle_to_runtime;
    }

    pub fn process_block(
        &mut self,
        start_beats: f64,
        end_beats: f64,
        block_start_sample: u64,
        samples_per_quarter: f64,
    ) -> Vec<ProcessRunInvocation> {
        let mut invocations = Vec::new();
        let channel_snapshot = self.channels.clone();
        let handle_snapshot = self.handle_to_runtime.clone();
        let instance_snapshot = self.instances.clone();
        for instance in &mut self.instances {
            if !instance.running {
                continue;
            }
            let Some(source) = instance.run_source.clone() else {
                continue;
            };
            let seed_policy = self
                .defs
                .get(&instance.class_name)
                .map(|def| def.seed_policy)
                .unwrap_or_default();
            let ports = self
                .defs
                .get(&instance.class_name)
                .map(|def| def.ports.clone())
                .unwrap_or_default();
            if instance.one_shot {
                let Some(target_beat) = instance.one_shot_target_beat else {
                    continue;
                };
                if target_beat <= start_beats || target_beat > end_beats {
                    continue;
                }
                let sample_offset = ((target_beat - start_beats) * samples_per_quarter)
                    .round()
                    .max(0.0) as u64;
                invocations.push(ProcessRunInvocation {
                    runtime_id: instance.runtime_id,
                    source,
                    beat: target_beat,
                    sample_time: block_start_sample.saturating_add(sample_offset),
                    inlets: resolve_inlets(
                        &instance.inlets,
                        &channel_snapshot,
                        &handle_snapshot,
                        &instance_snapshot,
                    ),
                    state: instance.state.clone(),
                    event: None,
                    step_context: None,
                    ports: ports.clone(),
                    reads: ProcessReadSnapshot::default(),
                    seed: process_rng_seed(
                        instance.runtime_id,
                        seed_policy,
                        ProcessRngPosition::Temporal { beat: target_beat },
                    ),
                });
                continue;
            }
            let Some(clock) = instance.clock.as_mut() else {
                continue;
            };
            let runtime_id = instance.runtime_id;
            let class_seed_policy = seed_policy;
            let inlets = resolve_inlets(
                &instance.inlets,
                &channel_snapshot,
                &handle_snapshot,
                &instance_snapshot,
            );
            let state = instance.state.clone();
            process_grid_boundaries(
                clock,
                start_beats,
                end_beats,
                block_start_sample,
                samples_per_quarter,
                |beat, _idx, sample_time| {
                    invocations.push(ProcessRunInvocation {
                        runtime_id,
                        source: source.clone(),
                        beat,
                        sample_time,
                        inlets: inlets.clone(),
                        state: state.clone(),
                        event: None,
                        step_context: None,
                        ports: ports.clone(),
                        reads: ProcessReadSnapshot::default(),
                        seed: process_rng_seed(
                            runtime_id,
                            class_seed_policy,
                            ProcessRngPosition::Temporal { beat },
                        ),
                    });
                },
            );
        }
        invocations.sort_by_key(|invocation| (invocation.sample_time, invocation.runtime_id));
        invocations
    }

    pub fn take_conductor_invocations_before(&mut self, beat: f64) -> Vec<ProcessRunInvocation> {
        self.take_conductor_invocations(beat, false)
    }

    pub fn take_conductor_invocations_through(&mut self, beat: f64) -> Vec<ProcessRunInvocation> {
        self.take_conductor_invocations(beat, true)
    }

    fn take_conductor_invocations(
        &mut self,
        beat: f64,
        inclusive: bool,
    ) -> Vec<ProcessRunInvocation> {
        let mut due = Vec::new();
        while self.pending_conductor_ticks.front().is_some_and(|tick| {
            if inclusive {
                tick.beat <= beat + 1e-9
            } else {
                tick.beat < beat - 1e-9
            }
        }) {
            if let Some(tick) = self.pending_conductor_ticks.pop_front() {
                due.push(tick);
            }
        }
        let channel_snapshot = self.channels.clone();
        let handle_snapshot = self.handle_to_runtime.clone();
        let instance_snapshot = self.instances.clone();
        let mut invocations = Vec::new();
        for tick in due {
            for conductor in self.conductors.clone() {
                if !conductor
                    .observe_tracks
                    .iter()
                    .any(|track| tick.fired_tracks.contains(track))
                {
                    continue;
                }
                let Some(runtime_id) = self
                    .handle_to_runtime
                    .get(&conductor.process_handle_id)
                    .copied()
                else {
                    continue;
                };
                let Some(instance) = self
                    .instances
                    .iter()
                    .find(|instance| instance.runtime_id == runtime_id)
                else {
                    continue;
                };
                let Some(source) = instance.run_source.clone() else {
                    continue;
                };
                let ports = self
                    .defs
                    .get(&instance.class_name)
                    .map(|def| def.ports.clone())
                    .unwrap_or_default();
                let seed_policy = self
                    .defs
                    .get(&instance.class_name)
                    .map(|def| def.seed_policy)
                    .unwrap_or_default();
                invocations.push(ProcessRunInvocation {
                    runtime_id,
                    source,
                    beat: tick.beat,
                    sample_time: tick.sample_time,
                    inlets: resolve_inlets(
                        &instance.inlets,
                        &channel_snapshot,
                        &handle_snapshot,
                        &instance_snapshot,
                    ),
                    state: instance.state.clone(),
                    event: None,
                    step_context: None,
                    ports,
                    reads: self.conductor_read_snapshot(
                        tick.beat,
                        &conductor.observe_tracks,
                        &conductor.play_tracks,
                    ),
                    seed: process_rng_seed(
                        runtime_id,
                        seed_policy,
                        ProcessRngPosition::Temporal { beat: tick.beat },
                    ),
                });
            }
        }
        invocations.sort_by_key(|invocation| (invocation.sample_time, invocation.runtime_id));
        invocations
    }

    pub fn apply_run_result(&mut self, result: ProcessRunResult) -> Vec<ProcessRunInvocation> {
        let mut invocations = Vec::new();
        let Some(pos) = self
            .instances
            .iter()
            .position(|instance| instance.runtime_id == result.runtime_id)
        else {
            if self.step_process_runtime_ids.contains(&result.runtime_id) {
                let mut state = result.state;
                let mut channel_sends = Vec::new();
                let mut field_suggestions = Vec::new();
                for output in result.outputs {
                    if let Some(channel) = output.name.strip_prefix("__chan:") {
                        channel_sends.push((channel.to_string(), output.value));
                    } else if let Some(field) = output.name.strip_prefix("__field:") {
                        field_suggestions.push((field.to_string(), output.value));
                    } else {
                        state.insert(output.name, output.value);
                    }
                }
                self.step_process_states
                    .insert(ProcessInstanceId(result.runtime_id), state);
                for (channel, value) in channel_sends {
                    invocations.extend(self.send_channel_at(
                        &channel,
                        value,
                        result.beat,
                        result.sample_time,
                    ));
                }
                for (field, value) in field_suggestions {
                    invocations.extend(self.suggest_field_at(
                        &field,
                        value,
                        result.beat,
                        result.sample_time,
                    ));
                }
                for mut event in result.emissions {
                    let beat = result.beat + event.offset_beats.max(0.0) as f64;
                    event.offset_beats = 0.0;
                    self.pending_events.push(PendingProcessEvent {
                        process_runtime_id: result.runtime_id,
                        beat,
                        event: ProcessScheduledEvent::Emission(event),
                    });
                }
            }
            return invocations;
        };
        if let Some(transpose) = result.transpose {
            self.global_transpose = transpose;
        }
        let mut propagated_outputs = Vec::new();
        let mut channel_sends = Vec::new();
        let mut field_suggestions = Vec::new();
        {
            let instance = &mut self.instances[pos];
            instance.state = result.state;
            for output in result.outputs {
                if let Some(channel) = output.name.strip_prefix("__chan:") {
                    channel_sends.push((channel.to_string(), output.value));
                    continue;
                }
                if let Some(field) = output.name.strip_prefix("__field:") {
                    field_suggestions.push((field.to_string(), output.value));
                    continue;
                }
                instance
                    .outlets
                    .insert(output.name.clone(), output.value.clone());
                propagated_outputs.push((
                    ProcessSourceRef::Outlet(ProcessOutletRef {
                        process_handle_id: instance.handle_id,
                        outlet: output.name,
                    }),
                    output.value,
                ));
            }
            if instance.one_shot {
                instance.running = false;
            }
        }
        for (source, value) in propagated_outputs {
            invocations.extend(self.propagate_source_at(
                source,
                value,
                result.beat,
                result.sample_time,
                None,
            ));
        }
        for (channel, value) in channel_sends {
            invocations.extend(self.send_channel_at(
                &channel,
                value,
                result.beat,
                result.sample_time,
            ));
        }
        for (field, value) in field_suggestions {
            invocations.extend(self.suggest_field_at(
                &field,
                value,
                result.beat,
                result.sample_time,
            ));
        }
        for mut event in result.emissions {
            let beat = result.beat + event.offset_beats.max(0.0) as f64;
            event.offset_beats = 0.0;
            self.pending_events.push(PendingProcessEvent {
                process_runtime_id: result.runtime_id,
                beat,
                event: ProcessScheduledEvent::Emission(event),
            });
        }
        invocations
    }

    pub fn send_channel_at(
        &mut self,
        name: &str,
        value: Value,
        beat: f64,
        sample_time: u64,
    ) -> Vec<ProcessRunInvocation> {
        let channel = self
            .channels
            .entry(name.to_string())
            .or_insert(ChannelState {
                name: name.to_string(),
                value: None,
                message_only: false,
                field_publications: VecDeque::new(),
            });
        if !channel.message_only {
            channel.value = Some(value.clone());
        }
        self.propagate_source_at(
            ProcessSourceRef::Channel(name.to_string()),
            value,
            beat,
            sample_time,
            None,
        )
    }

    pub fn suggest_field_at(
        &mut self,
        name: &str,
        value: Value,
        beat: f64,
        sample_time: u64,
    ) -> Vec<ProcessRunInvocation> {
        let channel = self
            .channels
            .entry(name.to_string())
            .or_insert(ChannelState {
                name: name.to_string(),
                value: None,
                message_only: false,
                field_publications: VecDeque::new(),
            });
        channel.value = Some(value.clone());
        channel.field_publications.push_back(TimedFieldValue {
            beat,
            value: value.clone(),
        });
        while channel.field_publications.len() > PROCESS_READ_HISTORY_DEPTH {
            channel.field_publications.pop_front();
        }
        self.propagate_source_at(
            ProcessSourceRef::Channel(name.to_string()),
            value,
            beat,
            sample_time,
            None,
        )
    }

    pub fn schedule_step_event_at(
        &mut self,
        process_runtime_id: u64,
        beat: f64,
        event: ProcessScheduledStepEvent,
    ) {
        self.pending_events.push(PendingProcessEvent {
            process_runtime_id,
            beat,
            event: ProcessScheduledEvent::Step(event),
        });
    }

    pub fn take_due_events(&mut self, up_to_beat: f64) -> Vec<ProcessScheduledItem> {
        let mut due = Vec::new();
        let mut i = 0;
        while i < self.pending_events.len() {
            if self.pending_events[i].beat <= up_to_beat {
                let pending = self.pending_events.swap_remove(i);
                due.push(ProcessScheduledItem {
                    process_runtime_id: pending.process_runtime_id,
                    beat: pending.beat,
                    event: pending.event,
                });
            } else {
                i += 1;
            }
        }
        due.sort_by(|a, b| {
            a.beat
                .partial_cmp(&b.beat)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.process_runtime_id.cmp(&b.process_runtime_id))
        });
        due
    }

    pub fn take_due_emissions(&mut self, up_to_beat: f64) -> Vec<ProcessScheduledEmission> {
        let mut emissions = Vec::new();
        let mut requeue = Vec::new();
        for item in self.take_due_events(up_to_beat) {
            match item.event {
                ProcessScheduledEvent::Emission(event) => {
                    emissions.push(ProcessScheduledEmission {
                        process_runtime_id: item.process_runtime_id,
                        beat: item.beat,
                        event,
                    });
                }
                ProcessScheduledEvent::Step(event) => {
                    requeue.push(PendingProcessEvent {
                        process_runtime_id: item.process_runtime_id,
                        beat: item.beat,
                        event: ProcessScheduledEvent::Step(event),
                    });
                }
            }
        }
        self.pending_events.extend(requeue);
        emissions
    }

    fn propagate_source_at(
        &mut self,
        source: ProcessSourceRef,
        value: Value,
        beat: f64,
        sample_time: u64,
        step_context: Option<ProcessStepEventContext>,
    ) -> Vec<ProcessRunInvocation> {
        let mut invocations = Vec::new();
        let patches = self.patches.clone();
        for patch in patches {
            if patch.source != source {
                continue;
            }
            match patch.target {
                ProcessTargetRef::Inlet {
                    process_handle_id,
                    inlet,
                } => {
                    if let Some(runtime_id) = self.handle_to_runtime.get(&process_handle_id) {
                        if let Some(instance) = self
                            .instances
                            .iter_mut()
                            .find(|instance| instance.runtime_id == *runtime_id)
                        {
                            instance
                                .inlets
                                .insert(inlet, ProcessInletValue::Literal(value.clone()));
                        }
                    }
                }
                ProcessTargetRef::Channel(name) => {
                    invocations.extend(self.send_channel_at(
                        &name,
                        value.clone(),
                        beat,
                        sample_time,
                    ));
                }
            }
        }
        invocations.extend(self.listener_invocations_for_source(
            source,
            value,
            beat,
            sample_time,
            step_context,
        ));
        invocations
    }

    fn listener_invocations_for_source(
        &self,
        source: ProcessSourceRef,
        value: Value,
        beat: f64,
        sample_time: u64,
        step_context: Option<ProcessStepEventContext>,
    ) -> Vec<ProcessRunInvocation> {
        let channel_snapshot = self.channels.clone();
        let handle_snapshot = self.handle_to_runtime.clone();
        let instance_snapshot = self.instances.clone();
        let mut invocations = Vec::new();
        for instance in &self.instances {
            if !instance.running {
                continue;
            }
            for listen in &instance.listens {
                if !listen.source.matches_process_source(&source) {
                    continue;
                }
                invocations.push(ProcessRunInvocation {
                    runtime_id: instance.runtime_id,
                    source: listen.handler_source.clone(),
                    beat,
                    sample_time,
                    inlets: resolve_inlets(
                        &instance.inlets,
                        &channel_snapshot,
                        &handle_snapshot,
                        &instance_snapshot,
                    ),
                    state: instance.state.clone(),
                    event: Some(value.clone()),
                    step_context: step_context.clone(),
                    ports: self
                        .defs
                        .get(&instance.class_name)
                        .map(|def| def.ports.clone())
                        .unwrap_or_default(),
                    reads: ProcessReadSnapshot::default(),
                    seed: process_rng_seed(
                        instance.runtime_id,
                        self.defs
                            .get(&instance.class_name)
                            .map(|def| def.seed_policy)
                            .unwrap_or_default(),
                        ProcessRngPosition::Temporal { beat },
                    ),
                });
            }
        }
        invocations
    }

    pub fn track_fires_at(
        &self,
        track: usize,
        value: Value,
        beat: f64,
        sample_time: u64,
        step_context: ProcessStepEventContext,
    ) -> Vec<ProcessRunInvocation> {
        self.listener_invocations_for_source(
            ProcessSourceRef::TrackFires(track),
            value,
            beat,
            sample_time,
            Some(step_context),
        )
    }

    pub fn step_process_writes(
        &self,
        slot: &TrackProcessSlot,
        step: usize,
        cycle: u64,
        cycle_len: usize,
    ) -> Vec<ProcessTargetWrite> {
        self.step_process_writes_with_inlet_writes(slot, step, cycle, cycle_len, None)
    }

    pub fn step_process_writes_with_inlet_writes(
        &self,
        slot: &TrackProcessSlot,
        step: usize,
        cycle: u64,
        cycle_len: usize,
        inlet_writes: Option<&BTreeMap<String, Vec<ProcessInletWrite>>>,
    ) -> Vec<ProcessTargetWrite> {
        if !slot.enabled {
            return Vec::new();
        }
        let Some(def) = self.defs.get(&slot.class_name) else {
            return Vec::new();
        };
        let Some(accumulator) = def.accumulator.as_ref() else {
            return Vec::new();
        };
        let Some(port) = def.ports.first().cloned() else {
            return Vec::new();
        };
        let amount_default = def
            .inlets
            .iter()
            .find(|inlet| inlet.name == accumulator.amount_inlet)
            .and_then(|inlet| match &inlet.default {
                Value::Number(value) => Some(*value as f32),
                _ => None,
            })
            .unwrap_or(0.0);
        let reset_default = accumulator
            .reset_inlet
            .as_ref()
            .and_then(|name| {
                def.inlets
                    .iter()
                    .find(|inlet| inlet.name == *name)
                    .and_then(|inlet| match &inlet.default {
                        Value::Number(value) => Some(*value as f32),
                        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
                        _ => None,
                    })
            })
            .unwrap_or(0.0);
        let acc = process_accumulator_value_at(
            slot,
            accumulator,
            amount_default,
            reset_default,
            step,
            cycle,
            cycle_len,
            inlet_writes,
        );
        vec![ProcessTargetWrite {
            port: port.name,
            target: port.target,
            op: ProcessTargetOp::Add,
            value: acc,
        }]
    }

    pub fn step_process_invocation(
        &mut self,
        slot: &TrackProcessSlot,
        ctx: ProcessStepRunContext,
    ) -> Option<ProcessRunInvocation> {
        self.step_process_invocation_with_inlet_writes(slot, ctx, None)
    }

    pub fn step_process_invocation_with_inlet_writes(
        &mut self,
        slot: &TrackProcessSlot,
        ctx: ProcessStepRunContext,
        inlet_writes: Option<&BTreeMap<String, Vec<ProcessInletWrite>>>,
    ) -> Option<ProcessRunInvocation> {
        if !slot.enabled {
            return None;
        }
        let def = self.defs.get(&slot.class_name)?.clone();
        if def.accumulator.is_some() {
            return None;
        }
        let source = def.run_source.clone()?;
        let ports = def.ports.clone();
        let seed_policy = def.seed_policy;
        let instance_id = track_process_slot_runtime_id(slot, ctx.track);
        if let Some(name) = slot.instance_name.as_ref() {
            self.step_process_aliases
                .insert(name.clone(), Some(instance_id.0));
        }
        self.step_process_aliases
            .entry(slot.class_name.clone())
            .and_modify(|existing| {
                if *existing != Some(instance_id.0) {
                    *existing = None;
                }
            })
            .or_insert(Some(instance_id.0));
        self.step_process_runtime_ids.insert(instance_id.0);
        let existing_state = self
            .step_process_states
            .remove(&instance_id)
            .unwrap_or_default();
        let state = reconciled_state(existing_state, Some(&def));
        self.step_process_states.insert(instance_id, state.clone());
        Some(ProcessRunInvocation {
            runtime_id: instance_id.0,
            source,
            beat: ctx.beat,
            sample_time: ctx.sample_time,
            inlets: resolve_step_process_inlets(&def, slot, ctx.step, inlet_writes),
            state,
            event: Some(ctx.event),
            step_context: Some(ProcessStepEventContext {
                track: ctx.track,
                step: ctx.step,
                cycle: ctx.cycle,
                beat: ctx.beat,
                sample_time: ctx.sample_time,
                step_beats: ctx.step_beats,
                resolved: ctx.resolved,
            }),
            ports,
            reads: ProcessReadSnapshot::default(),
            seed: process_rng_seed(
                instance_id.0,
                seed_policy,
                ProcessRngPosition::Step {
                    cycle: ctx.cycle,
                    step: ctx.step,
                },
            ),
        })
    }
}

#[derive(Clone, Debug)]
pub struct ProcessStepRunContext {
    pub track: usize,
    pub step: usize,
    pub cycle: u64,
    pub beat: f64,
    pub sample_time: u64,
    pub step_beats: f32,
    pub resolved: ResolvedStep,
    pub event: Value,
}

fn defaulted_inlets(
    def: Option<&ProcessDef>,
    authored: HashMap<String, ProcessInletValue>,
) -> HashMap<String, ProcessInletValue> {
    let mut inlets = HashMap::new();
    if let Some(def) = def {
        for inlet in &def.inlets {
            inlets.insert(
                inlet.name.clone(),
                ProcessInletValue::Literal(inlet.default.clone()),
            );
        }
    }
    for (key, value) in authored {
        inlets.insert(key, value);
    }
    inlets
}

fn reconciled_state(
    existing: HashMap<String, Value>,
    def: Option<&ProcessDef>,
) -> HashMap<String, Value> {
    let mut state = HashMap::new();
    if let Some(def) = def {
        for cell in &def.state {
            state.insert(
                cell.name.clone(),
                existing
                    .get(&cell.name)
                    .cloned()
                    .unwrap_or_else(|| cell.initial.clone()),
            );
        }
    }
    state
}

fn initialize_outlets(instance: &mut ProcessInstance, def: Option<&ProcessDef>) {
    let mut next = HashMap::new();
    if let Some(def) = def {
        for outlet in &def.outlets {
            next.insert(
                outlet.name.clone(),
                instance
                    .outlets
                    .get(&outlet.name)
                    .cloned()
                    .unwrap_or(Value::Nil),
            );
        }
    }
    instance.outlets = next;
}

fn resolve_inlets(
    inlets: &HashMap<String, ProcessInletValue>,
    channels: &HashMap<String, ChannelState>,
    handle_to_runtime: &HashMap<AuthoredHandleId, u64>,
    instances: &[ProcessInstance],
) -> HashMap<String, Value> {
    inlets
        .iter()
        .map(|(name, value)| {
            let resolved = match value {
                ProcessInletValue::Literal(value) => value.clone(),
                ProcessInletValue::Channel(channel) => channels
                    .get(channel)
                    .and_then(|channel| channel.value.clone())
                    .unwrap_or(Value::Nil),
                ProcessInletValue::Outlet(outlet) => handle_to_runtime
                    .get(&outlet.process_handle_id)
                    .and_then(|runtime_id| {
                        instances
                            .iter()
                            .find(|instance| instance.runtime_id == *runtime_id)
                    })
                    .and_then(|instance| instance.outlets.get(&outlet.outlet).cloned())
                    .unwrap_or(Value::Nil),
            };
            (name.clone(), resolved)
        })
        .collect()
}

fn resolve_step_process_inlets(
    def: &ProcessDef,
    slot: &TrackProcessSlot,
    step: usize,
    inlet_writes: Option<&BTreeMap<String, Vec<ProcessInletWrite>>>,
) -> HashMap<String, Value> {
    let mut inlets = HashMap::new();
    for inlet in &def.inlets {
        let mut value = slot
            .inlets
            .get(&inlet.name)
            .map(ProcessLiteral::to_value)
            .unwrap_or_else(|| inlet.default.clone());
        if inlet.lane {
            if let Some(lane) = slot.lanes.get(&inlet.name) {
                let fallback = match &value {
                    Value::Number(value) => *value as f32,
                    Value::Bool(value) => {
                        if *value {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => 0.0,
                };
                value = Value::Number(lane.value_at(step, fallback) as f64);
            }
        }
        if let Some(writes) = inlet_writes.and_then(|writes| writes.get(&inlet.name)) {
            value = apply_process_inlet_writes(value, writes);
        }
        inlets.insert(inlet.name.clone(), value);
    }
    inlets
}

fn process_value_as_f32(value: &Value) -> Option<f32> {
    match value {
        Value::Number(value) => Some(*value as f32),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn apply_process_inlet_writes(mut value: Value, writes: &[ProcessInletWrite]) -> Value {
    for write in writes {
        value = match write.op {
            ProcessTargetOp::Set => Value::Number(write.value as f64),
            ProcessTargetOp::Add => {
                let current = process_value_as_f32(&value).unwrap_or(0.0);
                Value::Number((current + write.value) as f64)
            }
        };
    }
    value
}

fn process_accumulator_value_at(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    reset_default: f32,
    step: usize,
    cycle: u64,
    cycle_len: usize,
    inlet_writes: Option<&BTreeMap<String, Vec<ProcessInletWrite>>>,
) -> f32 {
    let cycle_len = cycle_len.max(step.saturating_add(1)).max(1);
    let step = step.min(cycle_len - 1);
    if let Some(inlet_writes) = inlet_writes.filter(|writes| !writes.is_empty()) {
        let start = process_accumulator_cycle_start_fallback(
            slot,
            accumulator,
            amount_default,
            reset_default,
            cycle,
            cycle_len,
        );
        let acc = if step == 0 {
            start
        } else {
            fold_process_accumulator_steps(
                slot,
                accumulator,
                amount_default,
                reset_default,
                start,
                0..step,
            )
        };
        return fold_process_accumulator_step_with_inlet_writes(
            slot,
            accumulator,
            amount_default,
            reset_default,
            acc,
            step,
            inlet_writes,
        );
    }
    if accumulator_cycle_has_reset(slot, accumulator, reset_default, cycle_len) {
        let start = if cycle == 0 {
            0.0
        } else {
            fold_process_accumulator_steps(
                slot,
                accumulator,
                amount_default,
                reset_default,
                0.0,
                0..cycle_len,
            )
        };
        return fold_process_accumulator_steps(
            slot,
            accumulator,
            amount_default,
            reset_default,
            start,
            0..(step + 1),
        );
    }

    if accumulator_can_use_linear_cycle_fold(slot, accumulator, amount_default, cycle_len) {
        let cycle_total =
            process_accumulator_amount_sum(slot, accumulator, amount_default, 0..cycle_len);
        let prefix_total =
            process_accumulator_amount_sum(slot, accumulator, amount_default, 0..(step + 1));
        let value = ((cycle as f64) * (cycle_total as f64) + prefix_total as f64) as f32;
        return apply_process_accumulator_range(value, accumulator);
    }

    let start = process_accumulator_cycle_start_fallback(
        slot,
        accumulator,
        amount_default,
        reset_default,
        cycle,
        cycle_len,
    );
    fold_process_accumulator_steps(
        slot,
        accumulator,
        amount_default,
        reset_default,
        start,
        0..(step + 1),
    )
}

fn accumulator_cycle_has_reset(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    reset_default: f32,
    cycle_len: usize,
) -> bool {
    (0..cycle_len)
        .any(|idx| process_accumulator_reset_at(slot, accumulator, reset_default, idx) > 0.5)
}

fn accumulator_can_use_linear_cycle_fold(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    cycle_len: usize,
) -> bool {
    match accumulator.range {
        None => true,
        Some(_) if accumulator.mode == ProcessAccumulatorMode::Wrap => true,
        Some(_) if accumulator.mode == ProcessAccumulatorMode::Clip => {
            let mut saw_positive = false;
            let mut saw_negative = false;
            for idx in 0..cycle_len {
                let amount = process_accumulator_amount_at(slot, accumulator, amount_default, idx);
                saw_positive |= amount > 0.0;
                saw_negative |= amount < 0.0;
                if saw_positive && saw_negative {
                    return false;
                }
            }
            true
        }
        Some(_) => false,
    }
}

fn process_accumulator_cycle_start_fallback(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    reset_default: f32,
    cycle: u64,
    cycle_len: usize,
) -> f32 {
    let mut acc = 0.0_f32;
    let mut completed = 0_u64;
    let mut seen = HashMap::new();
    while completed < cycle {
        // Mixed-sign clip and bounce modes are not reducible to a simple sum
        // because clamping/reflection happens after each lane step. Fold full
        // cycles exactly, skipping repeats when the bounded state cycles.
        let key = canonical_accumulator_bits(acc);
        if let Some(previous) = seen.insert(key, completed) {
            let period = completed.saturating_sub(previous);
            if period > 0 {
                let remaining = cycle - completed;
                let skips = remaining / period;
                if skips > 0 {
                    completed += skips * period;
                    continue;
                }
            }
        }
        acc = fold_process_accumulator_steps(
            slot,
            accumulator,
            amount_default,
            reset_default,
            acc,
            0..cycle_len,
        );
        completed += 1;
    }
    acc
}

fn canonical_accumulator_bits(value: f32) -> u32 {
    if value == 0.0 {
        0.0_f32.to_bits()
    } else {
        value.to_bits()
    }
}

fn fold_process_accumulator_steps(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    reset_default: f32,
    mut acc: f32,
    steps: std::ops::Range<usize>,
) -> f32 {
    for idx in steps {
        let reset = process_accumulator_reset_at(slot, accumulator, reset_default, idx);
        if reset > 0.5 {
            acc = 0.0;
        } else {
            acc += process_accumulator_amount_at(slot, accumulator, amount_default, idx);
        }
        acc = apply_process_accumulator_range(acc, accumulator);
    }
    acc
}

fn fold_process_accumulator_step_with_inlet_writes(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    reset_default: f32,
    mut acc: f32,
    step: usize,
    inlet_writes: &BTreeMap<String, Vec<ProcessInletWrite>>,
) -> f32 {
    let reset = accumulator
        .reset_inlet
        .as_ref()
        .map(|name| process_accumulator_inlet_at(slot, name, reset_default, step, inlet_writes))
        .unwrap_or(0.0);
    if reset > 0.5 {
        acc = 0.0;
    } else {
        acc += process_accumulator_inlet_at(
            slot,
            &accumulator.amount_inlet,
            amount_default,
            step,
            inlet_writes,
        );
    }
    apply_process_accumulator_range(acc, accumulator)
}

fn process_accumulator_amount_sum(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    steps: std::ops::Range<usize>,
) -> f32 {
    steps
        .map(|idx| process_accumulator_amount_at(slot, accumulator, amount_default, idx))
        .sum()
}

fn process_accumulator_amount_at(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    amount_default: f32,
    step: usize,
) -> f32 {
    slot.lanes
        .get(&accumulator.amount_inlet)
        .map(|lane| lane.value_at(step, amount_default))
        .unwrap_or(amount_default)
}

fn process_accumulator_reset_at(
    slot: &TrackProcessSlot,
    accumulator: &ProcessAccumulatorSpec,
    reset_default: f32,
    step: usize,
) -> f32 {
    accumulator
        .reset_inlet
        .as_ref()
        .and_then(|name| {
            slot.lanes
                .get(name)
                .map(|lane| lane.value_at(step, reset_default))
        })
        .unwrap_or(reset_default)
}

fn process_accumulator_inlet_at(
    slot: &TrackProcessSlot,
    inlet: &str,
    default: f32,
    step: usize,
    inlet_writes: &BTreeMap<String, Vec<ProcessInletWrite>>,
) -> f32 {
    let base = slot
        .lanes
        .get(inlet)
        .map(|lane| lane.value_at(step, default))
        .unwrap_or(default);
    let Some(writes) = inlet_writes.get(inlet) else {
        return base;
    };
    process_value_as_f32(&apply_process_inlet_writes(
        Value::Number(base as f64),
        writes,
    ))
    .unwrap_or(base)
}

fn apply_process_accumulator_range(value: f32, accumulator: &ProcessAccumulatorSpec) -> f32 {
    match accumulator.range {
        Some((lo, hi)) => apply_accumulator_range(value, lo, hi, accumulator.mode),
        None => value,
    }
}

fn apply_accumulator_range(value: f32, lo: f32, hi: f32, mode: ProcessAccumulatorMode) -> f32 {
    if !lo.is_finite() || !hi.is_finite() || hi <= lo {
        return value;
    }
    match mode {
        ProcessAccumulatorMode::Clip => value.clamp(lo, hi),
        ProcessAccumulatorMode::Wrap => {
            let span = hi - lo;
            let mut wrapped = (value - lo) % span;
            if wrapped < 0.0 {
                wrapped += span;
            }
            lo + wrapped
        }
        ProcessAccumulatorMode::Bounce => {
            let span = hi - lo;
            let period = span * 2.0;
            let mut phase = (value - lo) % period;
            if phase < 0.0 {
                phase += period;
            }
            if phase <= span {
                lo + phase
            } else {
                hi - (phase - span)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ProcessRngPosition {
    Temporal { beat: f64 },
    Step { cycle: u64, step: usize },
}

fn process_rng_seed(
    runtime_id: u64,
    seed_policy: ProcessSeedPolicy,
    position: ProcessRngPosition,
) -> u64 {
    let mut seed = stable_mix64(runtime_id ^ 0xA2F7_0D64_3B1D_9A91);
    match position {
        ProcessRngPosition::Step { cycle, step } => {
            seed ^= stable_mix64((step as u64).wrapping_add(0xD1B5_4A32_D192_ED03));
            if matches!(seed_policy, ProcessSeedPolicy::PerCycle) {
                seed ^= stable_mix64(cycle.wrapping_add(0x8CB9_2BA7_2F3D_8DD7));
            }
        }
        ProcessRngPosition::Temporal { beat } => {
            if matches!(seed_policy, ProcessSeedPolicy::PerCycle) {
                let cycle = beat.floor().max(0.0) as u64;
                seed ^= stable_mix64(cycle.wrapping_add(0x8CB9_2BA7_2F3D_8DD7));
            }
        }
    }
    if seed == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        seed
    }
}

fn stable_mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

const NAMED_PROCESS_RUNTIME_ID_FLAG: u64 = 1 << 63;

fn named_process_runtime_id(class_name: &str, name: &str) -> u64 {
    stable_process_id(&format!("process-instance:{class_name}:{name}"))
        | NAMED_PROCESS_RUNTIME_ID_FLAG
}

fn runtime_instance_id(instance: &AuthoredProcessInstance) -> u64 {
    if let Some(name) = instance.name.as_deref() {
        named_process_runtime_id(&instance.class_name, name)
    } else {
        instance.handle_id.0
    }
}

fn track_process_slot_runtime_id(slot: &TrackProcessSlot, track: usize) -> ProcessInstanceId {
    let base = if let Some(name) = slot.instance_name.as_deref() {
        named_process_runtime_id(&slot.class_name, name)
    } else {
        slot.instance_id.0
    };
    if slot.project_layer {
        // Project-layer slots share configuration but never state: every
        // track gets its own runtime identity (and therefore its own state
        // map entry and RNG stream).
        ProcessInstanceId(base ^ stable_mix64((track as u64).wrapping_add(0x51ED_2701_A6C3_49B5)))
    } else {
        ProcessInstanceId(base)
    }
}

pub fn stable_process_id(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    if hash == 0 {
        0xA076_1D64_78BD_642F
    } else {
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_values(transpose: f32) -> ProcessResolvedValues {
        let mut values = std::array::from_fn(|index| StepParam::ALL[index].default_value());
        values[StepParam::Transpose.index()] = transpose;
        values
    }

    #[test]
    fn resolved_track_reads_are_previous_tick_sample_and_hold() {
        let mut runtime = ProcessRuntime::default();
        runtime.reset_resolved_track_history(&[read_values(0.0)]);
        runtime.record_track_step_boundary(0, 0.0);
        runtime.record_track_fire(0, 0.0, 0, read_values(3.0));

        let same_tick = runtime.read_snapshot(0.0);
        assert_eq!(
            same_tick.tracks[0].current[StepParam::Transpose.index()],
            0.0
        );
        assert_eq!(
            same_tick.tracks[0].steps[0][StepParam::Transpose.index()],
            0.0
        );

        runtime.record_track_step_boundary(0, 1.0);
        let next_tick = runtime.read_snapshot(1.0);
        assert_eq!(
            next_tick.tracks[0].current[StepParam::Transpose.index()],
            3.0
        );
        assert_eq!(
            next_tick.tracks[0].steps[0][StepParam::Transpose.index()],
            3.0
        );
        assert_eq!(
            next_tick.tracks[0].steps[1][StepParam::Transpose.index()],
            0.0
        );
    }

    #[test]
    fn resolved_track_trigger_history_ignores_grid_gaps_and_is_bounded() {
        let mut runtime = ProcessRuntime::default();
        runtime.reset_resolved_track_history(&[read_values(0.0)]);
        runtime.record_track_fire(0, 0.0, 0, read_values(2.0));
        for beat in 1..4 {
            runtime.record_track_step_boundary(0, beat as f64);
        }
        runtime.record_track_fire(0, 4.0, 4, read_values(7.0));

        let snapshot = runtime.read_snapshot(5.0);
        assert_eq!(
            snapshot.tracks[0].trigs[0][StepParam::Transpose.index()],
            7.0
        );
        assert_eq!(
            snapshot.tracks[0].trigs[1][StepParam::Transpose.index()],
            2.0
        );
        assert_eq!(
            snapshot.tracks[0].steps[0][StepParam::Transpose.index()],
            2.0
        );

        for beat in 5..(PROCESS_READ_HISTORY_DEPTH + 20) {
            runtime.record_track_step_boundary(0, beat as f64);
        }
        assert_eq!(
            runtime.read_snapshot(10_000.0).tracks[0].steps.len(),
            PROCESS_READ_HISTORY_DEPTH
        );
    }

    #[test]
    fn pattern_reset_clears_previous_tick_field_registers() {
        let mut runtime = ProcessRuntime::default();
        runtime.suggest_field_at("density", Value::Number(0.75), 0.0, 0);
        assert_eq!(
            runtime.read_snapshot(1.0).fields.get("density"),
            Some(&Value::Number(0.75))
        );

        runtime.reset_resolved_track_history(&[]);
        assert!(!runtime.read_snapshot(1.0).fields.contains_key("density"));
    }

    fn test_step_context(track: usize) -> ProcessStepEventContext {
        ProcessStepEventContext {
            track,
            step: 0,
            cycle: 0,
            beat: 1.0,
            sample_time: 48_000,
            step_beats: 0.25,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity: 1.0,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
        }
    }

    fn authored_instance(
        handle: u64,
        class_name: &str,
        every: Option<ProcessTimeExpr>,
        one_shot: bool,
    ) -> AuthoredProcessInstance {
        AuthoredProcessInstance {
            handle_id: AuthoredHandleId(handle),
            name: None,
            class_name: class_name.to_string(),
            inlets: HashMap::new(),
            bindings: BTreeMap::new(),
            running: true,
            anonymous: true,
            one_shot,
            every,
            run_source: Some("(emit :track 0)".to_string()),
        }
    }

    #[test]
    fn step_process_rng_seed_is_step_locked_and_optionally_cycle_variant() {
        let runtime_id = 41;
        let locked_step_zero_cycle_zero = process_rng_seed(
            runtime_id,
            ProcessSeedPolicy::Locked,
            ProcessRngPosition::Step { cycle: 0, step: 0 },
        );
        let locked_step_zero_later_cycle = process_rng_seed(
            runtime_id,
            ProcessSeedPolicy::Locked,
            ProcessRngPosition::Step { cycle: 9, step: 0 },
        );
        let locked_step_one = process_rng_seed(
            runtime_id,
            ProcessSeedPolicy::Locked,
            ProcessRngPosition::Step { cycle: 0, step: 1 },
        );

        assert_eq!(
            locked_step_zero_cycle_zero, locked_step_zero_later_cycle,
            "locked process randomness should repeat the same pattern each cycle"
        );
        assert_ne!(
            locked_step_zero_cycle_zero, locked_step_one,
            "different steps need independent deterministic rolls"
        );

        let per_cycle_zero = process_rng_seed(
            runtime_id,
            ProcessSeedPolicy::PerCycle,
            ProcessRngPosition::Step { cycle: 0, step: 0 },
        );
        let per_cycle_one = process_rng_seed(
            runtime_id,
            ProcessSeedPolicy::PerCycle,
            ProcessRngPosition::Step { cycle: 1, step: 0 },
        );
        assert_ne!(
            per_cycle_zero, per_cycle_one,
            "per-cycle process randomness should vary the pattern between cycles"
        );
    }

    #[test]
    fn deferred_step_process_inlet_writes_drop_stale_targets_on_target_track() {
        let mut runtime = ProcessRuntime::default();
        runtime.defer_step_process_inlet_write(
            0,
            ProcessInstanceId(7),
            "amount",
            ProcessInletWrite {
                op: ProcessTargetOp::Set,
                value: 3.0,
            },
        );
        runtime.defer_step_process_inlet_write(
            1,
            ProcessInstanceId(9),
            "amount",
            ProcessInletWrite {
                op: ProcessTargetOp::Set,
                value: 5.0,
            },
        );

        let current = runtime.take_step_process_inlet_writes(0, &TrackProcessChain::default());

        assert!(current.is_empty());
        assert!(
            !runtime.pending_step_inlet_writes.contains_key(&(
                0,
                ProcessInstanceId(7),
                "amount".to_string()
            )),
            "stale writes for the currently firing track should be dropped"
        );
        assert!(
            runtime.pending_step_inlet_writes.contains_key(&(
                1,
                ProcessInstanceId(9),
                "amount".to_string()
            )),
            "writes for other tracks should remain pending"
        );
    }

    #[test]
    fn named_authored_process_runtime_ids_are_stable_across_re_eval_handles() {
        let mut first = authored_instance(1, "counter", Some(ProcessTimeExpr::Beats(1.0)), false);
        first.name = Some("counter-h".to_string());
        first.anonymous = false;
        let mut second = authored_instance(2, "counter", Some(ProcessTimeExpr::Beats(1.0)), false);
        second.name = Some("counter-h".to_string());
        second.anonymous = false;
        let mut renamed = authored_instance(3, "counter", Some(ProcessTimeExpr::Beats(1.0)), false);
        renamed.name = Some("other-counter-h".to_string());
        renamed.anonymous = false;
        let anonymous = authored_instance(4, "counter", Some(ProcessTimeExpr::Beats(1.0)), false);

        assert_eq!(runtime_instance_id(&first), runtime_instance_id(&second));
        assert_ne!(runtime_instance_id(&first), runtime_instance_id(&renamed));
        assert_eq!(runtime_instance_id(&anonymous), 4);
    }

    #[test]
    fn every_runs_on_grid_boundary() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                instances: vec![authored_instance(
                    1,
                    "__anonymous_every",
                    Some(ProcessTimeExpr::Beats(1.0)),
                    false,
                )],
                ..ProcessAuthoringSnapshot::default()
            },
            0.25,
        );

        let invocations = runtime.process_block(0.25, 1.0, 1_000, 48_000.0);

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].beat, 1.0);
        assert_eq!(invocations[0].sample_time, 37_000);
    }

    #[test]
    fn after_runs_once_relative_to_sync_time() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                instances: vec![authored_instance(
                    1,
                    "__anonymous_after",
                    Some(ProcessTimeExpr::Beats(1.0)),
                    true,
                )],
                ..ProcessAuthoringSnapshot::default()
            },
            0.25,
        );

        assert!(runtime.process_block(0.25, 1.0, 1_000, 48_000.0).is_empty());
        let invocations = runtime.process_block(1.0, 1.25, 10_000, 48_000.0);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].beat, 1.25);
        assert_eq!(invocations[0].sample_time, 22_000);

        let followups = runtime.apply_run_result(ProcessRunResult {
            runtime_id: invocations[0].runtime_id,
            beat: invocations[0].beat,
            sample_time: invocations[0].sample_time,
            ..ProcessRunResult::default()
        });
        assert!(followups.is_empty());
        assert!(runtime
            .process_block(1.25, 2.25, 22_000, 48_000.0)
            .is_empty());
    }

    #[test]
    fn outlet_patch_to_channel_invokes_channel_listener() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                defs: vec![
                    ProcessDef {
                        id: 1,
                        name: "source".to_string(),
                        source_path: None,
                        doc: None,
                        inlets: Vec::new(),
                        outlets: vec![ProcessOutletDef {
                            name: "value".to_string(),
                        }],
                        state: Vec::new(),
                        every: None,
                        seed_policy: ProcessSeedPolicy::default(),
                        ports: Vec::new(),
                        accumulator: None,
                        run_source: None,
                        listens: Vec::new(),
                    },
                    ProcessDef {
                        id: 2,
                        name: "listener".to_string(),
                        source_path: None,
                        doc: None,
                        inlets: Vec::new(),
                        outlets: Vec::new(),
                        state: Vec::new(),
                        every: None,
                        seed_policy: ProcessSeedPolicy::default(),
                        ports: Vec::new(),
                        accumulator: None,
                        run_source: None,
                        listens: vec![ProcessListenDef {
                            name: "event".to_string(),
                            source: ProcessEventSource::Channel("drift".to_string()),
                            handler_source: "listener-body".to_string(),
                        }],
                    },
                ],
                instances: vec![
                    AuthoredProcessInstance {
                        handle_id: AuthoredHandleId(7),
                        name: None,
                        class_name: "source".to_string(),
                        inlets: HashMap::new(),
                        bindings: BTreeMap::new(),
                        running: true,
                        anonymous: false,
                        one_shot: false,
                        every: None,
                        run_source: None,
                    },
                    AuthoredProcessInstance {
                        handle_id: AuthoredHandleId(9),
                        name: None,
                        class_name: "listener".to_string(),
                        inlets: HashMap::new(),
                        bindings: BTreeMap::new(),
                        running: true,
                        anonymous: true,
                        one_shot: false,
                        every: None,
                        run_source: None,
                    },
                ],
                channels: vec![AuthoredChannel {
                    handle_id: AuthoredHandleId(8),
                    name: Some("drift".to_string()),
                    initial: Some(Value::Number(0.0)),
                    message_only: false,
                }],
                patches: vec![AuthoredPatch {
                    source: ProcessSourceRef::Outlet(ProcessOutletRef {
                        process_handle_id: AuthoredHandleId(7),
                        outlet: "value".to_string(),
                    }),
                    target: ProcessTargetRef::Channel("drift".to_string()),
                }],
                conductors: Vec::new(),
            },
            0.0,
        );

        let followups = runtime.apply_run_result(ProcessRunResult {
            runtime_id: 7,
            beat: 12.0,
            sample_time: 1234,
            outputs: vec![ProcessOutput {
                name: "value".to_string(),
                value: Value::Number(7.0),
            }],
            ..ProcessRunResult::default()
        });

        assert_eq!(followups.len(), 1);
        assert_eq!(followups[0].runtime_id, 9);
        assert_eq!(followups[0].source, "listener-body");
        assert_eq!(followups[0].beat, 12.0);
        assert_eq!(followups[0].sample_time, 1234);
        assert_eq!(followups[0].event, Some(Value::Number(7.0)));
    }

    #[test]
    fn track_fires_listener_invocations_are_routed_by_track() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                defs: vec![ProcessDef {
                    id: 1,
                    name: "listener".to_string(),
                    source_path: None,
                    doc: None,
                    inlets: Vec::new(),
                    outlets: Vec::new(),
                    state: Vec::new(),
                    every: None,
                    seed_policy: ProcessSeedPolicy::default(),
                    ports: Vec::new(),
                    accumulator: None,
                    run_source: None,
                    listens: vec![ProcessListenDef {
                        name: "fire".to_string(),
                        source: ProcessEventSource::TrackFires(2),
                        handler_source: "listener-body".to_string(),
                    }],
                }],
                instances: vec![AuthoredProcessInstance {
                    handle_id: AuthoredHandleId(11),
                    name: None,
                    class_name: "listener".to_string(),
                    inlets: HashMap::new(),
                    bindings: BTreeMap::new(),
                    running: true,
                    anonymous: false,
                    one_shot: false,
                    every: None,
                    run_source: None,
                }],
                ..ProcessAuthoringSnapshot::default()
            },
            0.0,
        );

        assert!(runtime
            .track_fires_at(1, Value::Number(1.0), 1.0, 48_000, test_step_context(1))
            .is_empty());
        let invocations =
            runtime.track_fires_at(2, Value::Number(7.0), 1.0, 48_000, test_step_context(2));
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].runtime_id, 11);
        assert_eq!(invocations[0].source, "listener-body");
        assert_eq!(invocations[0].event, Some(Value::Number(7.0)));
        assert_eq!(
            invocations[0].step_context.as_ref().map(|ctx| ctx.track),
            Some(2)
        );
    }

    #[test]
    fn accumulator_fold_is_sparse_lane_replay_safe() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                defs: vec![ProcessDef {
                    id: 1,
                    name: "sparse".to_string(),
                    source_path: None,
                    doc: None,
                    inlets: vec![ProcessInletDef {
                        name: "amount".to_string(),
                        kind: ProcessInletKind::Float,
                        min: None,
                        max: None,
                        default: Value::Number(0.0),
                        lane: true,
                        doc: None,
                    }],
                    outlets: Vec::new(),
                    state: Vec::new(),
                    every: None,
                    seed_policy: ProcessSeedPolicy::default(),
                    ports: vec![ProcessPortDef::default_with_target(
                        ProcessTargetHint::StepParam {
                            param: "transpose".to_string(),
                        },
                    )],
                    accumulator: Some(ProcessAccumulatorSpec {
                        amount_inlet: "amount".to_string(),
                        reset_inlet: None,
                        range: None,
                        mode: ProcessAccumulatorMode::Wrap,
                    }),
                    run_source: None,
                    listens: Vec::new(),
                }],
                ..ProcessAuthoringSnapshot::default()
            },
            0.0,
        );
        let slot = TrackProcessSlot {
            instance_id: ProcessInstanceId(55),
            instance_name: None,
            class_name: "sparse".to_string(),
            enabled: true,
            project_layer: false,
            inlets: BTreeMap::new(),
            lanes: BTreeMap::from([(
                "amount".to_string(),
                ProcessLane {
                    values: vec![0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                },
            )]),
            bindings: BTreeMap::new(),
        };

        let first = (0..8)
            .map(|step| runtime.step_process_writes(&slot, step, 0, 8)[0].value)
            .collect::<Vec<_>>();
        let replay = (0..8)
            .map(|step| runtime.step_process_writes(&slot, step, 0, 8)[0].value)
            .collect::<Vec<_>>();
        let second_cycle = (0..8)
            .map(|step| runtime.step_process_writes(&slot, step, 1, 8)[0].value)
            .collect::<Vec<_>>();
        assert_eq!(first, vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
        assert_eq!(replay, first);
        assert_eq!(second_cycle, vec![2.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0, 4.0]);
    }
}
