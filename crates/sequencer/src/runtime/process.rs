//! Scheduler-owned process/channel runtime.
//!
//! Lisp authoring lives in `lisp_host`; this module owns the live musical-time
//! state: process instances, clocks, channels, patches, and pending process
//! emissions. It deliberately does not evaluate Lisp.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use eseqlisp::vm::Value;
use serde::{Deserialize, Serialize};

use crate::accumulator::ResolvedStep;
use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
use crate::lisp_host::EmittedAccumulatorEvent;
use crate::neural::ParamNodeId;
use super::grid_clock::{process_grid_boundaries, GridBoundaryClock};
use crate::scheduled_event::StepEvent;
use crate::sequencer::{StepParam, NUM_PARAMS};

pub const DEFAULT_PROCESS_PORT: &str = "__default";

/// Short label for a `ParamTarget::BusSend` bus id: the two default buses
/// read as the mixer's "A"/"B" send knobs, anything else by id.
pub fn bus_send_label(bus: u64) -> String {
    match bus {
        crate::sequencer::DEFAULT_BUS_A_ID => "A".to_string(),
        crate::sequencer::DEFAULT_BUS_B_ID => "B".to_string(),
        other => format!("bus{other}"),
    }
}
/// Exact retained depth for both grid-step and fired-trigger reads. Keeping a
/// fixed, documented window makes scheduler memory independent of authored
/// process input while covering sixteen bars at sixteenth-note resolution.
pub const PROCESS_READ_HISTORY_DEPTH: usize = 256;

pub type ProcessResolvedValues = [f32; NUM_PARAMS];

/// Pattern data of one step as authored: what `(track n :param :pattern)`
/// reads. Unlike the resolved registers this is the row value itself, before
/// accumulators and other process writes, and it is visible on the same tick
/// the track steps onto it (the Cirklon grab/xpose-by-track model: "the note
/// value from pattern B remains current for as long as it takes the next
/// step to play").
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcessStepPattern {
    /// The step's authored pitch in semitones from the track root: the chord
    /// base note when the step carries chord data, else its transpose p-lock.
    /// Playback sounds a plain step at its resolved transpose and a chord note
    /// at `chord_note + (resolved - step_transpose)`, so this is the part of
    /// the pitch that a note grab replaces.
    pub note: f32,
    pub params: ProcessResolvedValues,
    /// The step's authored chord notes (semitones from the track root), in
    /// step order; only the first `chord_count` entries are meaningful. A
    /// plain step has none, and `pitches` then yields `note` alone.
    pub chord: [f32; PROCESS_STEP_PATTERN_MAX_CHORD],
    pub chord_count: usize,
    /// The bar transpose (Cirklon P3 bar XPOSE) in force on this step's
    /// 16-step page. `note` stays authored — it excludes this — so a plain
    /// `:note` read is the manual's "nte" and `:note+b` is its "nte+B".
    pub bar_transpose: f32,
    /// Whether the step is a trig. An empty step never becomes the pattern a
    /// `:pattern` read sees: the boundary it records carries the last active
    /// step's pattern instead, so grab/xpose/harmony hold the note or chord
    /// that actually played until the source steps onto its next trig.
    pub active: bool,
    /// Pitch-class set of the whole pattern this step belongs to (see
    /// `runtime::harmony::pattern_pitch_class_mask`): the key a `:key
    /// :pattern` read reports. Refreshed on every boundary, empty steps
    /// included, so edits to the source pattern reach a listener at the next
    /// step rather than the next trig.
    pub key_mask: u16,
}

/// Chord notes a step pattern read carries: the voice limit, since a step
/// cannot author more notes than that.
pub const PROCESS_STEP_PATTERN_MAX_CHORD: usize = crate::audio::MAX_VOICES;

impl ProcessStepPattern {
    pub fn from_step_snapshot(
        step: &crate::sequencer::SequencerStepSnapshot,
        bar_transpose: f32,
        key_mask: u16,
    ) -> Self {
        let mut chord = [0.0; PROCESS_STEP_PATTERN_MAX_CHORD];
        let chord_count = step.chord.len().min(PROCESS_STEP_PATTERN_MAX_CHORD);
        chord[..chord_count].copy_from_slice(&step.chord[..chord_count]);
        Self {
            note: step_authored_note(&step.chord, &step.params),
            params: step.params,
            chord,
            chord_count,
            bar_transpose,
            active: step.active,
            key_mask,
        }
    }

    /// Build an active pattern with no chord data.
    pub fn plain(note: f32, params: ProcessResolvedValues) -> Self {
        Self {
            note,
            params,
            chord: [0.0; PROCESS_STEP_PATTERN_MAX_CHORD],
            chord_count: 0,
            bar_transpose: 0.0,
            active: true,
            key_mask: 0,
        }
    }

    /// The manual's "nte+B": the authored note plus this bar's transpose.
    pub fn note_with_bar(&self) -> f32 {
        self.note + self.bar_transpose
    }

    /// Every authored pitch on the step: the chord notes when it has chord
    /// data, else its single note. This is what a `:chord :pattern` read
    /// returns, so a harmony lane can follow monophonic and chord sources
    /// alike.
    pub fn pitches(&self) -> impl Iterator<Item = f32> + '_ {
        let chord = if self.chord_count > 0 {
            &self.chord[..self.chord_count]
        } else {
            std::slice::from_ref(&self.note)
        };
        chord.iter().copied()
    }
}

/// See [`ProcessStepPattern::note`].
pub fn step_authored_note(chord: &[f32], params: &[f32; NUM_PARAMS]) -> f32 {
    chord
        .first()
        .copied()
        .unwrap_or(params[StepParam::Transpose.index()])
}

#[derive(Clone, Debug, Default)]
pub struct ProcessTrackReadSnapshot {
    pub current: ProcessResolvedValues,
    /// Newest boundary first; index `n` implements `:steps-ago n`.
    pub steps: Vec<ProcessResolvedValues>,
    /// Pattern data of the step the track is on as of the read beat (the
    /// latest boundary at or before it). `None` until the track has stepped.
    pub step_pattern: Option<ProcessStepPattern>,
    /// Newest fired trigger first; index `n` implements `:trigs-ago n`.
    pub trigs: Vec<ProcessResolvedValues>,
    /// Beat timestamps aligned with `trigs`, used by bounded window reads.
    pub trig_beats: Vec<f64>,
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
    /// Only step boundaries carry the pattern data of the step entered.
    pattern: Option<ProcessStepPattern>,
}

#[derive(Clone, Debug)]
struct ResolvedTrackHistory {
    base: ProcessResolvedValues,
    current: ProcessResolvedValues,
    steps: VecDeque<TimedResolvedValues>,
    trigs: VecDeque<TimedResolvedValues>,
    /// Pattern of the newest active step recorded so far; what an empty
    /// step's boundary carries in its place.
    held_pattern: Option<ProcessStepPattern>,
}

impl ResolvedTrackHistory {
    fn new(base: ProcessResolvedValues) -> Self {
        Self {
            base,
            current: base,
            steps: VecDeque::new(),
            trigs: VecDeque::new(),
            held_pattern: None,
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
    /// A fixed option list (`:enum ("<" ">" ...)`); the inlet value is the
    /// option index, so bodies read it with `(in :op)` like any number and
    /// wire writes land as an index rounded and clamped to the list.
    Enum(Vec<String>),
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
    RackMacroParam { macro_id: u8 },
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
    RackMacroParam,
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
            Self::RackMacroParam => "rack-macro-param",
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
            Self::RackMacroParam => matches!(hint, ProcessTargetHint::RackMacroParam { .. }),
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
                    | ParamTarget::BusSend { .. }
            ),
            Self::InstrumentParam => matches!(target, ParamTarget::InstrumentParam { .. }),
            Self::EffectParam => matches!(target, ParamTarget::EffectParam { .. }),
            Self::MidiFxParam => matches!(target, ParamTarget::MidiFxParam { .. }),
            Self::ProcessInlet => matches!(target, ParamTarget::ProcessInlet { .. }),
            Self::RackSlotParam => matches!(target, ParamTarget::RackSlotParam { .. }),
            Self::RackSlotInstrumentParam => {
                matches!(target, ParamTarget::RackSlotInstrumentParam { .. })
            }
            Self::RackMacroParam => matches!(target, ParamTarget::RackMacroParam { .. }),
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
            Self::RackMacroParam { .. } => ProcessTargetKind::RackMacroParam,
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
    RackMacroParam {
        macro_id: u8,
    },
    /// A track's send level into a mix bus (the mixer "sends" knobs). The
    /// bus is addressed by its project-stable id, never by graph node id.
    /// Applied on the process's own track, like every other target.
    BusSend {
        bus: u64,
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

/// The value a process OUT port last wrote onto one instrument param, as
/// resolved by the scheduler: `base` is what the step would have used without
/// the process (the stored knob value, or the step's p-lock), `value` is what
/// the instrument actually received, both in stored units. `clamped` is set
/// when the write hit the param's range end, so the UI can mark that the
/// displayed base plus the port value no longer predicts the result.
/// Published for read-only UI display (knob dot / number-picker bar).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessEffectiveParam {
    pub param_idx: usize,
    pub base: f32,
    pub value: f32,
    pub clamped: bool,
}

/// Twin of `ProcessEffectiveParam` for a bus-send write: what the send
/// level resolved to on the last fire, keyed by project bus id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessEffectiveSend {
    pub bus: u64,
    pub base: f32,
    pub value: f32,
    pub clamped: bool,
}

/// One extra target for a process port: the port's value is rescaled from
/// the slot's output range (its `lo`/`hi` inlets, else 0..1) into `lo..hi`
/// and *set* on the target. Lets one generator drive several parameters
/// with their own ranges ("rand → velocity 0.1..1 and → retrig 4..8").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessPortFanout {
    pub target: ParamTarget,
    pub lo: f32,
    pub hi: f32,
}

