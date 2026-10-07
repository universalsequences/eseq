//! Generators (spec §14.2t, stage 7g-5): every tick-mode sequencer the
//! scheduler runs (a created kind's instance, such as `jaki`, or a script's
//! `def-sequencer` with a `:tick`) as a `generator`, the marks its ticks
//! stamp (`(gen-mark v [key])`) as `generator-mark`s; `project.generators`.
//!
//! Identity: generators are positional (`(index)`, in publish order), the
//! instance kept by sequencer id across reorders (`registry::reconcile`) and
//! replaced on a project load. A created instance's generator has the
//! instance's id as its `gid`, so `(generator-of self)` finds it; creating
//! or deleting the instance publishes or unpublishes its sequencer, which
//! registers or drops the generator and its marks (a held handle goes
//! stale). Marks are keyed (generator instance id, a stable key per mark
//! key string, never reused for another string until a project load
//! replaces every generator), registered when their key gets its first mark
//! and dropped with the generator's marks: unpublishing its sequencer drops
//! them (`SequencerState::drop_generator_marks`, so a redefinition starts
//! with none), as does a clear (a project load, an instance replacement).
//!
//! Feeds: the model fields behind one key ([`GeneratorKey`]: the published
//! sequencer version, the mark keys' revision and the group instances, which
//! an owner names), compared without allocating; the published generators
//! are re-read only when the published version moved, the mark keys only
//! when their revision did. Live (observed only, compared with the last
//! push, so an idle tick reads nothing): a mark's `value`
//! (`SequencerState::with_shown_generator_marks`), every observed mark
//! under one lock.

use super::*;

/// The inputs of the generator model sync, compared without allocating.
struct GeneratorKey {
    published: u64,
    marks: u64,
    groups: Vec<Option<InstanceId>>,
}

/// A published generator: what its fields derive from.
struct Generator {
    id: u64,
    name: String,
    owner_rack: Option<u64>,
}

/// What the generator half of the sync keeps across ticks.
#[derive(Default)]
pub(crate) struct GeneratorState {
    key: Option<GeneratorKey>,
    /// The published generators, with the published version they were
    /// read at.
    published: Option<(u64, Vec<Generator>)>,
    /// Sequencer id → generator instance (dropped on a project load).
    instances: HashMap<u64, InstanceId>,
    ids: Vec<Option<InstanceId>>,
    /// A mark's sub-key per mark key: stable until a project load (which
    /// replaces every generator), so a mark keeps its instance across syncs.
    mark_keys: HashMap<String, u64>,
    /// The observed marks, and their values as last pushed (the scratch
    /// buffer holds this tick's reads).
    mark_observed: ObservedList,
    pushed: HashMap<InstanceId, f64>,
    shown: Vec<f64>,
    /// Generator model syncs (tests: an idle tick syncs none).
    pub(crate) syncs: u64,
}

impl GeneratorState {
    /// One generator instance (the stale check's representative).
    pub(super) fn representative(&self) -> Option<&InstanceId> {
        self.ids.iter().flatten().next()
    }

    /// Sync every generator at the next tick (a schema change or a hot
    /// reload dropped instances); the registered instances are kept.
    pub(super) fn invalidate(&mut self) {
        self.key = None;
        self.pushed.clear();
        self.mark_observed.reset();
    }

    /// The generator instances, for a project load (ids restart with it).
    pub(super) fn drain(&mut self) -> impl Iterator<Item = (u64, InstanceId)> + '_ {
        self.key = None;
        self.mark_keys.clear();
        self.pushed.clear();
        self.instances.drain()
    }

    /// The sub-key of the mark key `key` (see `mark_keys`).
    fn mark_key(&mut self, key: &str) -> u64 {
        if let Some(sub) = self.mark_keys.get(key) {
            return *sub;
        }
        let sub = self.mark_keys.len() as u64;
        self.mark_keys.insert(key.to_string(), sub);
        sub
    }
}

/// A mark's `value` (the reader hook's cold read; the tick reads every
/// observed mark under one lock instead).
pub(super) fn generator_mark_value(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    match key {
        f::GENERATOR_MARK_VALUE => {
            let shared = shared.borrow();
            let slot = shared.generator_marks.get(&id)?;
            let shown = sources
                .state
                .with_shown_generator_marks(|shown| shown(slot));
            Some(number(shown))
        }
        key => unreachable!("{key:?} is not a generator mark live field"),
    }
}

