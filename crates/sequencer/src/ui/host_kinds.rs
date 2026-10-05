//! The host kinds of `eseq.kinds` (`content/core/modules/kinds.lisp`),
//! published from the sequencer (docs/kind-bindings-spec.md §3.4, §4, §9;
//! stage 4).
//!
//! The host registers keyed instances from the project model (tracks,
//! scenes, banks, devices), re-keys them on reorder, and pushes their
//! `:host` fields with `Runtime::set_instance_field`. Every push compares
//! with the cell first, so only changed values reach readers and slots.
//!
//! Two feeds keep fields current ([`Feed`]):
//!
//! - **Live** fields read shared sequencer state the UI thread can reach
//!   without the `App` (atomics and shared handles: volume, mute, arm,
//!   selection, step state, playheads, meters, transport). They carry the
//!   observed bit (spec D3): the tick computes one only while
//!   `Runtime::host_fields_observed` says a reader or a held `#'` binding
//!   wants it, and a by-value read of an unobserved one asks the reader hook
//!   ([`install_reader`]) for its current value. Playheads and meters cost no
//!   per-step work while nothing observes them.
//! - **Model** fields come from the `App` (names, colors, presets, device
//!   chains, scenes and banks). The hook cannot reach the `App`, so the tick
//!   keeps every registered instance's model fields current, but only when
//!   the model may have moved: a [`ModelRevision`] built from the existing
//!   change counters (UI/FX epochs, pattern epoch, history revision, scene
//!   revision, track registry, theme tint), like the legacy publishers'
//!   epoch gates. The transport's queued scene and launch quantization are
//!   compared every tick.
//!
//! Track instances are keyed by the track registry's stable `TrackId`s
//! (paired with the registry's generation, so a project load replaces
//! them); scenes and banks by their ids. Steps are positional and lazy
//! (spec D2): keyed (track instance id, step index), registered on the first
//! read of `t.steps` (the reader hook, or the tick while `steps` is
//! observed) and dropped when their track goes or its length shrinks below
//! them. Devices are keyed (track instance id, slot + 1): the instrument is
//! slot -1.
//!
//! [`check_schema`] compares [`PUBLISHED`] with the loaded `eseq.kinds`; the
//! tick re-runs it whenever a kind schema changes (a hot reload) and skips
//! mismatched fields until they are fixed.

use crate::*;
use eseqlisp::vm::{HostFieldReader, InstanceId, VM};
use std::sync::atomic::AtomicBool;
use std::sync::LazyLock;

/// The module declaring the host kinds.
pub(crate) const KINDS_MODULE: &str = "eseq.kinds";

pub(crate) const TRACK: &str = "eseq.kinds:track";
pub(crate) const STEP: &str = "eseq.kinds:step";
pub(crate) const DEVICE: &str = "eseq.kinds:device";
pub(crate) const SCENE: &str = "eseq.kinds:scene";
pub(crate) const BANK: &str = "eseq.kinds:bank";
pub(crate) const TRANSPORT: &str = "eseq.kinds:transport";
pub(crate) const SELECTION: &str = "eseq.kinds:selection";
pub(crate) const PROJECT: &str = "eseq.kinds:project";

/// How the host keeps a field current (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feed {
    /// Re-derived from the `App` for every registered instance whenever the
    /// [`ModelRevision`] moves.
    Model,
    /// Computed only while observed; cold reads go through the reader hook.
    Live,
}

use Feed::{Live, Model};

/// A published field: (kind id, field name).
pub(crate) type FieldKey = (&'static str, &'static str);

/// Every published field, by name. [`PUBLISHED`] lists them all.
pub(crate) mod f {
    use super::*;

    pub(crate) const TRACK_INDEX: FieldKey = (TRACK, "index");
    pub(crate) const TRACK_NAME: FieldKey = (TRACK, "name");
    pub(crate) const TRACK_COLOR: FieldKey = (TRACK, "color");
    pub(crate) const TRACK_VOLUME: FieldKey = (TRACK, "volume");
    pub(crate) const TRACK_PEAK: FieldKey = (TRACK, "peak");
    pub(crate) const TRACK_MUTED: FieldKey = (TRACK, "muted");
    pub(crate) const TRACK_AUDIBLE: FieldKey = (TRACK, "audible");
    pub(crate) const TRACK_ARMED: FieldKey = (TRACK, "armed");
    pub(crate) const TRACK_SELECTED: FieldKey = (TRACK, "selected");
    pub(crate) const TRACK_PRESET: FieldKey = (TRACK, "preset");
    pub(crate) const TRACK_NUM_STEPS: FieldKey = (TRACK, "num-steps");
    pub(crate) const TRACK_STEPS: FieldKey = (TRACK, "steps");
    pub(crate) const TRACK_DEVICES: FieldKey = (TRACK, "devices");

    pub(crate) const STEP_INDEX: FieldKey = (STEP, "index");
    pub(crate) const STEP_TRACK: FieldKey = (STEP, "track");
    pub(crate) const STEP_ACTIVE: FieldKey = (STEP, "active");
    pub(crate) const STEP_PLAYING: FieldKey = (STEP, "playing");
    pub(crate) const STEP_SELECTED: FieldKey = (STEP, "selected");

    pub(crate) const DEVICE_TRACK: FieldKey = (DEVICE, "track");
    pub(crate) const DEVICE_SLOT: FieldKey = (DEVICE, "slot");
    pub(crate) const DEVICE_NAME: FieldKey = (DEVICE, "name");
    pub(crate) const DEVICE_ENABLED: FieldKey = (DEVICE, "enabled");

    pub(crate) const SCENE_INDEX: FieldKey = (SCENE, "index");
    pub(crate) const SCENE_NUMBER: FieldKey = (SCENE, "number");
    pub(crate) const SCENE_NAME: FieldKey = (SCENE, "name");
    pub(crate) const SCENE_ACTIVE: FieldKey = (SCENE, "active");
    pub(crate) const SCENE_QUEUED: FieldKey = (SCENE, "queued");
    pub(crate) const SCENE_BANK: FieldKey = (SCENE, "bank");

    pub(crate) const BANK_INDEX: FieldKey = (BANK, "index");
    pub(crate) const BANK_LABEL: FieldKey = (BANK, "label");
    pub(crate) const BANK_SCENES: FieldKey = (BANK, "scenes");
    pub(crate) const BANK_PLAYING: FieldKey = (BANK, "playing");

    pub(crate) const TRANSPORT_PLAYING: FieldKey = (TRANSPORT, "playing");
    pub(crate) const TRANSPORT_RECORDING: FieldKey = (TRANSPORT, "recording");
    pub(crate) const TRANSPORT_SCENE: FieldKey = (TRANSPORT, "scene");
    pub(crate) const TRANSPORT_QUEUED: FieldKey = (TRANSPORT, "queued");
    pub(crate) const TRANSPORT_LAUNCH_QUANTIZE: FieldKey = (TRANSPORT, "launch-quantize");

