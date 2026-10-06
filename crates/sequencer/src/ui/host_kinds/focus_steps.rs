//! The piano roll's focus steps (spec §14, stage 7e-2): `piano-roll.steps`,
//! the steps of the piano roll's source on its axis (a pinned pattern's or
//! take's too, which `step`, the live pattern's, does not reach): whether
//! each holds a note, where its notes sound, and its step parameters. The
//! automation lane under the piano roll is a view over them, the notes and
//! the device params' `step-locks` / `has-locks` (legacy
//! `SEQ.piano-roll-automation`, `-automation-params`).
//!
//! **Identity.** Positional (spec D2): keyed (track instance id, index)
//! under the piano roll's track, registered for that track alone (the
//! previous track's are dropped when the piano roll moves) and dropped past
//! the source's length. Another source of the same track (a clip pinned, a
//! scene launched) keeps the instances; only their values move, as a `step`
//! under a scene switch.
//!
//! **Feeds.** Model fields, registered on the first read of
//! `piano-roll.steps` (the reader hook, or the tick once observed), like
//! the notes; then re-read when the notes' [`ContentKey`] moved (a setter's
//! edit moves it: `App::focus_step_edits`), from the batch read the notes
//! share (`source_rows`), each field compared with its cell: an idle tick
//! loads a few counters. `project.focus-step-params` describes the step
//! params, pushed once.

use super::*;

/// The step params a focus step publishes, by field, in the legacy lane
/// picker's order (`StepParam::VISIBLE`): each visible param's focus-step
/// field in [`PUBLISHED`], by its `step_param_named` name.
pub(crate) static FOCUS_STEP_PARAMS: LazyLock<Vec<(FieldKey, StepParam)>> = LazyLock::new(|| {
    let fields = PUBLISHED.iter().map(|(key, _, _)| *key);
    let fields: Vec<FieldKey> = fields.filter(|(kind, _)| *kind == FOCUS_STEP).collect();
    (StepParam::VISIBLE.iter())
        .map(|param| {
            let field = (fields.iter())
                .find(|(_, name)| step_param_named(name) == Some(*param))
                .unwrap_or_else(|| panic!("{param:?} has no focus-step field"));
            (*field, *param)
        })
        .collect()
});

/// The step param a focus step's field `name` holds (exactly its field
/// name; no other spelling).
pub(crate) fn focus_step_param(name: &str) -> Option<StepParam> {
    (FOCUS_STEP_PARAMS.iter())
        .find(|((_, field), _)| *field == name)
        .map(|(_, param)| *param)
}

/// `project.focus-step-params`: per [`FOCUS_STEP_PARAMS`] entry, its field
/// (`:name`), `:label`, `:min`, `:max`, `:default` and `:increment`.
pub(super) fn focus_step_params_value() -> Value {
    list_value(FOCUS_STEP_PARAMS.iter().map(|((_, name), param)| {
        map_value([
            ("name", text(name)),
            ("label", text(param.label())),
            ("min", number(param.min())),
            ("max", number(param.max())),
            ("default", number(param.default_value())),
            ("increment", number(param.increment())),
        ])
    }))
}

/// The focus steps' share of [`KindsShared`] (the reader hook registers
/// them).
#[derive(Default)]
pub(crate) struct FocusStepShared {
    /// `piano-roll.steps` was read or observed: the steps are registered
    /// and kept current.
    registered: bool,
    /// The tick has pushed `piano-roll.steps` since they were registered.
    pushed: bool,
    /// The track instance they are registered under, and the listed steps.
    track: Option<InstanceId>,
    ids: Vec<InstanceId>,
    /// Re-reads, for tests.
    pub(crate) syncs: u64,
}

impl FocusStepShared {
    pub(super) fn registered(&self) -> bool {
        self.registered
    }

    pub(super) fn register(&mut self) {
        self.registered = true;
    }

