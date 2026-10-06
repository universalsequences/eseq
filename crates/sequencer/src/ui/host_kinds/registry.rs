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
    /// The `kind` children of `parent` whose sub-key (second key part)
    /// `pick` accepts.
    fn children_where(
        &self,
        parent: InstanceId,
        kind: &str,
        pick: &dyn Fn(u64) -> bool,
    ) -> Vec<InstanceId>;
    /// The `kind` children of `parent` whose index (second key part) is at
    /// or past `count`: steps past a track's length, params past a device's.
    fn children_past(&self, parent: InstanceId, kind: &str, count: usize) -> Vec<InstanceId> {
        self.children_where(parent, kind, &|index| index as usize >= count)
    }
    fn drop_id(&mut self, id: InstanceId);
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value);
    /// A field's value in its cell (without asking the reader hook).
    fn field(&self, id: InstanceId, name: &str) -> Option<Value>;
    /// A Lisp global's value (the step cursor), read without evaluating.
    fn global(&self, name: &str) -> Option<Value>;
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
    fn children_where(
        &self,
        parent: InstanceId,
        kind: &str,
        pick: &dyn Fn(u64) -> bool,
    ) -> Vec<InstanceId> {
        self.keyed_children_of_kind(parent, kind)
            .filter(|(_, key)| matches!(key, [_, sub] if pick(*sub)))
            .map(|(id, _)| id)
            .collect()
    }
    fn drop_id(&mut self, id: InstanceId) {
        self.drop_instance(id);
    }
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        report_push(self.set_instance_field(id, key.1, value), key);
    }
    fn field(&self, id: InstanceId, name: &str) -> Option<Value> {
        self.instance_field(id, name).ok()
    }
    fn global(&self, name: &str) -> Option<Value> {
        self.global_value(name)
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
    fn children_where(
        &self,
        parent: InstanceId,
        kind: &str,
        pick: &dyn Fn(u64) -> bool,
    ) -> Vec<InstanceId> {
        self.keyed_children_of_kind(parent, kind)
            .filter(|(_, key)| matches!(key, [_, sub] if pick(*sub)))
            .map(|(id, _)| id)
            .collect()
    }
    fn drop_id(&mut self, id: InstanceId) {
        self.drop_instance(id);
    }
    fn push(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        report_push(self.set_instance_field(id, key.1, value), key);
    }
    fn field(&self, id: InstanceId, name: &str) -> Option<Value> {
        self.instance_field(id, name).ok()
    }
    fn global(&self, name: &str) -> Option<Value> {
        self.global_value(name)
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
    let steps = indexed_children(store, track, STEP, num_steps, |store, id, step| {
        store.push(id, f::STEP_INDEX, number(step as f64));
        store.push(id, f::STEP_TRACK, Value::Instance(track));
    });
    instance_list(steps)
}

/// The `kind` children of `parent` keyed (parent, index) for indexes
/// `0..count`, in order: drops those at or past `count`, registers the
/// missing ones and runs `init` on each new one (its fixed fields).
pub(super) fn indexed_children<S: KindStore>(
    store: &mut S,
    parent: InstanceId,
    kind: &str,
    count: usize,
    mut init: impl FnMut(&mut S, InstanceId, usize),
) -> Vec<InstanceId> {
    drop_children_past(store, parent, kind, count);
    (0..count)
        .filter_map(|index| {
            let key = [parent, index as u64];
            if let Some(id) = store.keyed(kind, &key) {
                return Some(id);
            }
            let id = store.register(kind, &key)?;
            init(store, id, index);
            Some(id)
        })
        .collect()
}

/// Drop the `kind` children of `parent` whose index is at or past `count`
/// (steps past a track's length, params past a device's). Returns whether
/// any was dropped.
pub(super) fn drop_children_past<S: KindStore>(
    store: &mut S,
    parent: InstanceId,
    kind: &str,
    count: usize,
) -> bool {
    let doomed = store.children_past(parent, kind, count);
    for id in &doomed {
        store.drop_id(*id);
    }
    !doomed.is_empty()
}

/// Push `value` into `id`'s `key` unless the cell holds it (or a schema
/// mismatch skips it); returns whether it was pushed.
pub(super) fn put<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
    value: Value,
) -> bool {
    {
        let shared = shared.borrow();
        if !shared.skip.is_empty() && shared.skip.contains(&key) {
            return false;
        }
    }
    if store
        .field(id, key.1)
        .is_some_and(|current| current == value)
    {
        return false;
    }
    store.push(id, key, value);
    true
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
        self.changed |= put(&mut *self.rt, self.shared, id, key, value);
    }

    /// Push a live field value computed outside [`live_value`], counting it
    /// like one.
    pub(super) fn push_computed(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        if self.shared.borrow().skip.contains(&key) {
            return;
        }
        self.shared.borrow_mut().count(key);
        self.push(id, key, value);
    }

    /// Push a text field computed outside [`live_value`], counting it like
    /// one; allocates only when the text differs from the cell.
    pub(super) fn push_text(&mut self, id: InstanceId, key: FieldKey, text: &str) {
        if self.shared.borrow().skip.contains(&key) {
            return;
        }
        self.shared.borrow_mut().count(key);
        let same = self
            .rt
            .instance_field(id, key.1)
            .is_ok_and(|current| matches!(current, Value::String(current) if current == text));
        if !same {
            self.push(id, key, Value::String(text.to_string()));
        }
    }

    /// Push a live field value computed outside [`live_value`] when
    /// `changed` says it moved, counting the computation; `value` builds it.
    pub(super) fn push_computed_if(
        &mut self,
        id: InstanceId,
        key: FieldKey,
        changed: bool,
        value: impl FnOnce() -> Value,
    ) {
        if self.shared.borrow().skip.contains(&key) {
            return;
        }
        self.shared.borrow_mut().count(key);
        if changed {
            self.push(id, key, value());
        }
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
        self.push_live_except(id, fields, 0)
    }

    /// [`Self::push_live`], leaving the fields in `except` (bits as in the
    /// returned mask) to the caller.
    pub(super) fn push_live_except(
        &mut self,
        id: InstanceId,
        fields: &LiveFields,
        except: u32,
    ) -> u32 {
        let mask = self.rt.host_fields_observed(id, &fields.names);
        self.push_live_masked(id, fields, mask & !except);
        mask
    }

    /// Compute and push the live fields of `id` in `mask` (bit `i` is
    /// `fields.keys[i]`).
    pub(super) fn push_live_masked(&mut self, id: InstanceId, fields: &LiveFields, mask: u32) {
        for (bit, key) in fields.keys.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                self.push_live_field(id, *key);
            }
        }
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
    if let Some(union) = cached_union(rt, *cache) {
        return union;
    }
    let epoch = rt.instance_observer_epoch();
    let union = ids.fold(0, |union, id| {
        union | rt.host_fields_observed(id, &fields.names)
    });
    *cache = Some((epoch, union));
    union
}

