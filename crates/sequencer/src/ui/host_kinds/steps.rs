//! Steps: the step selection and the per-track step diff the tick pushes
//! from.

use super::*;

/// The observed-mask bits (in `STEP_LIVE` order) the per-tick step diff
/// uses, computed once.
struct StepBits {
    active: u32,
    playing: u32,
    selected: u32,
    /// Per [`STEP_VALUES`] field: its bit and its parameter (`None` for
    /// `held`).
    values: [(u32, Option<StepParam>); STEP_VALUES.len()],
    /// `plocked`, `lock-kind`, `variant-color` and `variant`.
    plocked: u32,
    lock_kind: u32,
    variant_color: u32,
    variant: u32,
}

static STEP_BITS: LazyLock<StepBits> = LazyLock::new(|| StepBits {
    active: STEP_LIVE.bit(f::STEP_ACTIVE),
    playing: STEP_LIVE.bit(f::STEP_PLAYING),
    selected: STEP_LIVE.bit(f::STEP_SELECTED),
    values: STEP_VALUES.map(|key| (STEP_LIVE.bit(key), step_param_named(key.1))),
    plocked: STEP_LIVE.bit(f::STEP_PLOCKED),
    lock_kind: STEP_LIVE.bit(f::STEP_LOCK_KIND),
    variant_color: STEP_LIVE.bit(f::STEP_VARIANT_COLOR),
    variant: STEP_LIVE.bit(f::STEP_VARIANT),
});

