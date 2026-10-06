//! P-lock variants (spec §14.2g, stage 7b-3): a track's step variants
//! (`t.variants`, the step panel's variant chips, legacy
//! `SEQ.track-plock-variants`) and a track instrument's key-lock variants
//! (`d.variants`, the keys tab's, legacy `:key-lock-variants` /
//! `:key-lock-note-variants`).
//!
//! A variant is keyed (owner instance id, `vid`): the track for a step
//! variant, the instrument device for a key-lock variant, and its label's
//! place in the A, B, …, A', … order (`plock_variants::label_sort_index`),
//! unique per registry while the variant exists. The registries are read
//! (reconciled, as the legacy publishers do) once per owner per [`PlockKey`]
//! of its track ([`variant_snapshot`], cached in [`KindsShared`]): a stamp,
//! a clear or a lock edit moves it. Every field is live: the owner's list
//! while observed and only when the key moved (the tick); a variant's
//! fields from the cached snapshot, `current` per tick (the selected step's
//! key, as the chips show it, read once per track per key and step).
//!
//! **Stale handles.** Whenever an owner's key moved, the tick drops its
//! variant instances whose variant is gone, observed or not
//! ([`HostKinds::prune_variants`]): a held handle goes stale rather than
//! becoming a later variant that reuses its label.

use super::*;
use sequencer::plock_variants::{label_sort_index, PlockVariantKey};

/// Which registry of a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum VariantScope {
    Steps,
    Keys,
}

/// One variant as of its registry's last read.
pub(crate) struct VariantEntry {
    pub(super) vid: u64,
    pub(super) label: String,
    pub(super) name: String,
    pub(super) count: usize,
    pub(super) color: [f32; 3],
    pub(super) key: PlockVariantKey,
    /// A key-lock variant's keys, ascending.
    pub(super) notes: Vec<u8>,
}

/// One registry of a track, read once per [`PlockKey`].
#[derive(Default)]
pub(crate) struct VariantSnapshot {
    pub(super) entries: Vec<VariantEntry>,
}

impl VariantSnapshot {
    fn entry(&self, vid: u64) -> Option<&VariantEntry> {
        self.entries.iter().find(|entry| entry.vid == vid)
    }
}

/// Read (reconciling, as the legacy publishers do) one registry of `track`.
fn read_variants(state: &SequencerState, track: usize, scope: VariantScope) -> VariantSnapshot {
    let (registry, assignments) = match scope {
        VariantScope::Steps => (state.plock_variant_registry_snapshot(track), Vec::new()),
        VariantScope::Keys => state.key_lock_variant_registry_with_assignments(track),
    };
    let entries = registry
        .entries
        .into_iter()
        .map(|entry| {
            let notes = (assignments.iter().enumerate())
                .filter(|(_, assigned)| {
                    (assigned.as_ref()).is_some_and(|assigned| assigned.label == entry.label)
                })
                .map(|(note, _)| note as u8)
                .collect();
            let chip = VariantChip::of(&entry);
            VariantEntry {
                vid: label_sort_index(&entry.label) as u64,
                label: chip.label,
                name: chip.name,
                count: chip.count,
                color: chip.color,
                key: entry.key,
                notes,
            }
        })
        .collect();
    VariantSnapshot { entries }
}

/// `track`'s registry for `scope`, cached under the track's [`PlockKey`].
pub(super) fn variant_snapshot(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
    scope: VariantScope,
) -> Rc<VariantSnapshot> {
    if !sources.track_exists(track) {
        return Rc::default();
    }
    let key = (sources.plock_key(track), 0);
    plock_cached(
        shared,
        |shared| &mut shared.variants,
        (track, scope),
        key,
        || read_variants(&sources.state, track, scope),
    )
}

/// The variant instances of `owner` (a track instance for step variants, a
/// track instrument's device instance for key-lock variants), in registry
/// order: registers the missing ones (with their `track` and `device`) and
/// drops those whose variant is gone. `track_id` is the owner's track
/// instance.
pub(super) fn owner_variants<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    (owner, track_id): (InstanceId, InstanceId),
    track: usize,
    scope: VariantScope,
) -> Value {
    let snapshot = variant_snapshot(sources, shared, track, scope);
    let wanted: Vec<u64> = snapshot.entries.iter().map(|entry| entry.vid).collect();
    let device = (scope == VariantScope::Keys).then_some(owner);
    let (ids, _) = reconcile_children(store, owner, VARIANT, &wanted, |store, id, _| {
        store.push(id, f::VARIANT_TRACK, Value::Instance(track_id));
        store.push(id, f::VARIANT_DEVICE, instance_or_nil(device));
    });
    // The owner's instances now match its registry under this key.
    let key = sources.plock_key(track);
    shared.borrow_mut().variant_owners.insert(owner, Some(key));
    instance_list(ids.into_iter().flatten().collect::<Vec<_>>())
}