/// The union a cache (see [`observed_union`]) holds, while it is of the
/// current observer epoch.
pub(super) fn cached_union(rt: &Runtime, cache: Option<(u64, u32)>) -> Option<u32> {
    let (seen, union) = cache?;
    (seen == rt.instance_observer_epoch()).then_some(union)
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
/// with `wanted` sub-keys: drop the others, register the missing ones and
/// run `init` on each new one (its fixed fields). Returns the instance of
/// each wanted sub-key, aligned with `wanted` (`None` where registering
/// failed), and whether any instance was dropped or registered.
pub(super) fn reconcile_children<S: KindStore>(
    store: &mut S,
    parent: InstanceId,
    kind: &str,
    wanted: &[u64],
    mut init: impl FnMut(&mut S, InstanceId, u64),
) -> (Vec<Option<InstanceId>>, bool) {
    let doomed = store.children_where(parent, kind, &|sub| !wanted.contains(&sub));
    let mut changed = !doomed.is_empty();
    for id in doomed {
        store.drop_id(id);
    }
    let ids = wanted
        .iter()
        .map(|sub| {
            let key = [parent, *sub];
            if let Some(id) = store.keyed(kind, &key) {
                return Some(id);
            }
            changed = true;
            let id = store.register(kind, &key)?;
            init(store, id, *sub);
            Some(id)
        })
        .collect();
    (ids, changed)
}

impl Pusher<'_> {
    /// [`reconcile_children`] with no fixed fields, through the pusher.
    pub(super) fn reconcile_children(
        &mut self,
        parent: InstanceId,
        kind: &str,
        wanted: &[u64],
    ) -> Vec<Option<InstanceId>> {
        let (ids, changed) = reconcile_children(&mut *self.rt, parent, kind, wanted, |_, _, _| {});
        self.changed |= changed;
        ids
    }
}

/// Every device instance: the track chains' and the device sync's.
pub(super) fn all_device_ids<'a>(
    chain: &'a [InstanceId],
    devices: &'a DeviceState,
) -> impl Iterator<Item = InstanceId> + 'a {
    chain.iter().copied().chain(devices.ids())
}

/// The `kind` children of every one of `owners`.
pub(super) fn children_of_owners(
    rt: &Runtime,
    owners: impl IntoIterator<Item = InstanceId>,
    kind: &str,
) -> Vec<InstanceId> {
    let children = owners
        .into_iter()
        .flat_map(|owner| rt.keyed_children_of_kind(owner, kind));
    children.map(|(id, _)| id).collect()
}

pub(super) fn distinct(ids: &[u64]) -> bool {
    let unique: HashSet<u64> = ids.iter().copied().collect();
    unique.len() == ids.len()
}