    pub(crate) const SELECTION_TRACK: FieldKey = (SELECTION, "track");

    pub(crate) const PROJECT_TRACKS: FieldKey = (PROJECT, "tracks");
    pub(crate) const PROJECT_SCENES: FieldKey = (PROJECT, "scenes");
    pub(crate) const PROJECT_BANKS: FieldKey = (PROJECT, "banks");
}

/// Every kind and `:host` field the host publishes, with its type as
/// `eseq.kinds` spells it and its feed, grouped by kind. [`check_schema`]
/// holds the module to this; the live-field loops and the reserved kind
/// names derive from it.
pub(crate) const PUBLISHED: &[(FieldKey, &str, Feed)] = &[
    (f::TRACK_INDEX, ":int", Model),
    (f::TRACK_NAME, ":string", Model),
    (f::TRACK_COLOR, ":rgb", Model),
    (f::TRACK_VOLUME, ":number", Live),
    (f::TRACK_PEAK, ":number", Live),
    (f::TRACK_MUTED, ":bool", Live),
    (f::TRACK_AUDIBLE, ":bool", Live),
    (f::TRACK_ARMED, ":bool", Live),
    (f::TRACK_SELECTED, ":bool", Live),
    (f::TRACK_PRESET, ":string", Model),
    (f::TRACK_NUM_STEPS, ":int", Live),
    (f::TRACK_STEPS, "(list-of step)", Live),
    (f::TRACK_DEVICES, "(list-of device)", Model),
    (f::STEP_INDEX, ":int", Model),
    (f::STEP_TRACK, "track", Model),
    (f::STEP_ACTIVE, ":bool", Live),
    (f::STEP_PLAYING, ":bool", Live),
    (f::STEP_SELECTED, ":bool", Live),
    (f::DEVICE_TRACK, "track", Model),
    (f::DEVICE_SLOT, ":int", Model),
    (f::DEVICE_NAME, ":string", Model),
    (f::DEVICE_ENABLED, ":bool", Model),
    (f::SCENE_INDEX, ":int", Model),
    (f::SCENE_NUMBER, ":int", Model),
    (f::SCENE_NAME, ":string", Model),
    (f::SCENE_ACTIVE, ":bool", Model),
    (f::SCENE_QUEUED, ":bool", Model),
    (f::SCENE_BANK, "bank", Model),
    (f::BANK_INDEX, ":int", Model),
    (f::BANK_LABEL, ":string", Model),
    (f::BANK_SCENES, "(list-of scene)", Model),
    (f::BANK_PLAYING, ":bool", Model),
    (f::TRANSPORT_PLAYING, ":bool", Live),
    (f::TRANSPORT_RECORDING, ":bool", Live),
    (f::TRANSPORT_SCENE, "scene", Model),
    (f::TRANSPORT_QUEUED, "scene", Model),
    (f::TRANSPORT_LAUNCH_QUANTIZE, ":string", Model),
    (f::SELECTION_TRACK, "track", Live),
    (f::PROJECT_TRACKS, "(list-of track)", Model),
    (f::PROJECT_SCENES, "(list-of scene)", Model),
    (f::PROJECT_BANKS, "(list-of bank)", Model),
];

/// The published kinds, in [`PUBLISHED`] order.
static PUBLISHED_KINDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut kinds: Vec<&'static str> = Vec::new();
    for ((kind, _), _, _) in PUBLISHED {
        if !kinds.contains(kind) {
            kinds.push(kind);
        }
    }
    kinds
});

/// The kind names `eseq.kinds` reserves (spec §3.4): every published kind.
pub(crate) fn host_kind_names() -> Vec<&'static str> {
    PUBLISHED_KINDS
        .iter()
        .map(|kind| eseqlisp::vm::kind_name_of(kind))
        .collect()
}

/// The live fields of one kind, with their names for
/// `Runtime::host_fields_observed` (bit `i` is `keys[i]`).
struct LiveFields {
    keys: Vec<FieldKey>,
    names: Vec<&'static str>,
}

impl LiveFields {
    fn of(kind: &str) -> Self {
        let keys: Vec<FieldKey> = PUBLISHED
            .iter()
            .filter(|((published, _), _, feed)| *published == kind && *feed == Live)
            .map(|(key, _, _)| *key)
            .collect();
        let names = keys.iter().map(|(_, name)| *name).collect();
        Self { keys, names }
    }

    /// The observed-mask bit of `key`.
    fn bit(&self, key: FieldKey) -> u32 {
        let index = self.keys.iter().position(|live| *live == key);
        index.map_or(0, |index| 1 << index)
    }
}

static TRACK_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(TRACK));
static STEP_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(STEP));
static TRANSPORT_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(TRANSPORT));
static SELECTION_LIVE: LazyLock<LiveFields> = LazyLock::new(|| LiveFields::of(SELECTION));

/// Every live field.
static LIVE_KEYS: LazyLock<Vec<FieldKey>> = LazyLock::new(|| {
    PUBLISHED
        .iter()
        .filter(|(_, _, feed)| *feed == Live)
        .map(|(key, _, _)| *key)
        .collect()
});

/// One way the loaded `eseq.kinds` differs from [`PUBLISHED`].
struct Mismatch {
    message: String,
    /// The published fields the host must not push while it stands.
    skip: Vec<FieldKey>,
}

fn schema_mismatches(rt: &Runtime) -> Vec<Mismatch> {
    let mut mismatches = Vec::new();
    for kind in PUBLISHED_KINDS.iter().copied() {
        let fields = || {
            PUBLISHED
                .iter()
                .filter(move |((published, _), _, _)| *published == kind)
        };
        let Some(schema) = rt.instance_kind_schema(kind) else {
            mismatches.push(Mismatch {
                message: format!("{KINDS_MODULE}: kind '{kind}' is not declared"),
                skip: fields().map(|(key, _, _)| *key).collect(),
            });
            continue;
        };
        for (key, ty, _) in fields() {
            let field = key.1;
            let message = match schema
                .host
                .iter()
                .find(|declared| declared.field.name == field)
            {
                None => format!(
                    "{kind}: the host publishes '{field}' ({ty}), which is not a :host field"
                ),
                Some(declared) if declared.field.ty.to_string() != *ty => format!(
                    "{kind}: '{field}' is declared {}, the host publishes {ty}",
                    declared.field.ty
                ),
                Some(_) => continue,
            };
            mismatches.push(Mismatch {
                message,
                skip: vec![*key],
            });
        }
        for declared in &schema.host {
            if !fields().any(|(key, _, _)| key.1 == declared.field.name) {
                mismatches.push(Mismatch {
                    message: format!(
                        "{kind}: :host field '{}' is declared but the host never publishes it",
                        declared.field.name
                    ),
                    skip: Vec::new(),
                });
            }
        }
    }
    mismatches
}