/// Where an owner's variant registry is: its track position and scope.
fn owner_registry<S: KindStore>(
    store: &S,
    shared: &RefCell<KindsShared>,
    owner: InstanceId,
) -> Option<(usize, VariantScope)> {
    match store.kind_of(owner)? {
        TRACK => Some((*store.key_of(owner)?.first()? as usize, VariantScope::Steps)),
        _ => {
            let track = shared.borrow().devices.get(&owner)?.owner;
            Some((track, VariantScope::Keys))
        }
    }
}

/// Where a variant instance's registry is: its track position and scope.
fn variant_owner<S: KindStore>(
    store: &S,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
) -> Option<(usize, VariantScope, u64)> {
    let &[owner, vid] = store.key_of(id)? else {
        return None;
    };
    let (track, scope) = owner_registry(store, shared, owner)?;
    Some((track, scope, vid))
}

/// Whether the selected step (the first, as the chips show it) plays
/// `entry` (a step variant). The step's variant key is read once per track
/// per [`PlockKey`] and selected step, however many variants ask.
fn variant_current(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
    entry: &VariantEntry,
) -> bool {
    let step = selected_plock_step(&sources.selected_steps);
    let key = sources.plock_key(track);
    let mut shared = shared.borrow_mut();
    if let Some((cached, at, playing)) = shared.variant_current.get(&track) {
        if *cached == key && *at == step {
            return playing.as_ref() == Some(&entry.key);
        }
    }
    let playing = step.and_then(|step| {
        sequencer::plock_variants::live_track_variant_key(&sources.state, track, step)
    });
    let current = playing.as_ref() == Some(&entry.key);
    shared.variant_current_scans += 1;
    shared.variant_current.insert(track, (key, step, playing));
    current
}

/// One live field of a variant.
pub(super) fn variant_live_value<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    let (track, scope, vid) = variant_owner(store, shared, id)?;
    let snapshot = variant_snapshot(sources, shared, track, scope);
    let entry = snapshot.entry(vid)?;
    Some(match key {
        f::VARIANT_LABEL => text(&entry.label),
        f::VARIANT_NAME => text(&entry.name),
        f::VARIANT_COUNT => number(entry.count as f64),
        f::VARIANT_COLOR => rgb3(entry.color),
        f::VARIANT_NOTES => list_value(entry.notes.iter().map(|note| number(*note))),
        f::VARIANT_CURRENT => Value::Bool(
            scope == VariantScope::Steps && variant_current(sources, shared, track, entry),
        ),
        _ => return None,
    })
}

impl HostKinds {
    /// The observed fields of every variant ([`ObservedList`] over the
    /// owners' variants): `current` per tick; the rest only when the
    /// owner's track's [`PlockKey`] moved.
    pub(super) fn sync_variant_live(&mut self, pusher: &mut Pusher<'_>) {
        if self.prune_variants(pusher) {
            self.panel.variant_observed.reset();
        }
        let (rt, tracks, devices) = (&*pusher.rt, &self.track_ids, &self.device_ids);
        self.panel
            .variant_observed
            .refresh(rt, &VARIANT_LIVE.names, || {
                let owners = tracks.iter().flatten().chain(devices).copied();
                children_of_owners(rt, owners, VARIANT)
            });
        let current = VARIANT_LIVE.bit(f::VARIANT_CURRENT);
        let (sources, shared) = (pusher.sources, pusher.shared);
        for entry in &mut self.panel.variant_observed.entries {
            let (id, mut mask, seen) = *entry;
            let Some((track, _, _)) = variant_owner(&*pusher.rt, shared, id) else {
                continue;
            };
            let key = sources.plock_key(track);
            if seen == Some(key) {
                mask &= current;
            }
            entry.2 = Some(key);
            pusher.push_live_masked(id, &VARIANT_LIVE, mask);
        }
    }

    /// Drop the variant instances whose variant is gone, for every owner
    /// that registered some (observed or not), when its track's
    /// [`PlockKey`] moved; forget owners that are gone. Returns whether an
    /// instance was dropped.
    pub(super) fn prune_variants(&mut self, pusher: &mut Pusher<'_>) -> bool {
        let (sources, shared) = (pusher.sources, pusher.shared);
        let owners: Vec<(InstanceId, Option<PlockKey>)> = (shared.borrow().variant_owners.iter())
            .map(|(owner, key)| (*owner, *key))
            .collect();
        let mut dropped = false;
        for (owner, seen) in owners {
            let registry = pusher
                .rt
                .instance_is_live(owner)
                .then(|| owner_registry(&*pusher.rt, shared, owner))
                .flatten();
            let Some((track, scope)) = registry.filter(|(track, _)| sources.track_exists(*track))
            else {
                shared.borrow_mut().variant_owners.remove(&owner);
                continue;
            };
            let key = sources.plock_key(track);
            if seen == Some(key) {
                continue;
            }
            let snapshot = variant_snapshot(sources, shared, track, scope);
            let gone = pusher.rt.children_where(owner, VARIANT, &|vid| {
                !snapshot.entries.iter().any(|entry| entry.vid == vid)
            });
            for id in gone {
                pusher.rt.drop_instance(id);
                pusher.changed = true;
                dropped = true;
            }
            shared.borrow_mut().variant_owners.insert(owner, Some(key));
        }
        dropped
    }
}