impl HostKinds {
    /// The generators, their marks and `project.generators`, when
    /// [`GeneratorKey`] moved. Runs after the model sync (groups).
    pub(super) fn sync_generator_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let read = (
            app.state.published_sequencers_version(),
            app.state.generator_mark_keys_revision(),
        );
        let (groups, state) = (&self.group_ids, &mut self.generators);
        let key = state.key.as_ref();
        if key.is_some_and(|key| (key.published, key.marks) == read && key.groups == *groups) {
            return;
        }
        let published = match state.published.take() {
            Some((version, published)) if version == read.0 => published,
            _ => (app.state.published_sequencers().into_iter())
                .filter(|published| published.graph.is_none())
                .map(|published| Generator {
                    id: published.id,
                    name: published.name,
                    owner_rack: published.owner_rack,
                })
                .collect(),
        };
        let model: Vec<u64> = published.iter().map(|generator| generator.id).collect();
        if !distinct(&model) {
            state.published = Some((read.0, published));
            return; // retried next tick
        }
        let ids = reconcile(pusher, GENERATOR, &mut state.instances, &model);
        let mut mark_keys = app.state.generator_mark_keys();
        mark_keys.sort_unstable();
        let mut slots = HashMap::with_capacity(mark_keys.len());
        for (index, (generator, id)) in published.iter().zip(&ids).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::GENERATOR_INDEX, number(index as f64));
            pusher.push(id, f::GENERATOR_GID, number(generator.id as f64));
            pusher.push(id, f::GENERATOR_NAME, text(&generator.name));
            let owner = (generator.owner_rack).and_then(|gid| self.groups.get(&gid).copied());
            pusher.push(id, f::GENERATOR_OWNER, instance_or_nil(owner));
            let keys: Vec<&str> = (mark_keys.iter())
                .filter(|(gid, _)| *gid == generator.id)
                .map(|(_, key)| key.as_str())
                .collect();
            let subs: Vec<u64> = keys.iter().map(|key| state.mark_key(key)).collect();
            let marks = pusher.reconcile_children(id, GENERATOR_MARK, &subs);
            for (key, mark) in keys.iter().zip(&marks) {
                let Some(mark) = *mark else { continue };
                pusher.push(mark, f::GENERATOR_MARK_GENERATOR, Value::Instance(id));
                pusher.push(mark, f::GENERATOR_MARK_NAME, text(key));
                slots.insert(mark, (generator.id, key.to_string()));
            }
            pusher.push(id, f::GENERATOR_MARKS, listed_instances(&marks));
        }
        if let Some(project) = pusher.singleton(PROJECT) {
            pusher.push(project, f::PROJECT_GENERATORS, listed_instances(&ids));
        }
        let mut shared = pusher.shared.borrow_mut();
        let known = &shared.generator_marks;
        if known.len() != slots.len() || slots.keys().any(|mark| !known.contains_key(mark)) {
            state.pushed.retain(|mark, _| slots.contains_key(mark));
            state.mark_observed.reset();
        }
        shared.generator_marks = slots;
        state.published = Some((read.0, published));
        state.ids = ids;
        state.syncs += 1;
        state.key = Some(GeneratorKey {
            published: read.0,
            marks: read.1,
            groups: groups.clone(),
        });
    }

    /// The observed marks' `value`s: read under one lock, pushed where they
    /// moved since the last push.
    pub(super) fn sync_generator_live(&mut self, pusher: &mut Pusher<'_>) {
        let state = &mut self.generators;
        let names = &GENERATOR_MARK_LIVE.names;
        let shared = pusher.shared.borrow();
        let slots = &shared.generator_marks;
        (state.mark_observed).refresh(pusher.rt, names, || slots.keys().copied().collect());
        let entries = &state.mark_observed.entries;
        state
            .pushed
            .retain(|id, _| entries.iter().any(|(observed, _, _)| observed == id));
        if entries.is_empty() {
            return;
        }
        let shown = &mut state.shown;
        shown.clear();
        pusher.sources.state.with_shown_generator_marks(|value| {
            let slot = |id| slots.get(id).map_or(0.0, value);
            shown.extend(entries.iter().map(|(id, _, _)| slot(id)));
        });
        drop(shared);
        for ((id, _, _), value) in entries.iter().zip(state.shown.iter().copied()) {
            let changed = state.pushed.insert(*id, value) != Some(value);
            pusher.push_computed_if(*id, f::GENERATOR_MARK_VALUE, changed, || number(value));
        }
    }
}