/// Check the loaded `eseq.kinds` against [`PUBLISHED`]: every published
/// field is a declared `:host` field of that type, and every declared
/// `:host` field is published. Returns one message per mismatch.
pub(crate) fn check_schema(rt: &Runtime) -> Result<(), Vec<String>> {
    let errors: Vec<String> = schema_mismatches(rt)
        .into_iter()
        .map(|mismatch| mismatch.message)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn schema_message(errors: &[String]) -> String {
    format!(
        "host kinds do not match {KINDS_MODULE} (content/core/modules/kinds.lisp):\n  {}",
        errors.join("\n  ")
    )
}

/// Run [`check_schema`] at startup: a hard error in debug builds, a
/// warning in release (spec §3.4). After startup the tick re-checks on
/// every schema change and only warns ([`HostKinds::sync`]).
pub(crate) fn check_schema_at_startup(rt: &Runtime) {
    if let Err(errors) = check_schema(rt) {
        let message = schema_message(&errors);
        if cfg!(debug_assertions) {
            panic!("{message}");
        }
        eprintln!("metal_seq: warning: {message}");
    }
}

/// Reserve the host kind names for `eseq.kinds` (spec §3.4).
pub(crate) fn reserve_kind_names(rt: &mut Runtime) {
    rt.reserve_kind_names(KINDS_MODULE, &host_kind_names());
}

// ── shared state ────────────────────────────────────────────────────────

/// What the reader hook and the tick share.
#[derive(Default)]
pub(crate) struct KindsShared {
    /// Live-field computations per field, for tests and profiling: an
    /// unobserved field never counts.
    pub(crate) computed: HashMap<FieldKey, u64>,
    /// Runs of the model half of the sync (gated by [`ModelRevision`]).
    pub(crate) model_syncs: u64,
    /// The last meter level of each track position (`t.peak`; the tick
    /// copies the meter cache, pruned to the track count).
    peaks: Vec<f64>,
    /// Who another track's (or a bus's) solo silences, as of the last
    /// sync (`t.audible`; the tick copies it from the `App`).
    solo: Option<app::SoloAudibility>,
    /// Fields a schema mismatch keeps the host from pushing.
    skip: HashSet<FieldKey>,
}

impl KindsShared {
    fn count(&mut self, key: FieldKey) {
        *self.computed.entry(key).or_default() += 1;
    }
}

/// What host kinds read outside the `App`: the shared live state and the UI
/// epochs. The event loop's [`SharedHandles`] holds all of it
/// ([`KindsHandles::of`]); headless capture, which has no `SharedHandles`,
/// builds one from its own handles. The reader hook keeps the first one it
/// is installed with.
#[derive(Clone)]
pub(crate) struct KindsHandles {
    pub(crate) state: Arc<SequencerState>,
    pub(crate) current_track: Arc<AtomicUsize>,
    pub(crate) selected_steps: Arc<Mutex<HashSet<usize>>>,
    pub(crate) active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
    pub(crate) active_delete_target_version: Arc<AtomicUsize>,
    pub(crate) record_armed: Arc<Mutex<Vec<bool>>>,
    pub(crate) recording: Arc<AtomicBool>,
    pub(crate) ui_epoch: Arc<AtomicUsize>,
    pub(crate) fx_epoch: Arc<AtomicUsize>,
    pub(crate) fx_value_epoch: Arc<AtomicUsize>,
}

impl KindsHandles {
    pub(crate) fn of(shared: &SharedHandles) -> Self {
        Self {
            state: shared.state.clone(),
            current_track: shared.current_track.clone(),
            selected_steps: shared.selected_steps.clone(),
            active_delete_target: shared.active_delete_target.clone(),
            active_delete_target_version: shared.active_delete_target_version.clone(),
            record_armed: shared.record_armed.clone(),
            recording: shared.recording.clone(),
            ui_epoch: shared.ui_epoch.clone(),
            fx_epoch: shared.fx_epoch.clone(),
            fx_value_epoch: shared.fx_value_epoch.clone(),
        }
    }

    fn track_exists(&self, track: usize) -> bool {
        track < self.state.active_track_count()
    }

    fn num_steps(&self, track: usize) -> usize {
        self.state.pattern.track_params[track]
            .get_num_steps()
            .min(MAX_STEPS)
    }

    fn playing_step(&self, track: usize) -> Option<usize> {
        self.state
            .transport
            .playing
            .load(Ordering::Relaxed)
            .then(|| track_active_playhead_step(&self.state, track))
    }

    /// Whether the step selection applies to `track`: the current track,
    /// or one of a rack-wide selection's tracks while its delete target is
    /// armed (like the legacy step-selection publish).
    fn selection_covers(&self, track: usize) -> bool {
        self.current_track.load(Ordering::Relaxed) == track
            || matches!(
                &*self.active_delete_target.lock().unwrap(),
                Some(ActiveDeleteTarget::TrackSteps { tracks }) if tracks.contains(&track)
            )
    }
}

/// Instance registry access shared by the reader hook (`&mut VM`) and the
/// tick (`&mut Runtime`).
trait KindStore {
    fn keyed(&self, kind: &str, key: &[u64]) -> Option<InstanceId>;
    fn key_of(&self, id: InstanceId) -> Option<&[u64]>;
    fn kind_of(&self, id: InstanceId) -> Option<&str>;
    fn register(&mut self, kind: &str, key: &[u64]) -> Option<InstanceId>;
    /// The step instances of `track` at or past `num_steps`.
    fn steps_past(&self, track: InstanceId, num_steps: usize) -> Vec<InstanceId>;
    fn drop_id(&mut self, id: InstanceId);
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value);
}

impl KindStore for VM {
    fn keyed(&self, kind: &str, key: &[u64]) -> Option<InstanceId> {
        self.keyed_instance(kind, key)
    }
    fn key_of(&self, id: InstanceId) -> Option<&[u64]> {
        self.instance_key(id)
    }
    fn kind_of(&self, id: InstanceId) -> Option<&str> {
        self.instance_kind(id)
    }
    fn register(&mut self, kind: &str, key: &[u64]) -> Option<InstanceId> {
        self.register_keyed_instance(kind, key).ok()
    }
    fn steps_past(&self, track: InstanceId, num_steps: usize) -> Vec<InstanceId> {
        self.keyed_children_of_kind(track, STEP)
            .filter(|(_, key)| matches!(key, [_, step] if *step as usize >= num_steps))
            .map(|(id, _)| id)
            .collect()
    }
    fn drop_id(&mut self, id: InstanceId) {
        self.drop_instance(id);
    }
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        report_push(self.set_instance_field(id, key.1, value), key);
    }
}