    /// Start over when a hot reload dropped the step instances; returns
    /// whether it did.
    pub(super) fn drop_if_stale(&mut self, rt: &Runtime) -> bool {
        if (self.ids.first()).is_none_or(|id| rt.instance_is_live(*id)) {
            return false;
        }
        self.ids.clear();
        self.track = None;
        self.pushed = false;
        true
    }
}

/// Bring the focus step instances in line with `source`'s `rows`
/// ([`source_rows`]): drop another track's, keep `0..rows.len()` under its
/// track and push every field (each compared with its cell). Returns the
/// steps and whether anything changed.
pub(super) fn sync_focus_steps<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    source: NoteSource,
    rows: &[StepRow],
) -> (Vec<InstanceId>, bool) {
    let track = source.track_id;
    let previous = shared.borrow().focus_steps.track;
    let mut changed = false;
    if let Some(old) = previous.filter(|old| *old != track) {
        changed |= drop_children_past(store, old, FOCUS_STEP, 0);
    }
    let ids = indexed_children(store, track, FOCUS_STEP, rows.len(), |store, id, index| {
        put(
            store,
            shared,
            id,
            f::FOCUS_STEP_TRACK,
            Value::Instance(track),
        );
        put(store, shared, id, f::FOCUS_STEP_INDEX, number(index as f64));
        changed = true;
    });
    for (step, (id, (notes, values))) in ids.iter().zip(rows).enumerate() {
        let (start, end) = piano_roll_step_span(step, notes);
        let fields = [
            (f::FOCUS_STEP_ACTIVE, Value::Bool(!notes.is_empty())),
            (f::FOCUS_STEP_START, number(start)),
            (f::FOCUS_STEP_END, number(end)),
        ];
        let params = (FOCUS_STEP_PARAMS.iter())
            .map(|(field, param)| (*field, number(values[param.index()])));
        for (field, value) in fields.into_iter().chain(params) {
            changed |= put(store, shared, *id, field, value);
        }
    }
    let steps = &mut shared.borrow_mut().focus_steps;
    steps.track = Some(track);
    steps.ids.clone_from(&ids);
    steps.syncs += 1;
    (ids, changed)
}

/// A cold read of `piano-roll.steps` (the reader hook): registers the steps
/// the first time; afterwards the model field answers (`None`) once the tick
/// pushed it.
pub(super) fn cold_piano_roll_steps<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
) -> Option<Value> {
    let source = {
        let shared = shared.borrow();
        if shared.skip.contains(&f::PIANO_ROLL_STEPS) {
            return None;
        }
        let steps = &shared.focus_steps;
        if steps.registered {
            if steps.pushed {
                return None;
            }
            return Some(instance_list(steps.ids.iter().copied()));
        }
        shared.notes.focus?
    };
    shared.borrow_mut().focus_steps.registered = true;
    let rows = source_rows(sources, source);
    let (listed, _) = sync_focus_steps(store, shared, source, &rows);
    shared.borrow_mut().count(f::PIANO_ROLL_STEPS);
    Some(instance_list(listed))
}

impl HostKinds {
    /// Whether the registered focus steps are to be re-read: their
    /// [`ContentKey`] moved or they were never pushed.
    pub(super) fn steps_due(&self, shared: &RefCell<KindsShared>, key: ContentKey) -> bool {
        self.piano_roll.steps != Some(key) || !shared.borrow().focus_steps.pushed
    }

    /// Re-read the registered focus steps from `rows` (the source's steps).
    pub(super) fn sync_piano_roll_steps(
        &mut self,
        pusher: &mut Pusher<'_>,
        roll: InstanceId,
        source: NoteSource,
        key: ContentKey,
        rows: &[StepRow],
    ) {
        let shared = pusher.shared;
        let (listed, changed) = sync_focus_steps(&mut *pusher.rt, shared, source, rows);
        pusher.changed |= changed;
        pusher.push(roll, f::PIANO_ROLL_STEPS, instance_list(listed));
        shared.borrow_mut().focus_steps.pushed = true;
        self.piano_roll.steps = Some(key);
    }
}