impl ProcessPortFanout {
    /// An entry whose range equals the writer's range: the default an added
    /// fan-out starts with, and what a lane-to-lane cable stays at.
    pub fn is_identity(&self, source: (f32, f32)) -> bool {
        (self.lo - source.0).abs() <= f32::EPSILON && (self.hi - source.1).abs() <= f32::EPSILON
    }

    /// Rescale `value` from `source` (lo, hi) into this entry's range. An
    /// identity entry passes the value through untouched: a second cable out
    /// of a lane without `lo`/`hi` (source defaults to 0..1) must carry the
    /// same raw value the primary wire does, not a copy clamped to 0..1.
    pub fn scaled(&self, value: f32, source: (f32, f32)) -> f32 {
        if self.is_identity(source) {
            return value;
        }
        let span = source.1 - source.0;
        let norm = if span.abs() > f32::EPSILON {
            ((value - source.0) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.lo + norm * (self.hi - self.lo)
    }
}

/// The range a slot's port value spans, for fan-out scaling: its `lo`/`hi`
/// inlets when both are numbers, else 0..1.
pub fn process_slot_output_range(slot: &TrackProcessSlot) -> (f32, f32) {
    let number = |name: &str| match slot.inlets.get(name) {
        Some(ProcessLiteral::Number(value)) => Some(*value as f32),
        _ => None,
    };
    match (number("lo"), number("hi")) {
        (Some(lo), Some(hi)) if hi > lo => (lo, hi),
        _ => (0.0, 1.0),
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
    /// Extra scaled targets per port, applied after the port's binding.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fanout: BTreeMap<String, Vec<ProcessPortFanout>>,
    /// Ports the user disconnected outright. A port here writes nothing on
    /// fire even when the definition declares a target hint (`bindings` can't
    /// say this: an absent or `None` entry means "use the hint"). Binding or
    /// clearing the port lifts it. Fan-out entries are separate rows and keep
    /// running.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unbound_ports: BTreeSet<String>,
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
/// One track's copy-on-write view of a project-layer slot: any lane, scalar
/// inlet or port binding set here shadows the shared slot on that track only
/// (Cirklon-style per-track aux configuration). Absent keys fall through to
/// the shared slot. Serialized as `{lanes, inlets, bindings}`; the pre-rev-2
/// on-disk form was the bare lane map, which still loads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "ProjectSlotOverrideCompat")]
pub struct ProjectSlotOverride {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lanes: BTreeMap<String, ProcessLane>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inlets: BTreeMap<String, ProcessLiteral>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Option<ParamTarget>>,
    /// Whole-port fan-out lists this track owns (replace the shared list).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fanout: BTreeMap<String, Vec<ProcessPortFanout>>,
    /// Ports this track disconnected (see `TrackProcessSlot::unbound_ports`).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unbound_ports: BTreeSet<String>,
    /// This track's own enable state for the shared slot; `None` follows the
    /// shared slot. Lets one track bypass `prob` while the others keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl ProjectSlotOverride {
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
            && self.inlets.is_empty()
            && self.bindings.is_empty()
            && self.fanout.is_empty()
            && self.unbound_ports.is_empty()
            && self.enabled.is_none()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectSlotOverrideFields {
    #[serde(default)]
    lanes: BTreeMap<String, ProcessLane>,
    #[serde(default)]
    inlets: BTreeMap<String, ProcessLiteral>,
    #[serde(default)]
    bindings: BTreeMap<String, Option<ParamTarget>>,
    #[serde(default)]
    fanout: BTreeMap<String, Vec<ProcessPortFanout>>,
    #[serde(default)]
    unbound_ports: BTreeSet<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ProjectSlotOverrideCompat {
    Current(ProjectSlotOverrideFields),
    Legacy(BTreeMap<String, ProcessLane>),
}

impl From<ProjectSlotOverrideCompat> for ProjectSlotOverride {
    fn from(value: ProjectSlotOverrideCompat) -> Self {
        match value {
            ProjectSlotOverrideCompat::Current(fields) => Self {
                lanes: fields.lanes,
                inlets: fields.inlets,
                bindings: fields.bindings,
                fanout: fields.fanout,
                unbound_ports: fields.unbound_ports,
                enabled: fields.enabled,
            },
            ProjectSlotOverrideCompat::Legacy(lanes) => Self {
                lanes,
                ..Self::default()
            },
        }
    }
}

/// Per-track project-slot overrides, keyed by durable project-slot identity.
pub type ProjectLaneOverrides = BTreeMap<ProcessInstanceId, ProjectSlotOverride>;

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
        let Some(override_) = overrides.get(&project_slot_identity_id(slot)) else {
            continue;
        };
        for (inlet, lane) in &override_.lanes {
            slot.lanes.insert(inlet.clone(), lane.clone());
        }
        for (inlet, value) in &override_.inlets {
            slot.inlets.insert(inlet.clone(), value.clone());
        }
        for (port, target) in &override_.bindings {
            slot.bindings.insert(port.clone(), target.clone());
            // A track that bound the port reconnects it even when the
            // shared slot has it disconnected.
            if target.is_some() {
                slot.unbound_ports.remove(port);
            }
        }
        for (port, entries) in &override_.fanout {
            slot.fanout.insert(port.clone(), entries.clone());
        }
        for port in &override_.unbound_ports {
            slot.unbound_ports.insert(port.clone());
        }
        if let Some(enabled) = override_.enabled {
            slot.enabled = enabled;
        }
    }
}

#[cfg(test)]
mod project_slot_override_tests {
    use super::*;

    #[test]
    fn legacy_bare_lane_map_still_deserializes() {
        let legacy = r#"{"amount":{"values":[0.5,1.0]}}"#;
        let parsed: ProjectSlotOverride = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.lanes["amount"].values, vec![0.5, 1.0]);
        assert!(parsed.inlets.is_empty() && parsed.bindings.is_empty());

        let current = ProjectSlotOverride {
            lanes: parsed.lanes.clone(),
            inlets: BTreeMap::from([("lo".to_string(), ProcessLiteral::Number(2.0))]),
            bindings: BTreeMap::from([(
                "out".to_string(),
                Some(ParamTarget::StepParam { param: "rate".to_string() }),
            )]),
            fanout: BTreeMap::new(),
            unbound_ports: BTreeSet::from(["wire".to_string()]),
            enabled: Some(false),
        };
        let json = serde_json::to_string(&current).unwrap();
        let back: ProjectSlotOverride = serde_json::from_str(&json).unwrap();
        assert_eq!(back, current);
        assert!(parsed.enabled.is_none(), "legacy overrides follow the shared slot");
    }

    #[test]
    fn enabled_override_bypasses_the_shared_slot_for_one_track() {
        let mut chain = default_project_layer();
        let prob = chain
            .slots
            .iter()
            .find(|slot| slot.instance_name.as_deref() == Some("prob"))
            .unwrap();
        let identity = project_slot_identity_id(prob);
        let overrides = ProjectLaneOverrides::from([(
            identity,
            ProjectSlotOverride {
                enabled: Some(false),
                ..Default::default()
            },
        )]);
        apply_project_lane_overrides(&mut chain, &overrides);
        let prob = chain
            .slots
            .iter()
            .find(|slot| slot.instance_name.as_deref() == Some("prob"))
            .unwrap();
        assert!(!prob.enabled);
        assert!(chain.slots.iter().filter(|slot| slot.enabled).count() == chain.slots.len() - 1);
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

pub fn process_param_index_by_tag_or_name(
    descriptor: &EffectDescriptor,
    tag_or_name: &str,
) -> Result<Option<usize>, Vec<String>> {
    descriptor.resolve_param_index_by_tag_or_name(tag_or_name)
}

fn log_ambiguous_process_param(name: &str, candidates: &[String]) {
    eprintln!(
        "process binding load: dropped ambiguous legacy parameter '{name}' (candidates: {})",
        candidates.join(", ")
    );
}

fn slot_param_node_id(slot: &EffectSlotSnapshot, param_idx: usize) -> Option<ParamNodeId> {
    let raw_idx = slot.param_node_indices.get(param_idx).copied()?;
    ParamNodeId::from_slot_param(slot.node_id, slot.modulator_node_id, raw_idx)
}

fn refresh_effect_binding_param_id(
    slot_idx: usize,
    effect_name: &str,
    param_name: &mut String,
    param_id: &mut Option<ParamNodeId>,
    effect_descriptors: &[EffectDescriptor],
    effect_slots: &[EffectSlotSnapshot],
) -> bool {
    let Some(desc) = effect_descriptors.get(slot_idx) else {
        return true;
    };
    if !desc.name.eq_ignore_ascii_case(effect_name) {
        return true;
    }
    let param_idx = match process_param_index_by_tag_or_name(desc, param_name) {
        Ok(Some(index)) => index,
        Ok(None) => return true,
        Err(candidates) => {
            log_ambiguous_process_param(param_name, &candidates);
            return false;
        }
    };
    *param_name = desc.params[param_idx].name.clone();
    let Some(slot) = effect_slots.get(slot_idx) else {
        return true;
    };
    if let Some(updated) = slot_param_node_id(slot, param_idx) {
        *param_id = Some(updated);
    }
    true
}

pub fn refresh_track_process_chain_binding_param_ids(
    chain: &mut TrackProcessChain,
    instrument_descriptor: Option<&EffectDescriptor>,
    instrument_slot: Option<&EffectSlotSnapshot>,
    effect_descriptors: &[EffectDescriptor],
    effect_slots: &[EffectSlotSnapshot],
) {
    for slot in &mut chain.slots {
        for binding in slot.bindings.values_mut() {
            let Some(target) = binding.as_mut() else {
                continue;
            };
            let keep = match target {
                ParamTarget::InstrumentParam { param, param_id } => {
                    let (Some(desc), Some(slot)) = (instrument_descriptor, instrument_slot) else {
                        continue;
                    };
                    match process_param_index_by_tag_or_name(desc, param) {
                        Ok(Some(param_idx)) => {
                            *param = desc.params[param_idx].name.clone();
                            if let Some(updated) = slot_param_node_id(slot, param_idx) {
                                *param_id = Some(updated);
                            }
                            true
                        }
                        Ok(None) => true,
                        Err(candidates) => {
                            log_ambiguous_process_param(param, &candidates);
                            false
                        }
                    }
                }
                ParamTarget::EffectParam {
                    slot,
                    effect,
                    param,
                    param_id,
                } => {
                    refresh_effect_binding_param_id(
                        *slot,
                        effect,
                        param,
                        param_id,
                        effect_descriptors,
                        effect_slots,
                    )
                }
                _ => true,
            };
            if !keep {
                *binding = None;
            }
        }
    }
}

/// Re-resolve instrument bindings after replacing the instrument on a track.
///
/// Unlike project-load refresh, replacement is destructive: a binding whose
/// parameter no longer exists must be removed. Leaving its old `ParamNodeId`
/// in place could make the process write to an unrelated node after the graph
/// is rebound.
pub fn rebind_track_process_chain_instrument_param_ids(
    chain: &mut TrackProcessChain,
    instrument_descriptor: &EffectDescriptor,
    instrument_slot: &EffectSlotSnapshot,
) -> usize {
    let mut dropped = 0;
    for process_slot in &mut chain.slots {
        for binding in process_slot.bindings.values_mut() {
            let Some(ParamTarget::InstrumentParam { param, .. }) = binding.as_ref() else {
                continue;
            };
            let resolved = match process_param_index_by_tag_or_name(instrument_descriptor, param) {
                Ok(Some(param_idx)) => slot_param_node_id(instrument_slot, param_idx)
                    .map(|param_id| (param_idx, param_id)),
                Ok(None) => None,
                Err(candidates) => {
                    log_ambiguous_process_param(param, &candidates);
                    None
                }
            };
            if let Some((param_idx, param_id_value)) = resolved {
                if let Some(ParamTarget::InstrumentParam { param, param_id }) = binding.as_mut() {
                    *param = instrument_descriptor.params[param_idx].name.clone();
                    *param_id = Some(param_id_value);
                }
            } else {
                *binding = None;
                dropped += 1;
            }
        }
    }
    dropped
}

pub fn rebind_track_process_chain_effect_param_ids(
    chain: &mut TrackProcessChain,
    effect_descriptors: &[EffectDescriptor],
    effect_slots: &[EffectSlotSnapshot],
) -> usize {
    let mut dropped = 0;
    for process_slot in &mut chain.slots {
        for binding in process_slot.bindings.values_mut() {
            let Some(ParamTarget::EffectParam {
                slot,
                effect,
                param,
                ..
            }) = binding.as_ref()
            else {
                continue;
            };
            let descriptor = effect_descriptors
                .get(*slot)
                .filter(|descriptor| descriptor.name.eq_ignore_ascii_case(effect));
            let resolved = match descriptor
                .map(|descriptor| process_param_index_by_tag_or_name(descriptor, param))
            {
                Some(Ok(Some(param_idx))) => effect_slots.get(*slot)
                    .and_then(|slot| slot_param_node_id(slot, param_idx))
                    .map(|param_id| (param_idx, param_id)),
                Some(Err(candidates)) => {
                    log_ambiguous_process_param(param, &candidates);
                    None
                }
                Some(Ok(None)) | None => None,
            };
            if let (Some(descriptor), Some((param_idx, param_id_value))) = (descriptor, resolved) {
                if let Some(ParamTarget::EffectParam { param, param_id, .. }) = binding.as_mut() {
                    *param = descriptor.params[param_idx].name.clone();
                    *param_id = Some(param_id_value);
                }
            } else {
                *binding = None;
                dropped += 1;
            }
        }
    }
    dropped
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
            let Ok(Some(param_idx)) = process_param_index_by_tag_or_name(descriptor, param) else {
                continue;
            };
            *param = descriptor.params[param_idx].name.clone();
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
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.retained_heap_bytes()
    }

    fn retained_heap_bytes(&self) -> usize {
        match self {
            Self::Number(_) | Self::Bool(_) | Self::Nil => 0,
            Self::String(value) | Self::Symbol(value) | Self::Keyword(value) => value.capacity(),
            Self::List(items) => {
                items.capacity() * std::mem::size_of::<Self>()
                    + items.iter().map(Self::retained_heap_bytes).sum::<usize>()
            }
            Self::Map(items) => items
                .iter()
                .map(|(key, value)| {
                    std::mem::size_of::<(String, Self)>()
                        + key.capacity()
                        + value.retained_heap_bytes()
                })
                .sum(),
        }
    }

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
            | Value::OverrideDispatcher(_)
            | Value::OverrideOriginal(_)
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

fn channel_values_equal(a: &Value, b: &Value) -> bool {
    match (ProcessLiteral::from_value(a), ProcessLiteral::from_value(b)) {
        (Ok(a), Ok(b)) => a == b,
        // Channel payloads are data, but keep invalidation conservative if an
        // opaque VM value reaches this lower-level runtime API.
        _ => false,
    }
}

fn channel_value_maps_equal(a: &HashMap<String, Value>, b: &HashMap<String, Value>) -> bool {
    a.len() == b.len()
        && a.iter().all(|(name, a_value)| {
            b.get(name)
                .is_some_and(|b_value| channel_values_equal(a_value, b_value))
        })
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
    Graph(crate::graph::GraphControlCommand),
    /// `(roll! rate-index)`: engage the project-wide sequence roll from the
    /// firing step for that step's duration (docs/default-process-lanes-spec.md,
    /// roll lane). The lookahead collects it; the scheduler owns roll state.
    Roll(ProcessRollRequest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessRollRequest {
    /// Index into `Timebase::ROLL_RATES` (the transport roll-rate keys).
    pub rate_index: usize,
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
    /// The step's authored pitch (`step_authored_note`), what `(step-note)`
    /// returns.
    pub note: f32,
    /// Inlets that received a process-inlet write (wire or fan-out) on this
    /// fire. Inlet writes are per fire, so `(in? :a)` is the only way a body
    /// can tell "nothing arrived" from "the default arrived".
    pub written_inlets: Vec<String>,
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
    /// Monotonic generation for channel payload values. Generator-side Jaki
    /// cycle memos include this epoch while length memos deliberately do not.
    payload_epoch: u32,
    patches: Vec<AuthoredPatch>,
    pending_events: Vec<PendingProcessEvent>,
    step_process_states: HashMap<ProcessInstanceId, HashMap<String, Value>>,
    /// Per-runtime-instance history of every numeric state cell, one sample
    /// per fire, newest last. The UI's lane strip draws this as a scope.
    step_process_scopes: HashMap<u64, HashMap<String, VecDeque<f32>>>,
    step_process_scope_epoch: u64,
    step_process_scope_published_epoch: u64,
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
    /// Authored scene baseline, refreshed at each scheduler chunk boundary.
    scene_transpose: f32,
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
        self.scene_transpose + self.global_transpose
    }

    pub fn set_scene_transpose(&mut self, slots: &crate::sequencer::SceneSlotStore) {
        self.scene_transpose = slots.transpose_semitones();
    }

    /// Rebuild process-read aliases from the scheduler's current effective
    /// chains. Alias uniqueness is a property of the whole snapshot, not of
    /// whichever slot happened to fire most recently.
    pub fn sync_step_process_aliases<'a>(
        &mut self,
        chains: impl IntoIterator<Item = (usize, &'a TrackProcessChain)>,
    ) {
        fn register_alias(
            aliases: &mut HashMap<String, Option<u64>>,
            alias: &str,
            runtime_id: u64,
        ) {
            aliases
                .entry(alias.to_string())
                .and_modify(|existing| {
                    if *existing != Some(runtime_id) {
                        *existing = None;
                    }
                })
                .or_insert(Some(runtime_id));
        }

        let mut aliases = HashMap::new();
        let mut runtime_ids = HashSet::new();
        for (track, chain) in chains {
            for slot in chain.slots.iter().filter(|slot| slot.enabled) {
                let runtime_id = track_process_slot_runtime_id(slot, track).0;
                runtime_ids.insert(runtime_id);
                if let Some(name) = slot.instance_name.as_deref() {
                    register_alias(&mut aliases, name, runtime_id);
                }
                register_alias(&mut aliases, &slot.class_name, runtime_id);
            }
        }
        self.step_process_aliases = aliases;
        self.step_process_runtime_ids = runtime_ids;
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
        self.record_track_step_boundary_with_pattern(track, beat, None);
    }

    /// Record a step boundary together with the pattern data of the step the
    /// track just entered, so same-tick `:pattern` reads can see it. An
    /// inactive step's boundary carries the last active step's pattern
    /// instead (`None` until one has played), so reads hold what actually
    /// sounded rather than an empty step's default row values.
    pub fn record_track_step_boundary_with_pattern(
        &mut self,
        track: usize,
        beat: f64,
        pattern: Option<ProcessStepPattern>,
    ) {
        let Some(history) = self.resolved_track_history.get_mut(track) else {
            return;
        };
        let pattern = match pattern {
            Some(pattern) if pattern.active => {
                history.held_pattern = Some(pattern);
                Some(pattern)
            }
            Some(empty) => history.held_pattern.map(|held| ProcessStepPattern {
                key_mask: empty.key_mask,
                ..held
            }),
            None => None,
        };
        history.steps.push_back(TimedResolvedValues {
            beat,
            values: history.current,
            pattern,
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
        history.trigs.push_back(TimedResolvedValues {
            beat,
            values,
            pattern: None,
        });
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
                        let visible_trigs = history
                            .trigs
                            .iter()
                            .rev()
                            .filter(|entry| trig_is_visible(entry.beat))
                            .collect::<Vec<_>>();
                        let trigs = visible_trigs
                            .iter()
                            .map(|entry| entry.values)
                            .collect::<Vec<_>>();
                        let trig_beats = visible_trigs
                            .iter()
                            .map(|entry| entry.beat)
                            .collect::<Vec<_>>();
                        let current = trigs.first().copied().unwrap_or(history.base);
                        let steps = history
                            .steps
                            .iter()
                            .rev()
                            .filter(|entry| step_is_visible(entry.beat))
                            .map(|entry| entry.values)
                            .collect::<Vec<_>>();
                        let step_pattern = history
                            .steps
                            .iter()
                            .rev()
                            .filter(|entry| step_is_visible(entry.beat))
                            .find_map(|entry| entry.pattern);
                        ProcessTrackReadSnapshot {
                            current,
                            steps,
                            step_pattern,
                            trigs,
                            trig_beats,
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
            let visible_trigs = history
                .trigs
                .iter()
                .rev()
                .filter(|entry| entry.beat <= beat + 1e-9)
                .collect::<Vec<_>>();
            let trigs = visible_trigs
                .iter()
                .map(|entry| entry.values)
                .collect::<Vec<_>>();
            let trig_beats = visible_trigs
                .iter()
                .map(|entry| entry.beat)
                .collect::<Vec<_>>();
            track_snapshot.current = trigs.first().copied().unwrap_or(history.base);
            track_snapshot.trigs = trigs;
            track_snapshot.trig_beats = trig_beats;
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

    /// Forget every def-process `:state` cell (lane accumulators' `value`,
    /// rand's `held`, count's `count`, authored state). The next fire starts
    /// each cell from its declared initial value. Called on the stop→play
    /// transition and on an all-tracks accumulator reset; `reset_transport`
    /// deliberately does not do this because scene and pattern switches
    /// call it too and accumulators must ride across those.
    pub fn reset_step_process_states(&mut self) {
        self.step_process_states.clear();
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
        let previous_values = self.channel_values();
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
        if !channel_value_maps_equal(&previous_values, &self.channel_values()) {
            self.payload_epoch = self.payload_epoch.wrapping_add(1);
        }
    }

    /// Last value of every value channel (seeded from `defchan` initials), for
    /// read-only sampling by generator ticks (`chan-get`). Message-only
    /// channels hold no value and are omitted.
    pub fn channel_values(&self) -> HashMap<String, Value> {
        self.channels
            .iter()
            .filter_map(|(name, channel)| {
                channel.value.clone().map(|value| (name.clone(), value))
            })
            .collect()
    }

    /// Thread-safe copy of the held value channels for the scheduler → UI
    /// mirror. Callable and host-only VM values cannot be displayed by an
    /// inline value widget and are omitted rather than crossing threads.
    /// Push this fire's numeric state cells onto the instance's scope ring.
    fn record_step_process_scope(&mut self, runtime_id: u64, state: &HashMap<String, Value>) {
        const SCOPE_LEN: usize = 64;
        let mut touched = false;
        let scope = self.step_process_scopes.entry(runtime_id).or_default();
        for (name, value) in state {
            let sample = match value {
                Value::Number(value) => *value as f32,
                Value::Bool(value) => {
                    if *value {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => continue,
            };
            let ring = scope.entry(name.clone()).or_default();
            if ring.len() >= SCOPE_LEN {
                ring.pop_front();
            }
            ring.push_back(sample);
            touched = true;
        }
        if touched {
            self.step_process_scope_epoch = self.step_process_scope_epoch.wrapping_add(1);
        }
    }

    /// The scope rings, when any fire has landed since the last take.
    pub fn take_step_process_scopes_if_changed(
        &mut self,
    ) -> Option<HashMap<u64, HashMap<String, Vec<f32>>>> {
        if self.step_process_scope_published_epoch == self.step_process_scope_epoch {
            return None;
        }
        self.step_process_scope_published_epoch = self.step_process_scope_epoch;
        Some(
            self.step_process_scopes
                .iter()
                .map(|(id, cells)| {
                    (
                        *id,
                        cells
                            .iter()
                            .map(|(name, ring)| (name.clone(), ring.iter().copied().collect()))
                            .collect(),
                    )
                })
                .collect(),
        )
    }

    pub fn channel_value_literals(&self) -> HashMap<String, ProcessLiteral> {
        self.channels
            .iter()
            .filter_map(|(name, channel)| {
                channel.value.as_ref().and_then(|value| {
                    ProcessLiteral::from_value(value)
                        .ok()
                        .map(|literal| (name.clone(), literal))
                })
            })
            .collect()
    }

    /// Global generation for payload-bearing channel values. It changes only
    /// when the value snapshot changes, not merely when it is republished.
    pub fn payload_epoch(&self) -> u32 {
        self.payload_epoch
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
                self.record_step_process_scope(result.runtime_id, &state);
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
            let changed = channel
                .value
                .as_ref()
                .is_none_or(|current| !channel_values_equal(current, &value));
            channel.value = Some(value.clone());
            if changed {
                self.payload_epoch = self.payload_epoch.wrapping_add(1);
            }
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
        let changed = channel
            .value
            .as_ref()
            .is_none_or(|current| !channel_values_equal(current, &value));
        channel.value = Some(value.clone());
        if changed {
            self.payload_epoch = self.payload_epoch.wrapping_add(1);
        }
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
                note: ctx.note,
                written_inlets: written_step_process_inlets(&def, inlet_writes),
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
    /// See [`ProcessStepEventContext::note`].
    pub note: f32,
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
        if let ProcessInletKind::Enum(options) = &inlet.kind {
            value = clamp_enum_inlet_value(value, options.len());
        }
        inlets.insert(inlet.name.clone(), value);
    }
    inlets
}

/// Names of the inlets that received at least one write on this fire, in
/// definition order.
fn written_step_process_inlets(
    def: &ProcessDef,
    inlet_writes: Option<&BTreeMap<String, Vec<ProcessInletWrite>>>,
) -> Vec<String> {
    let Some(writes) = inlet_writes else {
        return Vec::new();
    };
    def.inlets
        .iter()
        .filter(|inlet| writes.get(&inlet.name).is_some_and(|entries| !entries.is_empty()))
        .map(|inlet| inlet.name.clone())
        .collect()
}

/// An enum inlet holds an option index: round whatever arrived (a wire from
/// an accumulator, a typed value) and clamp it to the option list.
fn clamp_enum_inlet_value(value: Value, option_count: usize) -> Value {
    let Some(number) = process_value_as_f32(&value) else {
        return value;
    };
    let last = option_count.saturating_sub(1) as f32;
    let index = if number.is_finite() { number.round().clamp(0.0, last) } else { 0.0 };
    Value::Number(index as f64)
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

pub(crate) fn named_process_runtime_id(class_name: &str, name: &str) -> u64 {
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

pub fn track_process_slot_runtime_id(slot: &TrackProcessSlot, track: usize) -> ProcessInstanceId {
    let base = if is_track_roster_slot(slot) {
        // Roster display names are minted per track and collide across
        // tracks ("grab 2" on two tracks); the band id is globally unique,
        // so it — not the name hash — is the runtime identity.
        slot.instance_id.0
    } else if let Some(name) = slot.instance_name.as_deref() {
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
    fn saved_process_bindings_alias_unique_names_and_drop_ambiguous_names() {
        let dgen_param = |name: &str, display_name: &str, group: &str, cell_id: usize| {
            crate::lisp_host::DGenParam {
                name: name.to_string(),
                display_name: display_name.to_string(),
                cell_id,
                cell_span: 1,
                default: 0.0,
                min: 0.0,
                max: 1.0,
                unit: None,
                hidden: false,
                group: Some(group.to_string()),
                env: None,
                role: None,
                options: None,
            }
        };
        let descriptor = EffectDescriptor::from_lisp_manifest(
            "namespaced",
            &[
                dgen_param("filter.cutoff", "cutoff", "filter", 0),
                dgen_param("op1.index", "index", "op1", 1),
                dgen_param("op2.index", "index", "op2", 2),
            ],
            0,
            1,
            None,
        );
        let live_slot = crate::effects::EffectSlotState::new(&descriptor, 42);
        let snapshot = EffectSlotSnapshot::capture(&live_slot);
        let mut chain = TrackProcessChain {
            slots: vec![TrackProcessSlot {
                instance_id: ProcessInstanceId(1),
                instance_name: None,
                class_name: "test".to_string(),
                enabled: true,
                project_layer: false,
                inlets: BTreeMap::new(),
                lanes: BTreeMap::new(),
                fanout: Default::default(),
                unbound_ports: Default::default(),
                bindings: BTreeMap::from([
                    ("unique".to_string(), Some(ParamTarget::InstrumentParam {
                        param: "cutoff".to_string(), param_id: None,
                    })),
                    ("ambiguous".to_string(), Some(ParamTarget::InstrumentParam {
                        param: "index".to_string(), param_id: None,
                    })),
                ]),
            }],
        };

        refresh_track_process_chain_binding_param_ids(
            &mut chain, Some(&descriptor), Some(&snapshot), &[], &[],
        );

        assert!(matches!(
            chain.slots[0].bindings["unique"].as_ref(),
            Some(ParamTarget::InstrumentParam { param, param_id: Some(_) })
                if param == "filter.cutoff"
        ));
        assert_eq!(chain.slots[0].bindings["ambiguous"], None);
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
    fn step_pattern_pitches_are_the_chord_or_the_single_note() {
        let plain = ProcessStepPattern::plain(5.0, read_values(5.0));
        assert_eq!(plain.pitches().collect::<Vec<_>>(), vec![5.0]);
        let mut chord = ProcessStepPattern::plain(0.0, read_values(0.0));
        chord.chord[..3].copy_from_slice(&[0.0, 4.0, 7.0]);
        chord.chord_count = 3;
        assert_eq!(chord.pitches().collect::<Vec<_>>(), vec![0.0, 4.0, 7.0]);
    }

    #[test]
    fn step_pattern_reads_hold_the_last_active_step_across_empty_steps() {
        let active = ProcessStepPattern::plain(7.0, read_values(7.0));
        let empty = ProcessStepPattern {
            active: false,
            ..ProcessStepPattern::plain(0.0, read_values(0.0))
        };
        let mut runtime = ProcessRuntime::default();
        runtime.reset_resolved_track_history(&[read_values(0.0)]);
        // Empty steps before any trig read as nothing, not as their defaults.
        runtime.record_track_step_boundary_with_pattern(0, 0.0, Some(empty));
        assert_eq!(runtime.read_snapshot(0.0).tracks[0].step_pattern, None);
        runtime.record_track_step_boundary_with_pattern(0, 1.0, Some(active));
        assert_eq!(
            runtime.read_snapshot(1.0).tracks[0].step_pattern,
            Some(active)
        );
        runtime.record_track_step_boundary_with_pattern(0, 2.0, Some(empty));
        runtime.record_track_step_boundary_with_pattern(0, 3.0, Some(empty));
        assert_eq!(
            runtime.read_snapshot(3.0).tracks[0].step_pattern,
            Some(active),
            "empty steps keep the last trig's pattern in effect"
        );
        let next = ProcessStepPattern::plain(2.0, read_values(2.0));
        runtime.record_track_step_boundary_with_pattern(0, 4.0, Some(next));
        assert_eq!(runtime.read_snapshot(4.0).tracks[0].step_pattern, Some(next));
    }

    #[test]
    fn step_pattern_reads_are_same_tick_and_hold_until_the_next_boundary() {
        let pattern = |note: f32| ProcessStepPattern::plain(note, read_values(0.0));
        let mut runtime = ProcessRuntime::default();
        runtime.reset_resolved_track_history(&[read_values(0.0)]);
        assert_eq!(runtime.read_snapshot(0.0).tracks[0].step_pattern, None);

        // The chunk pre-pass records every boundary in the chunk before any
        // process runs: a boundary at the read beat is visible, a later one
        // is not, and the register holds between boundaries.
        runtime.record_track_step_boundary_with_pattern(0, 0.0, Some(pattern(5.0)));
        runtime.record_track_step_boundary_with_pattern(0, 1.0, Some(pattern(7.0)));
        assert_eq!(
            runtime.read_snapshot(0.0).tracks[0].step_pattern,
            Some(pattern(5.0))
        );
        assert_eq!(
            runtime.read_snapshot(0.5).tracks[0].step_pattern,
            Some(pattern(5.0))
        );
        assert_eq!(
            runtime.read_snapshot(1.0).tracks[0].step_pattern,
            Some(pattern(7.0))
        );
        // A pattern-less boundary (tests, legacy callers) keeps the last one.
        runtime.record_track_step_boundary(0, 2.0);
        assert_eq!(
            runtime.read_snapshot(2.0).tracks[0].step_pattern,
            Some(pattern(7.0))
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

    #[test]
    fn step_process_class_alias_recovers_after_duplicate_is_removed() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                defs: vec![ProcessDef {
                    id: 1,
                    name: "reader-state".to_string(),
                    source_path: None,
                    doc: None,
                    inlets: Vec::new(),
                    outlets: Vec::new(),
                    state: vec![ProcessStateDef {
                        name: "phase".to_string(),
                        initial: Value::Number(0.0),
                    }],
                    every: None,
                    seed_policy: ProcessSeedPolicy::default(),
                    ports: Vec::new(),
                    accumulator: None,
                    run_source: Some("nil".to_string()),
                    listens: Vec::new(),
                }],
                ..ProcessAuthoringSnapshot::default()
            },
            0.0,
        );
        let slot = |instance_id| TrackProcessSlot {
            instance_id: ProcessInstanceId(instance_id),
            instance_name: None,
            class_name: "reader-state".to_string(),
            enabled: true,
            project_layer: false,
            inlets: BTreeMap::new(),
            lanes: BTreeMap::new(),
            fanout: Default::default(),
            unbound_ports: Default::default(),
            bindings: BTreeMap::new(),
        };
        let first = TrackProcessChain {
            slots: vec![slot(11)],
        };
        let second = TrackProcessChain {
            slots: vec![slot(12)],
        };

        runtime.sync_step_process_aliases([(0, &first), (1, &second)]);
        for (track, chain, phase) in [(0, &first, 1.0), (1, &second, 2.0)] {
            let invocation = runtime
                .step_process_invocation(
                    &chain.slots[0],
                    ProcessStepRunContext {
                        track,
                        step: 0,
                        cycle: 0,
                        beat: 0.0,
                        sample_time: 0,
                        step_beats: 0.25,
                        resolved: test_step_context(track).resolved,
                        note: 0.0,
                        event: Value::Nil,
                    },
                )
                .expect("build step process invocation");
            runtime.apply_run_result(ProcessRunResult {
                runtime_id: invocation.runtime_id,
                state: HashMap::from([("phase".to_string(), Value::Number(phase))]),
                ..ProcessRunResult::default()
            });
        }
        assert!(
            !runtime
                .read_snapshot(1.0)
                .process_values
                .contains_key("reader-state"),
            "class reads must be inert while more than one active runtime matches"
        );

        runtime.sync_step_process_aliases([(0, &first)]);
        assert_eq!(
            runtime.read_snapshot(1.0).process_values["reader-state"]["phase"],
            Value::Number(1.0),
            "removing the duplicate must restore the remaining class alias"
        );
    }

    fn test_step_context(track: usize) -> ProcessStepEventContext {
        ProcessStepEventContext {
            track,
            step: 0,
            cycle: 0,
            beat: 1.0,
            sample_time: 48_000,
            step_beats: 0.25,
            written_inlets: Vec::new(),
            note: 0.0,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity: 1.0,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
                retrig: crate::sequencer::StepParam::Retrig.default_value(),
                retrig_rate: crate::sequencer::StepParam::RetrigRate.default_value(),
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
            fanout: Default::default(),
            unbound_ports: Default::default(),
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

    #[test]
    fn channel_values_snapshot_holds_value_channels_only() {
        let mut runtime = ProcessRuntime::default();
        runtime.sync_authoring(
            ProcessAuthoringSnapshot {
                channels: vec![
                    AuthoredChannel {
                        handle_id: AuthoredHandleId(1),
                        name: Some("warp".to_string()),
                        initial: Some(Value::Number(1.0)),
                        message_only: false,
                    },
                    AuthoredChannel {
                        handle_id: AuthoredHandleId(2),
                        name: Some("retrig".to_string()),
                        initial: None,
                        message_only: true,
                    },
                ],
                ..ProcessAuthoringSnapshot::default()
            },
            0.0,
        );

        let values = runtime.channel_values();
        assert_eq!(values.get("warp"), Some(&Value::Number(1.0)));
        assert!(!values.contains_key("retrig"));
        assert_eq!(runtime.payload_epoch(), 1);

        runtime.send_channel_at("warp", Value::Number(2.0), 4.0, 1_000);
        assert_eq!(runtime.payload_epoch(), 2);
        runtime.send_channel_at("warp", Value::Number(2.0), 4.0, 1_000);
        runtime.send_channel_at("retrig", Value::Number(1.0), 4.0, 1_000);
        assert_eq!(
            runtime.payload_epoch(),
            2,
            "equal writes and message-only channels do not change the payload snapshot"
        );
        let values = runtime.channel_values();
        assert_eq!(values.get("warp"), Some(&Value::Number(2.0)));
        assert!(!values.contains_key("retrig"));
    }
}

// ---------------------------------------------------------------------------
// Default project lanes (docs/default-process-lanes-spec.md)
// ---------------------------------------------------------------------------

/// Class names of the always-on project layer. The classes themselves are
/// defined in `content/processes/builtin.lisp`.
pub const DEFAULT_LANE_CLASSES: [&str; 11] = [
    "lane-prob",
    "lane-acc",
    "lane-reset",
    "lane-grab",
    "lane-rand",
    "lane-count",
    "lane-cmp",
    "lane-veto",
    "lane-roll",
    "lane-xpose",
    "lane-xpose-b",
];

/// One default lane as installed on every scene's project layer, in chain
/// order. `name` doubles as the dropdown label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefaultLaneSpec {
    pub class_name: &'static str,
    pub name: &'static str,
}

/// The logic lanes (`cmp A`, `cmp B`, `veto`, `roll`) sit after every
/// generator so a wire from any of them lands on the same fire (writes only
/// flow forward within one fire; a backward wire waits for the next).
/// `xpose` (Cirklon "xpose by trk n") is appended after `roll` rather than
/// next to `grab`: default lane ids are index-based, so inserting mid-list
/// would renumber the logic lanes and strand saved wires into them. For the
/// same reason `xpose+b` (Cirklon "xpose by trk n+B") goes on the END rather
/// than beside `xpose`.
pub const DEFAULT_LANES: [DefaultLaneSpec; 14] = [
    DefaultLaneSpec { class_name: "lane-prob", name: "prob" },
    DefaultLaneSpec { class_name: "lane-reset", name: "reset" },
    DefaultLaneSpec { class_name: "lane-rand", name: "rand" },
    DefaultLaneSpec { class_name: "lane-count", name: "count" },
    DefaultLaneSpec { class_name: "lane-acc", name: "tacc" },
    DefaultLaneSpec { class_name: "lane-acc", name: "acc A" },
    DefaultLaneSpec { class_name: "lane-acc", name: "acc B" },
    DefaultLaneSpec { class_name: "lane-grab", name: "grab" },
    DefaultLaneSpec { class_name: "lane-cmp", name: "cmp A" },
    DefaultLaneSpec { class_name: "lane-cmp", name: "cmp B" },
    DefaultLaneSpec { class_name: "lane-veto", name: "veto" },
    DefaultLaneSpec { class_name: "lane-roll", name: "roll" },
    DefaultLaneSpec { class_name: "lane-xpose", name: "xpose" },
    DefaultLaneSpec { class_name: "lane-xpose-b", name: "xpose+b" },
];

/// Default lane slot ids sit in a fixed block below the UI handle base
/// (`1 << 48`) and well inside f64's exact-integer range: instance ids cross
/// into Lisp as numbers, so a 64-bit hash id would round and never match on
/// the way back. Per-track lane overrides still key on the name-derived
/// identity (`project_slot_identity_id`).
const DEFAULT_LANE_INSTANCE_ID_BASE: u64 = 1 << 47;

pub fn default_lane_instance_id(spec: &DefaultLaneSpec) -> ProcessInstanceId {
    let index = DEFAULT_LANES
        .iter()
        .position(|entry| entry.name == spec.name && entry.class_name == spec.class_name)
        .expect("default lane spec") as u64;
    ProcessInstanceId(DEFAULT_LANE_INSTANCE_ID_BASE + index)
}

/// A project-layer slot that belongs to the default lane set.
pub fn is_default_lane_slot(slot: &TrackProcessSlot) -> bool {
    slot.project_layer
        && slot.instance_name.as_deref().is_some_and(|name| {
            DEFAULT_LANES
                .iter()
                .any(|spec| spec.name == name && spec.class_name == slot.class_name)
        })
}

fn default_lane_slot(spec: &DefaultLaneSpec) -> TrackProcessSlot {
    let instance_id = default_lane_instance_id(spec);
    let mut bindings: BTreeMap<String, Option<ParamTarget>> = BTreeMap::new();
    let mut inlets: BTreeMap<String, ProcessLiteral> = BTreeMap::new();
    let mut fanout: BTreeMap<String, Vec<ProcessPortFanout>> = BTreeMap::new();
    let acc_inlet = |name: &str| {
        let target = DEFAULT_LANES
            .iter()
            .find(|entry| entry.name == name)
            .expect("default accumulator lane");
        Some(ParamTarget::ProcessInlet {
            process: target.class_name.to_string(),
            inlet: "reset".to_string(),
            instance_id: Some(default_lane_instance_id(target)),
        })
    };
    match spec.name {
        "tacc" => {
            bindings.insert(
                "out".to_string(),
                Some(ParamTarget::StepParam { param: "transpose".to_string() }),
            );
            bindings.insert("wire".to_string(), None);
        }
        "acc A" => {
            bindings.insert(
                "out".to_string(),
                Some(ParamTarget::StepParam { param: "retrig".to_string() }),
            );
            bindings.insert("wire".to_string(), None);
            inlets.insert("lo".to_string(), ProcessLiteral::Number(0.0));
            inlets.insert("hi".to_string(), ProcessLiteral::Number(8.0));
        }
        "acc B" => {
            bindings.insert(
                "out".to_string(),
                Some(ParamTarget::StepParam { param: "rate".to_string() }),
            );
            bindings.insert("wire".to_string(), None);
            inlets.insert("lo".to_string(), ProcessLiteral::Number(0.0));
            inlets.insert("hi".to_string(), ProcessLiteral::Number(8.0));
        }
        "reset" => {
            // One port, three cables: primary to tacc, fan-out to the
            // generic accumulators (identity 0..1 range, passed through).
            bindings.insert("wire".to_string(), acc_inlet("tacc"));
            let entry = |name: &str| ProcessPortFanout {
                target: acc_inlet(name).expect("default accumulator lane"),
                lo: 0.0,
                hi: 1.0,
            };
            fanout.insert("wire".to_string(), vec![entry("acc A"), entry("acc B")]);
        }
        "rand" | "count" => {
            bindings.insert("out".to_string(), None);
            bindings.insert("wire".to_string(), None);
        }
        _ => {}
    }
    TrackProcessSlot {
        instance_id,
        instance_name: Some(spec.name.to_string()),
        class_name: spec.class_name.to_string(),
        enabled: true,
        project_layer: true,
        inlets,
        lanes: BTreeMap::new(),
        fanout,
        unbound_ports: Default::default(),
        bindings,
    }
}

/// The complete default layer as a fresh chain.
pub fn default_project_layer() -> TrackProcessChain {
    TrackProcessChain {
        slots: DEFAULT_LANES.iter().map(default_lane_slot).collect(),
    }
}

/// Install every default lane that `chain` is missing, keeping any slot the
/// chain already holds (lane edits, manual bindings, user reordering).
/// Missing lanes are appended in default order after the existing slots so a
/// user-authored layer keeps its own ordering. Returns true when the chain
/// changed.
pub fn ensure_default_project_layer(chain: &mut TrackProcessChain) -> bool {
    let mut changed = false;
    for spec in DEFAULT_LANES.iter() {
        let expected_id = default_lane_instance_id(spec);
        match chain.slots.iter_mut().find(|slot| {
            slot.project_layer
                && slot.class_name == spec.class_name
                && slot.instance_name.as_deref() == Some(spec.name)
        }) {
            Some(slot) => {
                // Projects saved with an earlier id scheme keep their lanes
                // but take the current exact id, so Lisp lookups match.
                if slot.instance_id != expected_id {
                    slot.instance_id = expected_id;
                    changed = true;
                }
            }
            None => {
                chain.slots.push(default_lane_slot(spec));
                changed = true;
            }
        }
    }
    // The reset lane's wires must point at the accumulators' current ids,
    // and a project saved with the a/b/c trio (before one port carried
    // several cables) collapses to the single `wire` port + fan-out.
    let fresh_reset = default_lane_slot(&DEFAULT_LANES[1]);
    if let Some(reset) = chain
        .slots
        .iter_mut()
        .find(|slot| slot.project_layer && slot.instance_name.as_deref() == Some("reset"))
    {
        for (port, target) in &fresh_reset.bindings {
            if reset.bindings.get(port) != Some(target) {
                reset.bindings.insert(port.clone(), target.clone());
                changed = true;
            }
        }
        let stale = reset
            .bindings
            .keys()
            .filter(|port| !fresh_reset.bindings.contains_key(*port))
            .cloned()
            .collect::<Vec<_>>();
        for port in stale {
            reset.bindings.remove(&port);
            changed = true;
        }
        for (port, entries) in &fresh_reset.fanout {
            let existing = reset.fanout.entry(port.clone()).or_default();
            for entry in entries {
                if !existing.iter().any(|candidate| candidate.target == entry.target) {
                    existing.push(entry.clone());
                    changed = true;
                }
            }
        }
    }
    changed
}

/// One user-added process slot on a track (eseq-53y7). The roster is the
/// *structure* half of a track's own process chain: which instances the
/// track carries, in what order. It is scene-independent — every pattern of
/// the track runs the same roster — while each pattern keeps its own lane
/// values, inlet literals, port bindings, fan-out and enabled flag inside its
/// `TrackProcessChain`. `reconcile_track_lane_roster` is what joins the two.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackLaneRosterSlot {
    pub instance_id: ProcessInstanceId,
    /// Display name, unique within the track (`grab`, `grab 2`, …).
    pub instance_name: String,
    pub class_name: String,
}

/// One track's ordered roster.
pub type TrackLaneRoster = Vec<TrackLaneRosterSlot>;

/// Instance-id band reserved for track roster slots. Like the default-lane
/// block it sits below the UI handle base (`1 << 48`) and well inside f64's
/// exact-integer range, because instance ids cross into Lisp as numbers.
/// Membership in the band is also what marks a chain slot as roster-owned,
/// so reconciliation can drop a slot the roster no longer lists without
/// touching a chain a `(processes ...)` form authored.
pub const TRACK_ROSTER_INSTANCE_ID_BASE: u64 = 1 << 46;
/// One past the band's last id — the default-lane block starts here.
pub const TRACK_ROSTER_INSTANCE_ID_END: u64 = 1 << 47;

pub fn is_track_roster_instance_id(id: ProcessInstanceId) -> bool {
    (TRACK_ROSTER_INSTANCE_ID_BASE..TRACK_ROSTER_INSTANCE_ID_END).contains(&id.0)
}

/// A roster-owned chain slot carries the band id, so it is identifiable
/// without a name hash. Track slots normally take their runtime identity
/// from `class:name`, but roster names are minted per track and collide
/// across tracks (two tracks can both hold a `grab 2`); the band id is
/// globally unique, so roster slots key runtime state on it instead.
pub fn is_track_roster_slot(slot: &TrackProcessSlot) -> bool {
    !slot.project_layer && is_track_roster_instance_id(slot.instance_id)
}

/// The next free roster instance id given every id already in use.
pub fn next_track_roster_instance_id(
    used: impl IntoIterator<Item = ProcessInstanceId>,
) -> ProcessInstanceId {
    let highest = used
        .into_iter()
        .filter(|id| is_track_roster_instance_id(*id))
        .map(|id| id.0)
        .max();
    ProcessInstanceId(match highest {
        Some(id) => (id + 1).min(TRACK_ROSTER_INSTANCE_ID_END - 1),
        None => TRACK_ROSTER_INSTANCE_ID_BASE,
    })
}

/// The bare lane name a class reads as in the UI: `lane-grab` -> `grab`.
pub fn lane_class_display_name(class_name: &str) -> &str {
    class_name.strip_prefix("lane-").unwrap_or(class_name)
}

/// Mint a per-track-unique instance name for a new `class_name` slot: the
/// bare class name when free, else `name 2`, `name 3`, … . The default
/// project lanes already own their bare names (`grab`, `rand`, …) on every
/// track, so a track's first added grab reads `grab 2`.
pub fn mint_track_roster_instance_name(class_name: &str, taken: &BTreeSet<String>) -> String {
    let base = lane_class_display_name(class_name).to_string();
    if !taken.contains(&base) {
        return base;
    }
    for suffix in 2u32.. {
        let candidate = format!("{base} {suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("instance name space exhausted")
}

/// Every instance name a new roster slot on this track must avoid: the
/// project layer's default lanes plus whatever the roster already holds.
pub fn taken_track_roster_instance_names(roster: &[TrackLaneRosterSlot]) -> BTreeSet<String> {
    DEFAULT_LANES
        .iter()
        .map(|spec| spec.name.to_string())
        .chain(roster.iter().map(|entry| entry.instance_name.clone()))
        .collect()
}

/// A fresh chain slot for a roster entry: structure only, no lane values.
pub fn track_roster_chain_slot(entry: &TrackLaneRosterSlot) -> TrackProcessSlot {
    TrackProcessSlot {
        instance_id: entry.instance_id,
        instance_name: Some(entry.instance_name.clone()),
        class_name: entry.class_name.clone(),
        enabled: true,
        project_layer: false,
        inlets: BTreeMap::new(),
        lanes: BTreeMap::new(),
        bindings: BTreeMap::new(),
        fanout: BTreeMap::new(),
        unbound_ports: BTreeSet::new(),
    }
}

/// Join a track's scene-independent roster into one pattern's chain.
///
/// Append every roster entry the chain is missing, in roster order, at the
/// END of the chain (track slots always run after the project layer, see
/// `compose_effective_process_chain`); keep an existing slot's lanes, inlet
/// literals, bindings, fan-out, unbound ports and enabled flag exactly as
/// the pattern has them; drop roster-owned slots the roster no longer lists.
/// Slots outside the roster band — chains authored by `(processes ...)` —
/// are never touched. Returns true when the chain changed.
///
/// Order is roster-owned too (eseq-53y7.5): slot order is structure, not
/// painted data, so a reorder has to land in every scene the way add and
/// remove do. The rule is a stable partition — the positions roster-owned
/// slots occupy in this chain stay roster positions and are refilled in
/// roster order, while script-authored slots keep their own index and
/// relative placement. A chain with no roster slots is untouched; a chain
/// that only gains slots simply grows at the end.
pub fn reconcile_track_lane_roster(
    chain: &mut TrackProcessChain,
    roster: &[TrackLaneRosterSlot],
) -> bool {
    let mut changed = false;
    let listed = roster
        .iter()
        .map(|entry| entry.instance_id)
        .collect::<BTreeSet<_>>();
    let before = chain.slots.len();
    chain
        .slots
        .retain(|slot| !is_track_roster_slot(slot) || listed.contains(&slot.instance_id));
    changed |= chain.slots.len() != before;
    for entry in roster {
        match chain
            .slots
            .iter_mut()
            .find(|slot| slot.instance_id == entry.instance_id)
        {
            Some(slot) => {
                // The roster owns class and display name; the pattern owns
                // everything else on the slot.
                if slot.class_name != entry.class_name {
                    slot.class_name = entry.class_name.clone();
                    changed = true;
                }
                if slot.instance_name.as_deref() != Some(entry.instance_name.as_str()) {
                    slot.instance_name = Some(entry.instance_name.clone());
                    changed = true;
                }
            }
            None => {
                chain.slots.push(track_roster_chain_slot(entry));
                changed = true;
            }
        }
    }
    // Roster order wins among roster-owned slots. Refill the positions they
    // already occupy, in roster order; script slots never move.
    let rank = roster
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.instance_id, index))
        .collect::<BTreeMap<_, _>>();
    let positions = chain
        .slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| is_track_roster_slot(slot))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if positions.len() > 1 {
        let mut ordered = positions.clone();
        // Stable, so slots the roster does not rank keep their relative order.
        ordered.sort_by_key(|index| {
            rank.get(&chain.slots[*index].instance_id)
                .copied()
                .unwrap_or(usize::MAX)
        });
        if ordered != positions {
            let reordered = ordered
                .iter()
                .map(|index| chain.slots[*index].clone())
                .collect::<Vec<_>>();
            for (position, slot) in positions.iter().zip(reordered) {
                chain.slots[*position] = slot;
            }
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod default_lane_tests {
    use super::*;

    #[test]
    fn enum_inlet_values_round_and_clamp_to_the_option_list() {
        let clamp = |value: f64| clamp_enum_inlet_value(Value::Number(value), 3);
        assert_eq!(clamp(1.4), Value::Number(1.0));
        assert_eq!(clamp(1.6), Value::Number(2.0));
        assert_eq!(clamp(9.0), Value::Number(2.0));
        assert_eq!(clamp(-4.0), Value::Number(0.0));
        assert_eq!(clamp(f64::NAN), Value::Number(0.0));
        assert_eq!(clamp_enum_inlet_value(Value::Nil, 3), Value::Nil);
    }

    #[test]
    fn ensure_default_project_layer_installs_once_and_keeps_edits() {
        let mut chain = TrackProcessChain::default();
        assert!(ensure_default_project_layer(&mut chain));
        assert_eq!(chain.slots.len(), DEFAULT_LANES.len());
        assert!(chain.slots.iter().all(is_default_lane_slot));
        assert!(!ensure_default_project_layer(&mut chain));

        // Edit a lane, drop one slot, reinstall: the edit survives and only
        // the missing slot comes back, appended at the end.
        chain.slots[0]
            .lanes
            .insert("prob".to_string(), ProcessLane { values: vec![0.5] });
        let removed = chain.slots.remove(2);
        assert!(ensure_default_project_layer(&mut chain));
        assert_eq!(chain.slots.len(), DEFAULT_LANES.len());
        assert_eq!(chain.slots[0].lanes["prob"].values, vec![0.5]);
        assert_eq!(chain.slots.last().unwrap().instance_name, removed.instance_name);

        // A slot saved under an older id scheme is renumbered and the reset
        // lane's wires follow it.
        let tacc = chain
            .slots
            .iter_mut()
            .find(|slot| slot.instance_name.as_deref() == Some("tacc"))
            .unwrap();
        tacc.instance_id = ProcessInstanceId(u64::MAX);
        assert!(ensure_default_project_layer(&mut chain));
        let tacc_id = chain
            .slots
            .iter()
            .find(|slot| slot.instance_name.as_deref() == Some("tacc"))
            .unwrap()
            .instance_id;
        assert_eq!(tacc_id, default_lane_instance_id(&DEFAULT_LANES[4]));
        let reset = chain
            .slots
            .iter()
            .find(|slot| slot.instance_name.as_deref() == Some("reset"))
            .unwrap();
        assert!(matches!(
            reset.bindings["wire"],
            Some(ParamTarget::ProcessInlet { instance_id: Some(id), .. }) if id == tacc_id
        ));
    }

    #[test]
    fn reconcile_track_lane_roster_appends_keeps_and_drops() {
        let entry = |id: u64, name: &str| TrackLaneRosterSlot {
            instance_id: ProcessInstanceId(TRACK_ROSTER_INSTANCE_ID_BASE + id),
            instance_name: name.to_string(),
            class_name: "lane-grab".to_string(),
        };
        let roster = vec![entry(0, "grab 2"), entry(1, "grab 3")];

        // A chain authored by `(processes ...)` keeps its slots, and the
        // roster's slots land after them in roster order.
        let mut chain = TrackProcessChain {
            slots: vec![TrackProcessSlot {
                instance_id: ProcessInstanceId(7),
                instance_name: Some("sparse-h".to_string()),
                class_name: "sparse".to_string(),
                enabled: true,
                project_layer: false,
                inlets: BTreeMap::new(),
                lanes: BTreeMap::new(),
                bindings: BTreeMap::new(),
                fanout: BTreeMap::new(),
                unbound_ports: BTreeSet::new(),
            }],
        };
        assert!(reconcile_track_lane_roster(&mut chain, &roster));
        assert_eq!(
            chain
                .slots
                .iter()
                .map(|slot| slot.instance_id)
                .collect::<Vec<_>>(),
            vec![
                ProcessInstanceId(7),
                roster[0].instance_id,
                roster[1].instance_id,
            ]
        );
        assert!(!reconcile_track_lane_roster(&mut chain, &roster));

        // Pattern-owned state on a roster slot survives reconciliation.
        chain.slots[1]
            .lanes
            .insert("value".to_string(), ProcessLane { values: vec![0.5] });
        chain.slots[1]
            .inlets
            .insert("lo".to_string(), ProcessLiteral::Number(3.0));
        chain.slots[1].enabled = false;
        assert!(!reconcile_track_lane_roster(&mut chain, &roster));
        assert_eq!(chain.slots[1].lanes["value"].values, vec![0.5]);
        assert_eq!(
            chain.slots[1].inlets["lo"],
            ProcessLiteral::Number(3.0)
        );
        assert!(!chain.slots[1].enabled);

        // Dropping the first roster entry drops exactly that slot.
        let trimmed = vec![roster[1].clone()];
        assert!(reconcile_track_lane_roster(&mut chain, &trimmed));
        assert_eq!(
            chain
                .slots
                .iter()
                .map(|slot| slot.instance_id)
                .collect::<Vec<_>>(),
            vec![ProcessInstanceId(7), roster[1].instance_id]
        );

        // A missing roster slot comes back empty, not with the old values.
        assert!(reconcile_track_lane_roster(&mut chain, &roster));
        let restored = chain
            .slots
            .iter()
            .find(|slot| slot.instance_id == roster[0].instance_id)
            .expect("roster slot reinstalled");
        assert!(restored.lanes.is_empty());
        assert!(restored.enabled);
        assert_eq!(restored.instance_name.as_deref(), Some("grab 2"));
    }

    #[test]
    fn reconcile_track_lane_roster_puts_roster_slots_in_roster_order() {
        let entry = |id: u64, name: &str| TrackLaneRosterSlot {
            instance_id: ProcessInstanceId(TRACK_ROSTER_INSTANCE_ID_BASE + id),
            instance_name: name.to_string(),
            class_name: "lane-grab".to_string(),
        };
        let script_slot = |id: u64, name: &str| TrackProcessSlot {
            instance_id: ProcessInstanceId(id),
            instance_name: Some(name.to_string()),
            class_name: "sparse".to_string(),
            enabled: true,
            project_layer: false,
            inlets: BTreeMap::new(),
            lanes: BTreeMap::new(),
            bindings: BTreeMap::new(),
            fanout: BTreeMap::new(),
            unbound_ports: BTreeSet::new(),
        };
        // Roster says grab 3 runs before grab 2; the chain was saved the
        // other way round, with a script slot wedged between them.
        let roster = vec![entry(1, "grab 3"), entry(0, "grab 2")];
        let mut chain = TrackProcessChain {
            slots: vec![
                track_roster_chain_slot(&entry(0, "grab 2")),
                script_slot(7, "sparse-h"),
                track_roster_chain_slot(&entry(1, "grab 3")),
            ],
        };
        chain.slots[0]
            .lanes
            .insert("grab".to_string(), ProcessLane { values: vec![1.0] });

        assert!(reconcile_track_lane_roster(&mut chain, &roster));
        assert_eq!(
            chain
                .slots
                .iter()
                .map(|slot| slot.instance_id)
                .collect::<Vec<_>>(),
            vec![
                roster[0].instance_id,
                ProcessInstanceId(7),
                roster[1].instance_id,
            ],
            "roster slots take roster order in the positions they held; the \
             script slot keeps its index"
        );
        // The pattern's painted values travel with their slot, not its index.
        assert_eq!(
            chain.slots[2].lanes["grab"].values,
            vec![1.0],
            "grab 2 must keep the values it was painted with"
        );
        assert!(chain.slots[0].lanes.is_empty());
        assert!(!reconcile_track_lane_roster(&mut chain, &roster));
    }

    #[test]
    fn roster_instance_names_avoid_the_default_lane_names() {
        let mut roster: TrackLaneRoster = Vec::new();
        let mint = |class: &str, roster: &mut TrackLaneRoster| {
            let name = mint_track_roster_instance_name(
                class,
                &taken_track_roster_instance_names(roster),
            );
            roster.push(TrackLaneRosterSlot {
                instance_id: next_track_roster_instance_id(
                    roster.iter().map(|entry| entry.instance_id),
                ),
                instance_name: name.clone(),
                class_name: class.to_string(),
            });
            name
        };
        assert_eq!(mint("lane-grab", &mut roster), "grab 2");
        assert_eq!(mint("lane-grab", &mut roster), "grab 3");
        assert_eq!(mint("sparse", &mut roster), "sparse");
        assert_eq!(
            roster.iter().map(|e| e.instance_id.0).collect::<Vec<_>>(),
            vec![
                TRACK_ROSTER_INSTANCE_ID_BASE,
                TRACK_ROSTER_INSTANCE_ID_BASE + 1,
                TRACK_ROSTER_INSTANCE_ID_BASE + 2,
            ]
        );
        assert!(roster.iter().all(|e| is_track_roster_instance_id(e.instance_id)));
    }

    #[test]
    fn default_reset_lane_wires_every_accumulator_by_identity() {
        let chain = default_project_layer();
        let reset = chain
            .slots
            .iter()
            .find(|slot| slot.instance_name.as_deref() == Some("reset"))
            .unwrap();
        let reset_target = |name: &str| {
            let acc = chain
                .slots
                .iter()
                .find(|slot| slot.instance_name.as_deref() == Some(name))
                .unwrap();
            // Slot ids stay exact through f64; override identity is name-derived.
            assert!(acc.instance_id.0 < (1u64 << 53));
            assert_eq!(
                project_slot_identity_id(acc).0,
                named_process_runtime_id("lane-acc", name)
            );
            ParamTarget::ProcessInlet {
                process: "lane-acc".to_string(),
                inlet: "reset".to_string(),
                instance_id: Some(acc.instance_id),
            }
        };
        // One port, three cables: the primary binding and two fan-out entries.
        assert_eq!(reset.bindings["wire"], Some(reset_target("tacc")));
        let fanout = &reset.fanout["wire"];
        assert_eq!(
            fanout.iter().map(|entry| entry.target.clone()).collect::<Vec<_>>(),
            vec![reset_target("acc A"), reset_target("acc B")]
        );
        assert!(fanout.iter().all(|entry| entry.is_identity((0.0, 1.0))));
    }

    #[test]
    fn ensure_default_project_layer_collapses_the_reset_trio_to_one_port() {
        // A project saved before rev 4 carries reset's a/b/c bindings.
        let mut chain = default_project_layer();
        let reset_index = chain
            .slots
            .iter()
            .position(|slot| slot.instance_name.as_deref() == Some("reset"))
            .unwrap();
        let fresh = chain.slots[reset_index].clone();
        let reset = &mut chain.slots[reset_index];
        let wire = reset.bindings.remove("wire").unwrap();
        let entries = reset.fanout.remove("wire").unwrap();
        reset.bindings.insert("a".to_string(), wire);
        reset
            .bindings
            .insert("b".to_string(), Some(entries[0].target.clone()));
        reset
            .bindings
            .insert("c".to_string(), Some(entries[1].target.clone()));

        assert!(ensure_default_project_layer(&mut chain));
        let reset = &chain.slots[reset_index];
        assert_eq!(reset.bindings, fresh.bindings, "a/b/c dropped, wire restored");
        assert_eq!(reset.fanout, fresh.fanout);
        assert!(!ensure_default_project_layer(&mut chain), "idempotent");
    }
}