impl KindStore for Runtime {
    fn keyed(&self, kind: &str, key: &[u64]) -> Option<InstanceId> {
        self.keyed_instance(kind, key)
    }
    fn key_of(&self, id: InstanceId) -> Option<&[u64]> {
        self.instance_key(id)
    }
    fn kind_of(&self, id: InstanceId) -> Option<&str> {
        self.instance_kind(id)
    }
    fn register(&mut self, kind: &str, key: &[u64]) -> Option<InstanceId> {
        self.register_keyed_instance(kind, key).ok()
    }
    fn steps_past(&self, track: InstanceId, num_steps: usize) -> Vec<InstanceId> {
        self.keyed_children_of_kind(track, STEP)
            .filter(|(_, key)| matches!(key, [_, step] if *step as usize >= num_steps))
            .map(|(id, _)| id)
            .collect()
    }
    fn drop_id(&mut self, id: InstanceId) {
        self.drop_instance(id);
    }
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        report_push(self.set_instance_field(id, key.1, value), key);
    }
}

/// A rejected push is a host bug or a schema the check reported; never a
/// panic, since a hot reload can drift the schema mid-session.
fn report_push(result: Result<(), eseqlisp::vm::InstanceError>, (kind, field): FieldKey) {
    if let Err(error) = result {
        eprintln!("metal_seq: host kind push of {kind}.{field} failed: {error}");
    }
}

fn number(n: impl Into<f64>) -> Value {
    Value::Number(n.into())
}

fn instance_or_nil(id: Option<InstanceId>) -> Value {
    id.map_or(Value::Nil, Value::Instance)
}

fn instance_list(ids: impl IntoIterator<Item = InstanceId>) -> Value {
    list_value(ids.into_iter().map(Value::Instance))
}

fn rgb(color: sequencer::track_color::TrackColor) -> Value {
    eseqlisp::vm::tagged_list(
        "rgb",
        vec![number(color.r), number(color.g), number(color.b)],
    )
}

/// The step instances of a track holding `num_steps` steps, registering the
/// missing ones (with their fixed `index`/`track`) and dropping those past
/// the end (spec D2).
fn track_steps<S: KindStore>(store: &mut S, track: InstanceId, num_steps: usize) -> Value {
    drop_steps_past(store, track, num_steps);
    let steps = (0..num_steps as u64).filter_map(|step| {
        if let Some(id) = store.keyed(STEP, &[track, step]) {
            return Some(id);
        }
        let id = store.register(STEP, &[track, step])?;
        store.push(id, f::STEP_INDEX, number(step as f64));
        store.push(id, f::STEP_TRACK, Value::Instance(track));
        Some(id)
    });
    let steps: Vec<InstanceId> = steps.collect();
    instance_list(steps)
}

/// Drop the step instances of `track` at or past `num_steps`. Returns
/// whether any was dropped.
fn drop_steps_past<S: KindStore>(store: &mut S, track: InstanceId, num_steps: usize) -> bool {
    let doomed = store.steps_past(track, num_steps);
    for id in &doomed {
        store.drop_id(*id);
    }
    !doomed.is_empty()
}

/// A live field's current value (see [`Feed::Live`]); `None` for anything
/// else, and for a field a schema mismatch skips. Counts the computation.
fn live_value<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    field: &str,
) -> Option<Value> {
    let kind = store.kind_of(id)?;
    let key = *LIVE_KEYS
        .iter()
        .find(|(live_kind, live_field)| *live_kind == kind && *live_field == field)?;
    if shared.borrow().skip.contains(&key) {
        return None;
    }
    let value = match key.0 {
        TRACK => {
            let track = *store.key_of(id)?.first()? as usize;
            if !sources.track_exists(track) {
                return None;
            }
            let params = &sources.state.pattern.track_params[track];
            match key {
                f::TRACK_VOLUME => number(params.get_volume()),
                f::TRACK_MUTED => Value::Bool(params.is_muted()),
                // Effective mute, like the legacy `track-muted-effective`.
                f::TRACK_AUDIBLE => Value::Bool(
                    !params.is_muted()
                        && !shared
                            .borrow()
                            .solo
                            .as_ref()
                            .is_some_and(|solo| solo.track_is_muted(params)),
                ),
                f::TRACK_ARMED => Value::Bool(
                    sources
                        .record_armed
                        .lock()
                        .unwrap()
                        .get(track)
                        .copied()
                        .unwrap_or(false),
                ),
                f::TRACK_SELECTED => {
                    Value::Bool(sources.current_track.load(Ordering::Relaxed) == track)
                }
                f::TRACK_NUM_STEPS => number(sources.num_steps(track) as f64),
                f::TRACK_STEPS => {
                    let num_steps = sources.num_steps(track);
                    track_steps(store, id, num_steps)
                }
                f::TRACK_PEAK => number(shared.borrow().peaks.get(track).copied()?),
                _ => return None,
            }
        }
        STEP => {
            let &[parent, step] = store.key_of(id)? else {
                return None;
            };
            let track = *store.key_of(parent)?.first()? as usize;
            let step = step as usize;
            if !sources.track_exists(track) {
                return None;
            }
            match key {
                f::STEP_ACTIVE => {
                    Value::Bool(sources.state.pattern.patterns[track].is_active(step))
                }
                f::STEP_PLAYING => Value::Bool(sources.playing_step(track) == Some(step)),
                f::STEP_SELECTED => Value::Bool(
                    step < sources.num_steps(track)
                        && sources.selection_covers(track)
                        && sources.selected_steps.lock().unwrap().contains(&step),
                ),
                _ => return None,
            }
        }
        _ => match key {
            f::TRANSPORT_PLAYING => {
                Value::Bool(sources.state.transport.playing.load(Ordering::Relaxed))
            }
            f::TRANSPORT_RECORDING => Value::Bool(sources.recording.load(Ordering::Relaxed)),
            f::SELECTION_TRACK => {
                let track = sources.current_track.load(Ordering::Relaxed) as u64;
                instance_or_nil(store.keyed(TRACK, &[track]))
            }
            _ => return None,
        },
    };
    shared.borrow_mut().count(key);
    Some(value)
}

/// Install the reader hook that answers by-value reads of unobserved live
/// fields (and registers steps on a cold `t.steps`). Anything but a live
/// field name returns `None` before any lookup.
fn install_reader(rt: &mut Runtime, sources: Rc<KindsHandles>, shared: Rc<RefCell<KindsShared>>) {
    let reader: HostFieldReader = Rc::new(move |vm: &mut VM, id: InstanceId, field: &str| {
        if !LIVE_KEYS.iter().any(|(_, live)| *live == field) {
            return None;
        }
        live_value(vm, &sources, &shared, id, field)
    });
    rt.set_host_field_reader(Some(reader));
}

// ── the tick ────────────────────────────────────────────────────────────

