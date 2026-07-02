//! Scheduler-owned process/channel runtime.
//!
//! Lisp authoring lives in `lisp_host`; this module owns the live musical-time
//! state: process instances, clocks, channels, patches, and pending process
//! emissions. It deliberately does not evaluate Lisp.

use std::collections::{BTreeMap, HashMap};

use eseqlisp::vm::Value;

use crate::lisp_host::EmittedAccumulatorEvent;
use crate::neural::{process_grid_boundaries, GridBoundaryClock};

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

#[derive(Clone, Debug)]
pub struct ProcessInletDef {
    pub name: String,
    pub default: Value,
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
    pub inlets: Vec<ProcessInletDef>,
    pub outlets: Vec<ProcessOutletDef>,
    pub state: Vec<ProcessStateDef>,
    pub every: Option<ProcessTimeExpr>,
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

#[derive(Clone, Debug, Default)]
pub struct ProcessAuthoringSnapshot {
    pub defs: Vec<ProcessDef>,
    pub instances: Vec<AuthoredProcessInstance>,
    pub channels: Vec<AuthoredChannel>,
    pub patches: Vec<AuthoredPatch>,
}

#[derive(Clone, Debug, PartialEq)]
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedProcessDef {
    pub id: u64,
    pub name: String,
    pub inlets: Vec<PublishedProcessInletDef>,
    pub outlets: Vec<ProcessOutletDef>,
    pub state: Vec<PublishedProcessStateDef>,
    pub every: Option<ProcessTimeExpr>,
    pub run_source: Option<String>,
    pub listens: Vec<ProcessListenDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedProcessInletDef {
    pub name: String,
    pub default: ProcessLiteral,
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
                        inlets: def
                            .inlets
                            .iter()
                            .map(|inlet| {
                                Ok(PublishedProcessInletDef {
                                    name: inlet.name.clone(),
                                    default: ProcessLiteral::from_value(&inlet.default)?,
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
    }

    pub fn to_runtime(&self) -> ProcessAuthoringSnapshot {
        ProcessAuthoringSnapshot {
            defs: self
                .defs
                .iter()
                .map(|def| ProcessDef {
                    id: def.id,
                    name: def.name.clone(),
                    inlets: def
                        .inlets
                        .iter()
                        .map(|inlet| ProcessInletDef {
                            name: inlet.name.clone(),
                            default: inlet.default.to_value(),
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
}

#[derive(Clone, Debug, Default)]
pub struct ProcessRunResult {
    pub runtime_id: u64,
    pub beat: f64,
    pub sample_time: u64,
    pub state: HashMap<String, Value>,
    pub outputs: Vec<ProcessOutput>,
    pub emissions: Vec<EmittedAccumulatorEvent>,
    pub transpose: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct ProcessOutput {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct ProcessScheduledEmission {
    pub process_runtime_id: u64,
    pub beat: f64,
    pub event: EmittedAccumulatorEvent,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessRuntime {
    defs: HashMap<String, ProcessDef>,
    instances: Vec<ProcessInstance>,
    handle_to_runtime: HashMap<AuthoredHandleId, u64>,
    channels: HashMap<String, ChannelState>,
    patches: Vec<AuthoredPatch>,
    pending_emissions: Vec<PendingProcessEmission>,
    global_transpose: f32,
}

#[derive(Clone, Debug)]
struct PendingProcessEmission {
    process_runtime_id: u64,
    beat: f64,
    event: EmittedAccumulatorEvent,
}

impl ProcessRuntime {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.pending_emissions.is_empty()
    }

    pub fn global_transpose(&self) -> f32 {
        self.global_transpose
    }

    pub fn reset_transport(&mut self, total_beats: f64) {
        self.pending_emissions.clear();
        for instance in &mut self.instances {
            if let Some(clock) = &mut instance.clock {
                clock.realign(total_beats);
            }
        }
    }

    pub fn clear_scene_pending(&mut self) {
        self.pending_emissions.clear();
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
    }

    fn sync_channels(&mut self, channels: Vec<AuthoredChannel>) {
        let mut next = HashMap::new();
        for channel in channels {
            let Some(name) = channel.name else {
                continue;
            };
            let existing = self.channels.remove(&name);
            let value = existing
                .and_then(|existing| existing.value)
                .or(channel.initial.clone());
            next.insert(
                name.clone(),
                ChannelState {
                    name,
                    value,
                    message_only: channel.message_only,
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
                });
                continue;
            }
            let Some(clock) = instance.clock.as_mut() else {
                continue;
            };
            let runtime_id = instance.runtime_id;
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
                    });
                },
            );
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
            return invocations;
        };
        if let Some(transpose) = result.transpose {
            self.global_transpose = transpose;
        }
        let mut propagated_outputs = Vec::new();
        let mut channel_sends = Vec::new();
        {
            let instance = &mut self.instances[pos];
            instance.state = result.state;
            for output in result.outputs {
                if let Some(channel) = output.name.strip_prefix("__chan:") {
                    channel_sends.push((channel.to_string(), output.value));
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
        for mut event in result.emissions {
            let beat = result.beat + event.offset_beats.max(0.0) as f64;
            event.offset_beats = 0.0;
            self.pending_emissions.push(PendingProcessEmission {
                process_runtime_id: result.runtime_id,
                beat,
                event,
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
            });
        if !channel.message_only {
            channel.value = Some(value.clone());
        }
        self.propagate_source_at(
            ProcessSourceRef::Channel(name.to_string()),
            value,
            beat,
            sample_time,
        )
    }

    pub fn take_due_emissions(&mut self, up_to_beat: f64) -> Vec<ProcessScheduledEmission> {
        let mut due = Vec::new();
        let mut i = 0;
        while i < self.pending_emissions.len() {
            if self.pending_emissions[i].beat <= up_to_beat {
                let pending = self.pending_emissions.swap_remove(i);
                due.push(ProcessScheduledEmission {
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

    fn propagate_source_at(
        &mut self,
        source: ProcessSourceRef,
        value: Value,
        beat: f64,
        sample_time: u64,
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
        invocations.extend(self.listener_invocations_for_source(source, value, beat, sample_time));
        invocations
    }

    fn listener_invocations_for_source(
        &self,
        source: ProcessSourceRef,
        value: Value,
        beat: f64,
        sample_time: u64,
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
                });
            }
        }
        invocations
    }
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

fn runtime_instance_id(instance: &AuthoredProcessInstance) -> u64 {
    instance
        .name
        .as_deref()
        .map(stable_process_id)
        .unwrap_or(instance.handle_id.0)
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
            running: true,
            anonymous: true,
            one_shot,
            every,
            run_source: Some("(emit :track 0)".to_string()),
        }
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
                        inlets: Vec::new(),
                        outlets: vec![ProcessOutletDef {
                            name: "value".to_string(),
                        }],
                        state: Vec::new(),
                        every: None,
                        run_source: None,
                        listens: Vec::new(),
                    },
                    ProcessDef {
                        id: 2,
                        name: "listener".to_string(),
                        inlets: Vec::new(),
                        outlets: Vec::new(),
                        state: Vec::new(),
                        every: None,
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
}
