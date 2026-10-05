//! Instance registry helpers: the store both the reader hook and the tick
//! register through, the comparing [`Pusher`], and reconciling keyed
//! instances with the model.

use super::*;

/// Instance registry access shared by the reader hook (`&mut VM`) and the
/// tick (`&mut Runtime`).
pub(super) trait KindStore {
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

/// The step instances of a track holding `num_steps` steps, registering the
/// missing ones (with their fixed `index`/`track`) and dropping those past
/// the end (spec D2).
pub(super) fn track_steps<S: KindStore>(
    store: &mut S,
    track: InstanceId,
    num_steps: usize,
) -> Value {
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
pub(super) fn drop_steps_past<S: KindStore>(
    store: &mut S,
    track: InstanceId,
    num_steps: usize,
) -> bool {
    let doomed = store.steps_past(track, num_steps);
    for id in &doomed {
        store.drop_id(*id);
    }
    !doomed.is_empty()
}

/// Pushes during one sync: compares with the cell first.
pub(super) struct Pusher<'a> {
    pub(super) rt: &'a mut Runtime,
    pub(super) sources: &'a KindsHandles,
    pub(super) shared: &'a RefCell<KindsShared>,
    pub(super) changed: bool,
}

impl Pusher<'_> {
    pub(super) fn push(&mut self, id: InstanceId, key: FieldKey, value: Value) {
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
    pub(super) fn push_live_field(&mut self, id: InstanceId, key: FieldKey) {
        if let Some(value) = live_value(&mut *self.rt, self.sources, self.shared, id, key.1) {
            self.push(id, key, value);
        }
    }

    /// The observed → compute → push pattern for one instance's live
    /// fields: one batched observed query, then the observed fields only.
    /// Returns the observed mask (bit `i` is `fields.keys[i]`).
    pub(super) fn push_live(&mut self, id: InstanceId, fields: &LiveFields) -> u32 {
        let mask = self.rt.host_fields_observed(id, &fields.names);
        for (bit, key) in fields.keys.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                self.push_live_field(id, *key);
            }
        }
        mask
    }

    /// [`Self::push_live`] for each of `ids` while `cache` (see
    /// [`observed_union`]) says any observes a field; returns the union of
    /// their observed masks and caches it, exact as of now.
    pub(super) fn push_live_all(
        &mut self,
        ids: impl Iterator<Item = InstanceId> + Clone,
        fields: &LiveFields,
        cache: &mut Option<(u64, u32)>,
    ) -> u32 {
        if observed_union(self.rt, cache, ids.clone(), fields) == 0 {
            return 0;
        }
        let union = ids.fold(0, |union, id| union | self.push_live(id, fields));
        *cache = Some((self.rt.instance_observer_epoch(), union));
        union
    }

    pub(super) fn singleton(&self, kind: &str) -> Option<InstanceId> {
        self.rt.singleton_instance(kind)
    }
}

/// The union of `ids`' observed `fields` (bit `i` is `fields.keys[i]`),
/// cached in `cache` per `Runtime::instance_observer_epoch`. Gaining an
/// observer moves the epoch and losing one does not, so a cached union may
/// over-approximate but never misses a new observer.
pub(super) fn observed_union(
    rt: &Runtime,
    cache: &mut Option<(u64, u32)>,
    ids: impl Iterator<Item = InstanceId>,
    fields: &LiveFields,
) -> u32 {
    let epoch = rt.instance_observer_epoch();
    if let Some((seen, union)) = *cache {
        if seen == epoch {
            return union;
        }
    }
    let union = ids.fold(0, |union, id| {
        union | rt.host_fields_observed(id, &fields.names)
    });
    *cache = Some((epoch, union));
    union
}

/// Bring the registry of index-keyed `kind` in line with `model` (stable
/// model ids in display order): drop instances whose thing is gone,
/// re-key moved ones in one step, register new ones. Returns the instance
/// at each index, aligned with `model` (`None` where registering failed).
pub(super) fn reconcile(
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

/// Bring the `kind` children of `parent`, keyed (parent, sub-key), in line
/// with `wanted` sub-keys: drop the others, register the missing ones.
/// Returns the instance of each wanted sub-key, aligned with `wanted`
/// (`None` where registering failed).
pub(super) fn reconcile_children(
    pusher: &mut Pusher<'_>,
    parent: InstanceId,
    kind: &str,
    wanted: &[u64],
) -> Vec<Option<InstanceId>> {
    let doomed: Vec<InstanceId> = pusher
        .rt
        .keyed_children_of_kind(parent, kind)
        .filter(|(_, key)| matches!(key, [_, sub] if !wanted.contains(sub)))
        .map(|(id, _)| id)
        .collect();
    for id in doomed {
        pusher.rt.drop_instance(id);
        pusher.changed = true;
    }
    wanted
        .iter()
        .map(|sub| {
            let key = [parent, *sub];
            if let Some(id) = pusher.rt.keyed_instance(kind, &key) {
                return Some(id);
            }
            pusher.changed = true;
            pusher.rt.register_keyed_instance(kind, &key).ok()
        })
        .collect()
}

pub(super) fn distinct(ids: &[u64]) -> bool {
    let unique: HashSet<u64> = ids.iter().copied().collect();
    unique.len() == ids.len()
}