/// The change counters the model fields derive from; the model half of a
/// sync runs only when this moves (or the track order does). Mirrors
/// `capture_param_sync_revision` (reactive_tick.rs).
#[derive(Clone, PartialEq)]
struct ModelRevision {
    ui_epoch: usize,
    fx_epoch: usize,
    fx_value_epoch: usize,
    pattern_epoch: u64,
    song_row_mirror_epoch: u64,
    sound_binding_epoch: usize,
    history_revision: u64,
    scenes_revision: u64,
    current_scene: usize,
    tracks: usize,
    active_tracks: usize,
    track_generation: u64,
    /// The display tint and palette track colors go through (a theme change
    /// bumps no epoch).
    track_tint: (
        eseqlisp::backend::Color,
        [eseqlisp::backend::Color; eseqlisp::theme::TRACK_PALETTE_SLOTS],
    ),
}

impl ModelRevision {
    fn capture(app: &app::App, shared: &KindsHandles) -> Self {
        Self {
            ui_epoch: shared.ui_epoch.load(Ordering::Relaxed),
            fx_epoch: shared.fx_epoch.load(Ordering::Relaxed),
            fx_value_epoch: shared.fx_value_epoch.load(Ordering::Relaxed),
            pattern_epoch: app.state.transport.pattern_epoch.load(Ordering::Relaxed),
            song_row_mirror_epoch: app.song_row_mirror_epoch,
            sound_binding_epoch: app.sound_binding_epoch,
            history_revision: app.history.current_revision(),
            scenes_revision: app.state.project_scenes_revision(),
            current_scene: app.state.current_scene_index(),
            tracks: app.tracks.len(),
            active_tracks: app.state.active_track_count(),
            track_generation: app.track_registry.generation(),
            track_tint: eseqlisp::theme::track_display_key(),
        }
    }
}

/// The step selection as of the last sync, so `step.selected` is
/// recomputed only when it (or the current track) changes.
#[derive(Default)]
struct StepSelection {
    primed: bool,
    current: usize,
    /// One flag per step index.
    steps: Vec<bool>,
    count: usize,
    delete_target_version: usize,
    /// A rack-wide selection's tracks (`ActiveDeleteTarget::TrackSteps`).
    rack_tracks: Vec<usize>,
}

impl StepSelection {
    /// Catch up with the shared selection; returns whether it changed.
    fn refresh(&mut self, sources: &KindsHandles) -> bool {
        let mut changed = !self.primed;
        self.primed = true;
        let current = sources.current_track.load(Ordering::Relaxed);
        if current != self.current {
            self.current = current;
            changed = true;
        }
        let version = sources.active_delete_target_version.load(Ordering::Relaxed);
        if version != self.delete_target_version || changed {
            self.delete_target_version = version;
            let target = sources.active_delete_target.lock().unwrap();
            let tracks = match &*target {
                Some(ActiveDeleteTarget::TrackSteps { tracks }) => tracks.as_slice(),
                _ => &[],
            };
            if tracks != self.rack_tracks.as_slice() {
                self.rack_tracks.clear();
                self.rack_tracks.extend_from_slice(tracks);
                changed = true;
            }
        }
        let set = sources.selected_steps.lock().unwrap();
        self.steps.resize(MAX_STEPS, false);
        let same = set.len() == self.count
            && set
                .iter()
                .all(|step| *step >= MAX_STEPS || self.steps[*step]);
        if !same {
            self.steps.fill(false);
            for step in set.iter().filter(|step| **step < MAX_STEPS) {
                self.steps[*step] = true;
            }
            self.count = set.len();
            changed = true;
        }
        changed
    }

    fn selected(&self, track: usize, step: usize) -> bool {
        (track == self.current || self.rack_tracks.contains(&track))
            && self.steps.get(step).copied().unwrap_or(false)
    }
}

/// Per-track step state the tick diffs against in place, so step pushes
/// cost work only where something changed.
#[derive(Default)]
struct StepDiff {
    /// The track's length at the last sync: steps are dropped only when it
    /// shrinks.
    num_steps: Option<usize>,
    /// The vectors and `playing` hold the values last pushed; when false
    /// every step is a candidate.
    primed: bool,
    active: Vec<bool>,
    selected: Vec<bool>,
    playing: Option<usize>,
    /// Whether any step instance of the track had an observed field, as of
    /// `Runtime::instance_observer_epoch`.
    observers: Option<(u64, bool)>,
}

/// The host side of `eseq.kinds`, kept across ticks (in
/// `FrameDiffState`).
#[derive(Default)]
pub(crate) struct HostKinds {
    pub(crate) shared: Rc<RefCell<KindsShared>>,
    /// Built when the reader is installed.
    sources: Option<Rc<KindsHandles>>,
    /// The schema generation [`check_schema`] last ran against, and the
    /// mismatches it warned about.
    schema_generation: Option<u64>,
    warned: Vec<String>,
    /// The model revision of the last model sync; `None` forces one.
    model: Option<ModelRevision>,
    /// The track registry's order at the last model sync.
    model_track_ids: Vec<sequencer::sequencer::TrackId>,
    /// `TrackId` → instance, for the registry generation in
    /// `track_generation`, so a reorder re-keys rather than re-registers
    /// (spec §4) and a project load replaces every track.
    tracks: HashMap<u64, InstanceId>,
    track_generation: Option<u64>,
    /// Scene / bank id → instance.
    scenes: HashMap<u64, InstanceId>,
    banks: HashMap<u64, InstanceId>,
    /// The instances at each position as of the last model sync.
    track_ids: Vec<Option<InstanceId>>,
    scene_ids: Vec<Option<InstanceId>>,
    bank_ids: Vec<Option<InstanceId>>,
    steps: HashMap<InstanceId, StepDiff>,
    selection: StepSelection,
    /// Per-step changed-field masks, reused across tracks and ticks.
    step_changes: Vec<u32>,
    /// The transport's queued scene and launch quantization last pushed.
    queued: Option<Option<usize>>,
    launch_quantize: Option<String>,
    /// Whether any track's `peak` was observed at the last sync.
    peaks_observed: bool,
}

/// Pushes during one sync: compares with the cell first.
struct Pusher<'a> {
    rt: &'a mut Runtime,
    sources: &'a KindsHandles,
    shared: &'a RefCell<KindsShared>,
    changed: bool,
}

impl Pusher<'_> {
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        {
            let shared = self.shared.borrow();
            if !shared.skip.is_empty() && shared.skip.contains(&key) {
                return;
            }
        }
        if self
            .rt
            .instance_field(id, key.1)
            .is_ok_and(|current| current == value)
        {
            return;
        }
        report_push(self.rt.set_instance_field(id, key.1, value), key);
        self.changed = true;
    }

    /// Compute and push one live field.
    fn push_live_field(&mut self, id: InstanceId, key: FieldKey) {
        if let Some(value) = live_value(&mut *self.rt, self.sources, self.shared, id, key.1) {
            self.push(id, key, value);
        }
    }

    /// The observed → compute → push pattern for one instance's live
    /// fields: one batched observed query, then the observed fields only.
    /// Returns the observed mask (bit `i` is `fields.keys[i]`).
    fn push_live(&mut self, id: InstanceId, fields: &LiveFields) -> u32 {
        let mask = self.rt.host_fields_observed(id, &fields.names);
        for (bit, key) in fields.keys.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                self.push_live_field(id, *key);
            }
        }
        mask
    }

    fn singleton(&self, kind: &str) -> Option<InstanceId> {
        self.rt.singleton_instance(kind)
    }
}