/// The step selection as of the last sync, so `step.selected` is
/// recomputed only when it (or the current track) changes.
#[derive(Default)]
pub(super) struct StepSelection {
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
    pub(super) fn refresh(&mut self, sources: &KindsHandles) -> bool {
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
pub(super) struct StepDiff {
    /// The track's length at the last sync: steps are dropped only when it
    /// shrinks.
    num_steps: Option<usize>,
    /// The vectors and `playing` hold the values last pushed; when false
    /// every step is a candidate.
    primed: bool,
    active: Vec<bool>,
    selected: Vec<bool>,
    playing: Option<usize>,
    /// The values last pushed of each [`STEP_VALUES`] field, per step; empty
    /// while nothing observes the field.
    values: [Vec<f64>; STEP_VALUES.len()],
    /// The track's `held` flags this tick (a reused buffer).
    held: Vec<bool>,
    /// The p-lock render last pushed per step, and the track's
    /// [`PlockKey`] then; empty while nothing observes it. Diffed only when
    /// the key moves.
    plocks: Vec<StepPlockRender>,
    plock_key: Option<PlockKey>,
    /// The union of the step instances' observed fields (bit `i` is
    /// `STEP_LIVE.keys[i]`), as of `Runtime::instance_observer_epoch`.
    observers: Option<(u64, u32)>,
}

/// Step fields of one track. Steps are dropped only when the track's
/// length shrank. Nothing more happens for a track with no step instances,
/// or while neither its `steps` nor any step field is observed (then the
/// diff starts over when something observes again). Otherwise `active`,
/// `selected` (only when the selection changed) and `playing` are diffed in
/// place, and only observed changed fields are pushed.
#[allow(clippy::too_many_arguments)]
pub(super) fn sync_steps(
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
        && drop_children_past(&mut *pusher.rt, track_id, STEP, num_steps)
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
    let union = {
        let rt = &*pusher.rt;
        let steps = rt.keyed_children_of_kind(track_id, STEP).map(|(id, _)| id);
        observed_union(rt, &mut diff.observers, steps, &STEP_LIVE)
    };
    if !steps_observed && union == 0 {
        diff.primed = false;
        return;
    }
    let bits = &*STEP_BITS;
    let primed = diff.primed;
    changes.clear();
    changes.resize(num_steps, 0);
    diff.active.resize(num_steps, false);
    diff.selected.resize(num_steps, false);
    let state = &pusher.sources.state;
    let pattern = &state.pattern.patterns[track];
    for (step, previous) in diff.active.iter_mut().enumerate() {
        let active = pattern.is_active(step);
        if !primed || *previous != active {
            *previous = active;
            changes[step] |= bits.active;
        }
    }
    if !primed || selection_changed {
        for (step, previous) in diff.selected.iter_mut().enumerate() {
            let selected = selection.selected(track, step);
            if !primed || *previous != selected {
                *previous = selected;
                changes[step] |= bits.selected;
            }
        }
    }
    let playing = pusher.sources.playing_step(track);
    if !primed {
        changes
            .iter_mut()
            .for_each(|change| *change |= bits.playing);
    } else if playing != diff.playing {
        for step in [diff.playing, playing].into_iter().flatten() {
            if let Some(change) = changes.get_mut(step) {
                *change |= bits.playing;
            }
        }
    }
    diff.playing = playing;
    // The value fields: only those some step of the track observes.
    for (slot, &(bit, param)) in bits.values.iter().enumerate() {
        if union & bit == 0 {
            diff.values[slot].clear();
            continue;
        }
        if param.is_none() {
            fill_track_held_steps(state, track, num_steps, &mut diff.held);
        }
        let last = &mut diff.values[slot];
        let fresh = !primed || last.len() != num_steps;
        last.resize(num_steps, f64::NAN);
        for (step, previous) in last.iter_mut().enumerate() {
            let value = match param {
                Some(param) => pusher.sources.step_param(track, step, param),
                None => f64::from(u8::from(diff.held[step])),
            };
            if fresh || *previous != value {
                *previous = value;
                changes[step] |= bit;
            }
        }
    }
    // The p-lock render: a whole-track scan (cached per track), so only
    // when the track's p-locks may have moved (or it starts being observed).
    let plock_bits = bits.plocked | bits.lock_kind | bits.variant_color | bits.variant;
    let plock_key = pusher.sources.plock_key(track);
    if union & plock_bits == 0 {
        diff.plocks.clear();
        diff.plock_key = None;
    } else if !primed || diff.plock_key != Some(plock_key) || diff.plocks.len() != num_steps {
        let fresh = !primed || diff.plocks.len() != num_steps;
        diff.plock_key = Some(plock_key);
        let render = track_plock_render(pusher.sources, pusher.shared, track);
        diff.plocks.resize(num_steps, StepPlockRender::default());
        for (step, previous) in diff.plocks.iter_mut().enumerate() {
            let now = render.get(step).copied().unwrap_or_default();
            if fresh || previous.plocked != now.plocked {
                changes[step] |= bits.plocked;
            }
            if fresh || previous.kind != now.kind {
                changes[step] |= bits.lock_kind;
            }
            if fresh || previous.color != now.color {
                changes[step] |= bits.variant_color;
            }
            if fresh || previous.variant != now.variant {
                changes[step] |= bits.variant;
            }
            *previous = now;
        }
        let reconciled =
            pusher.shared.borrow().variant_owners.get(&track_id) == Some(&Some(plock_key));
        if union & bits.variant != 0 && !reconciled {
            // The track's variant instances as its registry is now (a
            // variant gone since drops its instance first), so `variant`
            // never names a stale one.
            let (sources, shared) = (pusher.sources, pusher.shared);
            let owner = (track_id, track_id);
            owner_variants(
                &mut *pusher.rt,
                sources,
                shared,
                owner,
                track,
                VariantScope::Steps,
            );
        }
    }
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
            let bit = 1 << bit;
            if wanted & bit == 0 {
                continue;
            }
            // The render is a whole-track scan: push the one just made.
            match diff.plocks.get(step) {
                Some(render) if bit == bits.variant => {
                    // The track's variants were reconciled with the render.
                    let variant = (render.variant)
                        .and_then(|vid| pusher.rt.keyed_instance(VARIANT, &[track_id, vid]));
                    pusher.push_computed(id, *key, instance_or_nil(variant));
                }
                Some(render) if bit & plock_bits != 0 => {
                    if let Some(value) = render.field(*key) {
                        pusher.push_computed(id, *key, value);
                    }
                }
                _ => pusher.push_live_field(id, *key),
            }
        }
    }
}