/// Bring the registry of index-keyed `kind` in line with `model` (stable
/// model ids in display order): drop instances whose thing is gone,
/// re-key moved ones in one step, register new ones. Returns the instance
/// at each index, aligned with `model` (`None` where registering failed).
fn reconcile(
    pusher: &mut Pusher<'_>,
    kind: &str,
    known: &mut HashMap<u64, InstanceId>,
    model: &[u64],
) -> Vec<Option<InstanceId>> {
    let rt = &mut *pusher.rt;
    known.retain(|_, id| rt.instance_is_live(*id));
    let wanted: HashSet<u64> = model.iter().copied().collect();
    let gone: Vec<u64> = known
        .keys()
        .copied()
        .filter(|model_id| !wanted.contains(model_id))
        .collect();
    for model_id in gone {
        if let Some(id) = known.remove(&model_id) {
            rt.drop_instance(id);
            pusher.changed = true;
        }
    }
    let moves: Vec<(InstanceId, Vec<u64>)> = model
        .iter()
        .enumerate()
        .filter_map(|(index, model_id)| {
            let id = *known.get(model_id)?;
            let key = [index as u64];
            (rt.instance_key(id) != Some(&key[..])).then(|| (id, key.to_vec()))
        })
        .collect();
    if !moves.is_empty() {
        if let Err(error) = rt.rekey_instances(&moves) {
            // Something else holds a target key: start those over.
            eprintln!("metal_seq: re-keying {kind} instances failed ({error}); re-registering");
            for (id, _) in &moves {
                rt.drop_instance(*id);
            }
            known.retain(|_, id| rt.instance_is_live(*id));
        }
        pusher.changed = true;
    }
    model
        .iter()
        .enumerate()
        .map(|(index, model_id)| {
            if let Some(id) = known.get(model_id) {
                return Some(*id);
            }
            match rt.register_keyed_instance(kind, &[index as u64]) {
                Ok(id) => {
                    known.insert(*model_id, id);
                    pusher.changed = true;
                    Some(id)
                }
                Err(error) => {
                    eprintln!("metal_seq: registering {kind} {index} failed: {error}");
                    None
                }
            }
        })
        .collect()
}

impl HostKinds {
    /// Whether any track's `peak` was observed at the last sync: the tick
    /// then keeps the track meter cache polled even with no legacy meter
    /// on screen.
    pub(crate) fn wants_peaks(&self) -> bool {
        self.peaks_observed
    }

    /// One sync: schema check (on change), registry and model fields (on a
    /// model revision change), queued scene and quantization, observed live
    /// fields. `track_peaks` is the meter cache, by track position.
    /// Returns whether anything changed (the reactive cycle has then run).
    pub(crate) fn sync(
        &mut self,
        app: &app::App,
        rt: &mut Runtime,
        shared: &SharedHandles,
        track_peaks: &[f64],
    ) -> bool {
        match &self.sources {
            Some(sources) => self.sync_sources(app, rt, sources.clone(), track_peaks),
            None => self.sync_with(app, rt, &KindsHandles::of(shared), track_peaks),
        }
    }

    /// [`Self::sync`] over explicit handles (headless capture).
    pub(crate) fn sync_with(
        &mut self,
        app: &app::App,
        rt: &mut Runtime,
        handles: &KindsHandles,
        track_peaks: &[f64],
    ) -> bool {
        let sources = match &self.sources {
            Some(sources) => sources.clone(),
            None => Rc::new(handles.clone()),
        };
        self.sync_sources(app, rt, sources, track_peaks)
    }

    fn sync_sources(
        &mut self,
        app: &app::App,
        rt: &mut Runtime,
        sources: Rc<KindsHandles>,
        track_peaks: &[f64],
    ) -> bool {
        if rt.instance_kind_schema(TRACK).is_none() {
            return false; // eseq.kinds is not loaded
        }
        if self.refresh_schema(rt) {
            self.model = None;
        }
        if self.sources.is_none() {
            install_reader(rt, sources.clone(), self.shared.clone());
            self.sources = Some(sources.clone());
            self.model = None;
        }
        {
            let mut kinds_shared = self.shared.borrow_mut();
            let count = app.tracks.len().min(track_peaks.len());
            if kinds_shared.peaks.as_slice() != &track_peaks[..count] {
                kinds_shared.peaks.clear();
                kinds_shared.peaks.extend_from_slice(&track_peaks[..count]);
            }
            kinds_shared.solo = Some(app.solo_audibility());
        }
        let shared_kinds = self.shared.clone();
        let mut pusher = Pusher {
            rt,
            sources: &sources,
            shared: &shared_kinds,
            changed: false,
        };
        let revision = ModelRevision::capture(app, &sources);
        let model_due = self.model.as_ref() != Some(&revision)
            || app.track_registry.ids() != self.model_track_ids.as_slice()
            || self.cached_instances_stale(pusher.rt);
        if model_due {
            self.launch_quantize = None;
            let tracks_done = self.sync_track_model(&mut pusher, app);
            let scenes_done = self.sync_scene_model(&mut pusher, app);
            self.shared.borrow_mut().model_syncs += 1;
            if tracks_done && scenes_done {
                self.model = Some(revision);
                self.model_track_ids.clear();
                self.model_track_ids
                    .extend_from_slice(app.track_registry.ids());
            } else {
                // Ids were unavailable this frame: try again next tick.
                self.model = None;
            }
        }
        self.sync_transport_queue(&mut pusher, app);
        let selection_changed = self.selection.refresh(&sources);
        self.sync_track_live(&mut pusher, selection_changed);
        for (kind, fields) in [(TRANSPORT, &*TRANSPORT_LIVE), (SELECTION, &*SELECTION_LIVE)] {
            if let Some(id) = pusher.singleton(kind) {
                pusher.push_live(id, fields);
            }
        }
        let changed = pusher.changed;
        if changed {
            rt.run_reactive_cycle();
        }
        changed
    }

    /// Re-run [`check_schema`] when a kind schema changed (a hot reload of
    /// `eseq.kinds`): warn once per distinct mismatch set and skip the
    /// mismatched fields until fixed. Returns whether the generation moved.
    fn refresh_schema(&mut self, rt: &Runtime) -> bool {
        let generation = rt.instance_kind_schema_generation();
        if self.schema_generation == Some(generation) {
            return false;
        }
        self.schema_generation = Some(generation);
        let mismatches = schema_mismatches(rt);
        let messages: Vec<String> = mismatches.iter().map(|m| m.message.clone()).collect();
        if !messages.is_empty() && messages != self.warned {
            eprintln!(
                "metal_seq: warning: {}\n  (skipping those fields until fixed)",
                schema_message(&messages)
            );
        }
        self.warned = messages;
        self.shared.borrow_mut().skip = mismatches
            .into_iter()
            .flat_map(|mismatch| mismatch.skip)
            .collect();
        true
    }

    /// Whether an instance the last model sync produced is gone (a hot
    /// reload dropped it): the model sync must run again.
    fn cached_instances_stale(&self, rt: &Runtime) -> bool {
        self.track_ids
            .iter()
            .chain(&self.scene_ids)
            .chain(&self.bank_ids)
            .flatten()
            .any(|id| !rt.instance_is_live(*id))
    }

    /// Track registry and model fields (index, name, color, preset,
    /// devices, `project.tracks`). Returns false when the registry was not
    /// in step with the track list this frame (nothing changed then).
    fn sync_track_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) -> bool {
        let registry = &app.track_registry;
        if registry.len() != app.tracks.len() {
            return false;
        }
        if self.track_generation != Some(registry.generation()) {
            // A new registry (project load or clear): its ids name other
            // tracks than the old one's.
            for (_, id) in self.tracks.drain() {
                pusher.rt.drop_instance(id);
                pusher.changed = true;
            }
            self.track_generation = Some(registry.generation());
        }
        let count = app.tracks.len().min(app.state.active_track_count());
        let model: Vec<u64> = registry.ids()[..count].iter().map(|id| id.0).collect();
        let tracks = reconcile(pusher, TRACK, &mut self.tracks, &model);
        self.steps.retain(|id, _| pusher.rt.instance_is_live(*id));
        let presets = track_loaded_presets(app, count);
        for (track, id) in tracks.iter().enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::TRACK_INDEX, number(track as f64));
            pusher.push(id, f::TRACK_NAME, Value::String(app.tracks[track].clone()));
            pusher.push(id, f::TRACK_COLOR, rgb(track_display_color(app, track)));
            pusher.push(id, f::TRACK_PRESET, Value::String(presets[track].clone()));
            let devices = sync_devices(pusher, app, track, id);
            pusher.push(id, f::TRACK_DEVICES, instance_list(devices));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(
                project,
                f::PROJECT_TRACKS,
                instance_list(tracks.iter().flatten().copied()),
            );
        }
        self.track_ids = tracks;
        true
    }

    /// Scenes, banks, their fields, and the transport's scene fields.
    /// Returns false when the scene or bank ids were not distinct this frame
    /// (identity never falls back to position; nothing changed then).
    fn sync_scene_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) -> bool {
        let (scene_rows, bank_rows, current) = app.state.with_project_scenes(|project| {
            let scenes: Vec<(u64, String)> = project
                .scenes
                .iter()
                .map(|scene| (scene.id.0, scene.name.clone()))
                .collect();
            let banks: Vec<(u64, Option<String>, usize)> = project
                .scene_banks()
                .iter()
                .map(|bank| (bank.id.0, bank.name.clone(), bank.len))
                .collect();
            (scenes, banks, project.current_scene)
        });
        let scene_model: Vec<u64> = scene_rows.iter().map(|(id, _)| *id).collect();
        let bank_model: Vec<u64> = bank_rows.iter().map(|(id, _, _)| *id).collect();
        if !distinct(&scene_model) || !distinct(&bank_model) {
            return false;
        }
        let queued = queued_transport_scene(&app.state);
        let scenes = reconcile(pusher, SCENE, &mut self.scenes, &scene_model);
        let banks = reconcile(pusher, BANK, &mut self.banks, &bank_model);
        // Each scene's bank and position in it (banks are consecutive spans).
        let mut holder: Vec<Option<(usize, usize)>> = vec![None; scenes.len()];
        let mut offset = 0;
        for (bank, (_, name, len)) in bank_rows.iter().enumerate() {
            let span = offset..(offset + len).min(scenes.len());
            offset += len;
            for scene in span.clone() {
                holder[scene] = Some((bank, scene - span.start));
            }
            let Some(&Some(id)) = banks.get(bank) else {
                continue;
            };
            pusher.push(id, f::BANK_INDEX, number(bank as f64));
            pusher.push(
                id,
                f::BANK_LABEL,
                Value::String(scene_bank_label(bank, name.as_deref())),
            );
            let members = span.clone().filter_map(|scene| scenes[scene]);
            pusher.push(id, f::BANK_SCENES, instance_list(members));
            pusher.push(id, f::BANK_PLAYING, Value::Bool(span.contains(&current)));
        }
        for (scene, id) in scenes.iter().enumerate() {
            let Some(id) = *id else { continue };
            let (bank, number_in_bank) = match holder[scene] {
                Some((bank, position)) => (banks.get(bank).copied().flatten(), position),
                None => (None, scene),
            };
            pusher.push(id, f::SCENE_INDEX, number(scene as f64));
            pusher.push(id, f::SCENE_NUMBER, number((number_in_bank + 1) as f64));
            pusher.push(
                id,
                f::SCENE_NAME,
                Value::String(scene_rows[scene].1.clone()),
            );
            pusher.push(id, f::SCENE_ACTIVE, Value::Bool(scene == current));
            pusher.push(id, f::SCENE_QUEUED, Value::Bool(queued == Some(scene)));
            pusher.push(id, f::SCENE_BANK, instance_or_nil(bank));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(
                project,
                f::PROJECT_SCENES,
                instance_list(scenes.iter().flatten().copied()),
            );
            pusher.push(
                project,
                f::PROJECT_BANKS,
                instance_list(banks.iter().flatten().copied()),
            );
        }
        if let Some(transport) = pusher.singleton(TRANSPORT) {
            let scene = scenes.get(current).copied().flatten();
            pusher.push(transport, f::TRANSPORT_SCENE, instance_or_nil(scene));
            let queued_id = queued.and_then(|scene| scenes.get(scene).copied().flatten());
            pusher.push(transport, f::TRANSPORT_QUEUED, instance_or_nil(queued_id));
        }
        self.scene_ids = scenes;
        self.bank_ids = banks;
        self.queued = Some(queued);
        true
    }

    /// The queued scene and launch quantization, compared every tick (a
    /// quantized launch arms and fires without moving any model counter).
    fn sync_transport_queue(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let queued = queued_transport_scene(&app.state);
        if self.queued != Some(queued) {
            let previous = self.queued.flatten();
            for scene in [previous, queued].into_iter().flatten() {
                if let Some(Some(id)) = self.scene_ids.get(scene) {
                    pusher.push(*id, f::SCENE_QUEUED, Value::Bool(queued == Some(scene)));
                }
            }
            if let Some(transport) = pusher.singleton(TRANSPORT) {
                let queued_id =
                    queued.and_then(|scene| self.scene_ids.get(scene).copied().flatten());
                pusher.push(transport, f::TRANSPORT_QUEUED, instance_or_nil(queued_id));
            }
            self.queued = Some(queued);
        }
        let Some(transport) = pusher.singleton(TRANSPORT) else {
            return;
        };
        let quantize = match pusher
            .rt
            .reactive_field_value("SEQ", "scene-launch-quantize")
        {
            Some(Value::String(label)) => label.as_str(),
            _ => "off",
        };
        if self.launch_quantize.as_deref() != Some(quantize) {
            let quantize = quantize.to_string();
            self.launch_quantize = Some(quantize.clone());
            pusher.push(
                transport,
                f::TRANSPORT_LAUNCH_QUANTIZE,
                Value::String(quantize),
            );
        }
    }

    /// The observed live fields of every track, then its steps.
    fn sync_track_live(&mut self, pusher: &mut Pusher<'_>, selection_changed: bool) {
        let peak_bit = TRACK_LIVE.bit(f::TRACK_PEAK);
        let steps_bit = TRACK_LIVE.bit(f::TRACK_STEPS);
        let mut peaks_observed = false;
        for (track, id) in self.track_ids.iter().enumerate() {
            let Some(id) = *id else { continue };
            if !pusher.sources.track_exists(track) {
                continue;
            }
            let observed = pusher.push_live(id, &TRACK_LIVE);
            peaks_observed |= observed & peak_bit != 0;
            let diff = self.steps.entry(id).or_default();
            sync_steps(
                pusher,
                diff,
                &self.selection,
                &mut self.step_changes,
                track,
                id,
                observed & steps_bit != 0,
                selection_changed,
            );
        }
        self.peaks_observed = peaks_observed;
    }
}

fn distinct(ids: &[u64]) -> bool {
    let unique: HashSet<u64> = ids.iter().copied().collect();
    unique.len() == ids.len()
}

/// Step fields of one track. Steps are dropped only when the track's
/// length shrank. Nothing more happens for a track with no step instances,
/// or while neither its `steps` nor any step field is observed (then the
/// diff starts over when something observes again). Otherwise `active`,
/// `selected` (only when the selection changed) and `playing` are diffed in
/// place, and only observed changed fields are pushed.
#[allow(clippy::too_many_arguments)]
fn sync_steps(
    pusher: &mut Pusher<'_>,
    diff: &mut StepDiff,
    selection: &StepSelection,
    changes: &mut Vec<u32>,
    track: usize,
    track_id: InstanceId,
    steps_observed: bool,
    selection_changed: bool,
) {
    let num_steps = pusher.sources.num_steps(track);
    if diff.num_steps.is_none_or(|last| num_steps < last)
        && drop_steps_past(&mut *pusher.rt, track_id, num_steps)
    {
        pusher.changed = true;
    }
    if diff.num_steps != Some(num_steps) {
        diff.num_steps = Some(num_steps);
        diff.primed = false;
    }
    if pusher
        .rt
        .keyed_children_of_kind(track_id, STEP)
        .next()
        .is_none()
    {
        diff.primed = false;
        return;
    }
    let epoch = pusher.rt.instance_observer_epoch();
    let observed = steps_observed
        || match diff.observers {
            Some((seen, observed)) if seen == epoch => observed,
            _ => {
                let observed = pusher
                    .rt
                    .keyed_children_observed(track_id, STEP, &STEP_LIVE.names);
                diff.observers = Some((epoch, observed));
                observed
            }
        };
    if !observed {
        diff.primed = false;
        return;
    }
    let (active_bit, playing_bit, selected_bit) = (
        STEP_LIVE.bit(f::STEP_ACTIVE),
        STEP_LIVE.bit(f::STEP_PLAYING),
        STEP_LIVE.bit(f::STEP_SELECTED),
    );
    let primed = diff.primed;
    changes.clear();
    changes.resize(num_steps, 0);
    diff.active.resize(num_steps, false);
    diff.selected.resize(num_steps, false);
    let pattern = &pusher.sources.state.pattern.patterns[track];
    for (step, previous) in diff.active.iter_mut().enumerate() {
        let active = pattern.is_active(step);
        if !primed || *previous != active {
            *previous = active;
            changes[step] |= active_bit;
        }
    }
    if !primed || selection_changed {
        for (step, previous) in diff.selected.iter_mut().enumerate() {
            let selected = selection.selected(track, step);
            if !primed || *previous != selected {
                *previous = selected;
                changes[step] |= selected_bit;
            }
        }
    }
    let playing = pusher.sources.playing_step(track);
    if !primed {
        changes.iter_mut().for_each(|change| *change |= playing_bit);
    } else if playing != diff.playing {
        for step in [diff.playing, playing].into_iter().flatten() {
            if let Some(change) = changes.get_mut(step) {
                *change |= playing_bit;
            }
        }
    }
    diff.playing = playing;
    diff.primed = true;
    for (step, change) in changes.iter().enumerate() {
        if *change == 0 {
            continue;
        }
        let Some(id) = pusher.rt.keyed_instance(STEP, &[track_id, step as u64]) else {
            continue;
        };
        let wanted = pusher.rt.host_fields_observed(id, &STEP_LIVE.names) & change;
        for (bit, key) in STEP_LIVE.keys.iter().enumerate() {
            if wanted & (1 << bit) != 0 {
                pusher.push_live_field(id, *key);
            }
        }
    }
}

/// Register, update and drop the device instances of one track; returns
/// them in chain order. Keyed (track id, slot + 1).
fn sync_devices(
    pusher: &mut Pusher<'_>,
    app: &app::App,
    track: usize,
    track_id: InstanceId,
) -> Vec<InstanceId> {
    let chain = track_device_chain(app, &app.state, track);
    let doomed: Vec<InstanceId> = pusher
        .rt
        .keyed_children_of_kind(track_id, DEVICE)
        .filter(|(_, key)| {
            matches!(key, [_, slot] if !chain.iter().any(|entry| (entry.slot + 1) as u64 == *slot))
        })
        .map(|(id, _)| id)
        .collect();
    for id in doomed {
        pusher.rt.drop_instance(id);
        pusher.changed = true;
    }
    chain
        .into_iter()
        .filter_map(|entry| {
            let key = [track_id, (entry.slot + 1) as u64];
            let id = match pusher.rt.keyed_instance(DEVICE, &key) {
                Some(id) => id,
                None => {
                    pusher.changed = true;
                    pusher.rt.register_keyed_instance(DEVICE, &key).ok()?
                }
            };
            pusher.push(id, f::DEVICE_TRACK, Value::Instance(track_id));
            pusher.push(id, f::DEVICE_SLOT, number(entry.slot as f64));
            pusher.push(id, f::DEVICE_NAME, Value::String(entry.name));
            pusher.push(id, f::DEVICE_ENABLED, Value::Bool(entry.enabled));
            Some(id)
        })
        .collect()
}

#[cfg(test)]
#[path = "host_kinds_tests.rs"]
mod tests;
