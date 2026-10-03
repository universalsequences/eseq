/*!
Live values of DGen `(probe …)` taps for visible patchers
(docs/patcher-probes-spec.md §6.1).

On each analyzer poll (not every event-loop pass) the analyzer finds the
visible `patcher` widgets (by their `:path`), matches each path to the edit
session that opened it ([`PatcherProbeSource`], built from the event loop's
sessions only when some patcher is visible), and resolves the session's
target to a live DGen instance ([`resolve_probe_instance`]). The node id of an
effect changes on every recompile, so resolution runs every poll.

# Seam for the patcher widget (slice .5)

- `eseqlisp::live_audio::patch_probe_instance(path) -> Option<Arc<str>>` is
  the instance key the patcher at `path` reads from (`eng<engine_id>` /
  `fx<node_id>`), absent when no live instance backs it. Resolve it once per
  paint.
- `eseqlisp::live_audio::probe_frame_for(instance, id, occurrence)` is the
  per-node read: the latest [`ProbeFrame`] of probe `id`#`occurrence`, or
  `None` (show `—`). `patch_probe_frame(path, id, occurrence)` does both in
  one call. Every read registers a paint dependency, so publishing a frame
  or rebinding the path repaints the patcher without a layout rerun.
- Frames live under `probe_frame_key(instance, id, occurrence)` =
  `probe:<instance>:<id>#<occurrence>`.
- A scope frame carries `display_range`, the eased y-range to draw its trace
  in ([`scope_target_range`] eased by [`relax_scope_range`]), so the painter
  keeps no easing state of its own.

# Publish policy

A frame is republished only when `last`/`min`/`max` move past a relative
epsilon ([`value_moved`]), the stale flag flips, a scope trace changes, or a
scope's eased range has not yet reached its target, so a static patch stops
redrawing. `stale` means the capture's block counter has not
advanced for [`PROBE_STALE_AFTER`] (display voice idle, effect bypassed).

The capture gate (`PROBES_WATCHED`) is held through a [`ProbeWatchGuard`]
exactly while some visible patcher resolves to a live instance that exposes
probes.
*/

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eseqlisp::layout::LayoutNode;
use eseqlisp::live_audio::{self, ProbeFrame};
use eseqlisp::vm::Value;
use sequencer::app;
use sequencer::lisp_host::{
    self, ProbeInstance, ProbeReading, ProbeSetHandle, ProbeView, ProbeWatchGuard,
};

use crate::edit_sessions::EffectEditTarget;
use crate::loop_ctx::EditSessionState;

/// A probe whose capture counter has not advanced for this long is shown as
/// held. Several polls long so a large audio block (or a poll landing between
/// two blocks) never flickers the flag.
pub(crate) const PROBE_STALE_AFTER: Duration = Duration::from_millis(150);
/// Relative change that republishes a probe value: below the ~4 significant
/// digits the number view shows.
pub(crate) const PROBE_RELATIVE_EPSILON: f32 = 1e-4;
/// Magnitude floor for [`PROBE_RELATIVE_EPSILON`], so denormal-level jitter
/// around zero does not count as movement.
const PROBE_EPSILON_FLOOR: f32 = 1e-3;
/// Most columns the patcher's scope view paints (it bins the trace into at
/// most this many); more pairs than that would only be binned away.
pub(crate) const PROBE_SCOPE_MAX_PAINT_COLUMNS: usize = 512;
/// Scope pairs published per frame: the newest that the scope view can
/// paint, at most the whole capture ring.
pub(crate) const PROBE_SCOPE_PUBLISH_PAIRS: usize = if PROBE_SCOPE_MAX_PAINT_COLUMNS
    < lisp_host::PROBE_SCOPE_RING_PAIRS
{
    PROBE_SCOPE_MAX_PAINT_COLUMNS
} else {
    lisp_host::PROBE_SCOPE_RING_PAIRS
};
/// Headroom added above and below a scope trace, as a fraction of its span.
const SCOPE_RANGE_HEADROOM: f32 = 0.08;
/// Fraction of the way a scope's displayed range narrows toward a smaller
/// target per published frame. Widening is immediate.
const SCOPE_RANGE_RELAX: f32 = 0.2;

/// What an open patch-editor session edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatcherProbeTarget {
    /// A track's custom instrument; its engine is the instance.
    Instrument { track: usize },
    /// A track or bus fx-chain slot; its live node is the instance.
    Effect(EffectEditTarget),
}

/// A patch path (the `patcher` widget's `:path`) and the target it edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatcherProbeSource {
    pub(crate) path: String,
    pub(crate) target: PatcherProbeTarget,
}

/// The patch paths the event loop's edit sessions own. A patcher whose path
/// matches none of them (a file opened without a live target) gets no
/// instance, and its probes show `—`.
pub(crate) fn patcher_probe_sources(sessions: &EditSessionState) -> Vec<PatcherProbeSource> {
    let instrument = sessions.instrument_edit_session.as_ref().map(|session| PatcherProbeSource {
        path: session.path.to_string_lossy().into_owned(),
        target: PatcherProbeTarget::Instrument { track: session.track },
    });
    let effect = sessions.effect_edit_session.as_ref().map(|session| PatcherProbeSource {
        path: session.path.to_string_lossy().into_owned(),
        target: PatcherProbeTarget::Effect(session.target.clone()),
    });
    instrument.into_iter().chain(effect).collect()
}

/// The live DGen instance `target` names right now, if any.
pub(crate) fn resolve_probe_instance(
    app: &app::App,
    target: &PatcherProbeTarget,
) -> Option<ProbeInstance> {
    let effect = |node_id: i32| (node_id > 0).then_some(ProbeInstance::Effect { node_id });
    match *target {
        PatcherProbeTarget::Instrument { track } => app
            .graph
            .track_engine_ids
            .get(track)
            .copied()
            .flatten()
            .map(|engine_id| ProbeInstance::Instrument { engine_id }),
        PatcherProbeTarget::Effect(EffectEditTarget::Track { track, slot }) => app
            .state
            .pattern
            .effect_chains
            .get(track)
            .and_then(|chain| chain.get(slot))
            .and_then(|slot| effect(slot.node_id.load(Ordering::Relaxed) as i32)),
        PatcherProbeTarget::Effect(EffectEditTarget::Bus { bus, slot }) => app
            .buses
            .get(bus)
            .and_then(|bus| bus.effect_slots.get(slot))
            .and_then(|slot| effect(slot.node_id as i32)),
    }
}

pub(crate) fn probe_instance_key(instance: ProbeInstance) -> String {
    match instance {
        ProbeInstance::Instrument { engine_id } => {
            live_audio::probe_instance_key_for_instrument(engine_id)
        }
        ProbeInstance::Effect { node_id } => live_audio::probe_instance_key_for_effect(node_id),
    }
}

/// Paths of the `patcher` widgets laid out with a nonzero size.
pub(crate) fn collect_visible_patcher_paths(layout: &LayoutNode, paths: &mut HashSet<String>) {
    if layout.widget_type == "patcher" && layout.rect.width > 0.0 && layout.rect.height > 0.0 {
        if let Some(Value::String(path)) = layout.props.get("path") {
            paths.insert(path.to_string());
        }
    }
    for child in &layout.children {
        collect_visible_patcher_paths(child, paths);
    }
}

/// Whether two probe values differ enough to republish. Non-finite values
/// compare by bits, so NaN → NaN is steady and finite ↔ NaN always moves.
pub(crate) fn value_moved(a: f32, b: f32) -> bool {
    if !(a.is_finite() && b.is_finite()) {
        return a.to_bits() != b.to_bits();
    }
    let scale = a.abs().max(b.abs()).max(PROBE_EPSILON_FLOOR);
    (a - b).abs() > PROBE_RELATIVE_EPSILON * scale
}

fn range_moved(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => value_moved(a.0, b.0) || value_moved(a.1, b.1),
        (None, None) => false,
        _ => true,
    }
}

/// Finite min/max over the trace, or `None` when it holds no finite value.
fn trace_extent(pairs: &[(f32, f32)]) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &(min, max) in pairs {
        for value in [min, max] {
            if value.is_finite() {
                lo = lo.min(value);
                hi = hi.max(value);
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// The y range a scope would show for this trace with no history: the
/// trace's finite extent with [`SCOPE_RANGE_HEADROOM`], stretched to include
/// 0 when the signal sits near it (a 0.2…0.9 envelope reads better on a
/// 0-based axis than floating), and widened ±10% around a flat line so it
/// doesn't collapse to a zero span.
pub(crate) fn scope_target_range(pairs: &[(f32, f32)]) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = trace_extent(pairs)?;
    // Include zero when the trace's near edge is within half its far edge's
    // distance of it: 0.2..0.9 or 200..5000 gain a baseline, while 439..441
    // (a steady value with a little wobble) keeps a tight axis so the wobble
    // stays visible. A signal that crosses zero includes it already.
    if lo > 0.0 && lo <= hi * 0.5 {
        lo = 0.0;
    } else if hi < 0.0 && hi >= lo * 0.5 {
        hi = 0.0;
    }
    let span = hi - lo;
    if span <= f32::EPSILON * hi.abs().max(lo.abs()).max(1.0) {
        let pad = (hi.abs() * 0.1).max(1e-3);
        return Some((lo - pad, hi + pad));
    }
    let pad = span * SCOPE_RANGE_HEADROOM;
    Some((lo - pad, hi + pad))
}

/// One easing step of a displayed scope range toward `target`: an edge that
/// must grow to fit the trace jumps there at once; one that may shrink
/// closes [`SCOPE_RANGE_RELAX`] of the gap, so a passing spike doesn't leave
/// the trace squashed for long and a transient dip doesn't make it jump.
/// Snaps to `target` once a step is below the republish epsilon, and falls
/// back to `target` if the result is not a valid range.
pub(crate) fn relax_scope_range(previous: (f32, f32), target: (f32, f32)) -> (f32, f32) {
    let lo = if target.0 < previous.0 {
        target.0
    } else {
        previous.0 + (target.0 - previous.0) * SCOPE_RANGE_RELAX
    };
    let hi = if target.1 > previous.1 {
        target.1
    } else {
        previous.1 + (target.1 - previous.1) * SCOPE_RANGE_RELAX
    };
    let relaxed = (lo, hi);
    // Snap once the step itself is below the republish epsilon: otherwise
    // the publisher would stop short of the target, a step it never sends.
    if !(lo.is_finite() && hi.is_finite() && lo < hi) || !range_moved(Some(previous), Some(relaxed)) {
        target
    } else {
        relaxed
    }
}

/// Per-probe publish state, aligned with one slot of an instance's probe
/// set.
#[derive(Debug)]
pub(crate) struct ProbeTrack {
    /// `probe:<instance>:<id>#<occurrence>`, built once when the track is.
    frame_key: String,
    /// Capture counter at the last poll.
    seq: u64,
    /// When `seq` last advanced (or was first seen).
    seq_changed_at: Instant,
    /// Scope view: the range the latest trace asks for, which the displayed
    /// range keeps easing toward even while no new blocks arrive.
    scope_target: Option<(f32, f32)>,
    /// The frame last published under this key.
    published: Option<ProbeFrame>,
}

impl ProbeTrack {
    pub(crate) fn new(frame_key: String) -> Self {
        Self {
            frame_key,
            seq: 0,
            seq_changed_at: Instant::now(),
            scope_target: None,
            published: None,
        }
    }

    /// Fold one poll's reading in and return the frame to publish, if
    /// anything visible changed. `scope` is the current trace for a `scope`
    /// view (read only when it can have changed; `None` keeps the last one).
    /// A scope's displayed range advances one easing step per call until it
    /// reaches the trace's target, each step a new frame.
    pub(crate) fn observe(
        &mut self,
        reading: ProbeReading,
        scope: Option<&[(f32, f32)]>,
        now: Instant,
    ) -> Option<ProbeFrame> {
        if self.published.is_none() || reading.seq != self.seq {
            self.seq_changed_at = now;
        }
        self.seq = reading.seq;
        let stale = now.duration_since(self.seq_changed_at) >= PROBE_STALE_AFTER;
        let previous_scope = self.published.as_ref().and_then(|frame| frame.scope.as_ref());
        let scope = match (scope, previous_scope) {
            (Some(next), Some(previous)) if previous.as_slice() == next => Some(previous.clone()),
            (Some(next), _) => {
                self.scope_target = scope_target_range(next);
                Some(Arc::new(next.to_vec()))
            }
            (None, previous) => previous.cloned(),
        };
        let previous_range = self.published.as_ref().and_then(|frame| frame.display_range);
        let display_range = match (previous_range, self.scope_target) {
            (Some(previous), Some(target)) => Some(relax_scope_range(previous, target)),
            (None, target) => target,
            (Some(_), None) => None,
        };
        let moved = match &self.published {
            None => true,
            Some(previous) => {
                previous.stale != stale
                    || value_moved(previous.last, reading.last)
                    || value_moved(previous.min, reading.min)
                    || value_moved(previous.max, reading.max)
                    || range_moved(previous.display_range, display_range)
                    || match (&previous.scope, &scope) {
                        (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                        (None, None) => false,
                        _ => true,
                    }
            }
        };
        if !moved {
            return None;
        }
        let frame = ProbeFrame {
            revision: self.published.as_ref().map_or(1, |frame| frame.revision + 1),
            last: reading.last,
            min: reading.min,
            max: reading.max,
            seq: reading.seq,
            stale,
            scope,
            display_range,
        };
        self.published = Some(frame.clone());
        Some(frame)
    }

    fn seq(&self) -> Option<u64> {
        self.published.as_ref().map(|_| self.seq)
    }
}

/// One watched instance: the probe set last read and a track per slot.
struct InstanceProbes {
    set: ProbeSetHandle,
    tracks: Vec<ProbeTrack>,
}

/// Publishes the probe frames of every visible patcher's live instance.
pub(crate) struct ProbePublisher {
    watch: Option<ProbeWatchGuard>,
    instances: HashMap<ProbeInstance, InstanceProbes>,
    /// Whether the last pass published any path binding (so an empty pass
    /// has something to unbind).
    bound: bool,
    scope_scratch: Vec<(f32, f32)>,
}

impl ProbePublisher {
    pub(crate) fn new() -> Self {
        Self { watch: None, instances: HashMap::new(), bound: false, scope_scratch: Vec::new() }
    }

    pub(crate) fn watching(&self) -> bool {
        self.watch.is_some()
    }

    /// One poll. `bindings` pairs each visible patcher path with the instance
    /// it resolved to. Rebinds paths, keeps the capture gate open exactly
    /// while some binding exposes probes, and publishes every probe whose
    /// value moved. Returns whether anything a patcher paints changed. With
    /// no bindings and nothing left from earlier passes it returns at once.
    pub(crate) fn sync(&mut self, bindings: &[(String, Option<ProbeInstance>)], now: Instant) -> bool {
        if bindings.is_empty() && !self.bound && self.watch.is_none() && self.instances.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut active_paths = HashSet::new();
        let mut watched: Vec<ProbeInstance> = Vec::new();
        for (path, instance) in bindings {
            active_paths.insert(path.clone());
            let key = instance.map(probe_instance_key);
            changed |= live_audio::publish_patch_probe_instance(path, key.as_deref());
            if let Some(instance) = instance {
                if !watched.contains(instance) {
                    watched.push(*instance);
                }
            }
        }
        live_audio::retain_patch_probe_instances(&active_paths);
        self.bound = !bindings.is_empty();

        // One registry lookup per watched instance; frame keys are built
        // only when an instance's probe set is new or was rebuilt.
        let mut keys_changed = false;
        let mut previous = std::mem::take(&mut self.instances);
        for instance in watched {
            let Some(set) = lisp_host::probe_set(instance) else {
                keys_changed |= previous.remove(&instance).is_some();
                continue;
            };
            let entry = match previous.remove(&instance) {
                Some(entry) if entry.set.same_set(&set) => entry,
                old => {
                    keys_changed = true;
                    let mut old_tracks: HashMap<String, ProbeTrack> = old
                        .into_iter()
                        .flat_map(|entry| entry.tracks)
                        .map(|track| (track.frame_key.clone(), track))
                        .collect();
                    let instance_key = probe_instance_key(instance);
                    let tracks = set
                        .probes()
                        .map(|probe| {
                            let key = live_audio::probe_frame_key(&instance_key, &probe.id, probe.occurrence);
                            old_tracks.remove(&key).unwrap_or_else(|| ProbeTrack::new(key))
                        })
                        .collect();
                    InstanceProbes { set, tracks }
                }
            };
            self.instances.insert(instance, entry);
        }
        keys_changed |= !previous.is_empty();
        drop(previous);

        // The gate is open exactly while some visible patcher shows probes.
        match (self.instances.is_empty(), self.watch.is_some()) {
            (false, false) => self.watch = Some(ProbeWatchGuard::new()),
            (true, true) => self.watch = None,
            _ => {}
        }
        if keys_changed {
            let active_keys: HashSet<String> = self
                .instances
                .values()
                .flat_map(|entry| entry.tracks.iter().map(|track| track.frame_key.clone()))
                .collect();
            live_audio::retain_probe_frames(&active_keys);
        }

        lisp_host::reclaim_retired_probe_sets();
        let scratch = &mut self.scope_scratch;
        for entry in self.instances.values_mut() {
            for (index, (probe, track)) in entry.set.probes().zip(&mut entry.tracks).enumerate() {
                let Some(reading) = entry.set.reading(index) else {
                    continue;
                };
                let scope_changed = probe.view == ProbeView::Scope && track.seq() != Some(reading.seq);
                let scope = (scope_changed
                    && entry.set.read_scope(index, PROBE_SCOPE_PUBLISH_PAIRS, scratch))
                .then_some(scratch.as_slice());
                if let Some(frame) = track.observe(reading, scope, now) {
                    live_audio::publish_probe_frame(track.frame_key.clone(), frame);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Drop every binding, frame and the capture gate (project load).
    pub(crate) fn clear(&mut self) -> bool {
        let had_live_data = self.watch.is_some() || !self.instances.is_empty();
        self.watch = None;
        self.instances.clear();
        self.bound = false;
        live_audio::retain_patch_probe_instances(&HashSet::new());
        live_audio::retain_probe_frames(&HashSet::new());
        had_live_data
    }
}

#[cfg(test)]
mod tests {
    //! Every test that touches the probe registries or the watch gate relies
    //! on nextest's process-per-test isolation (they are process-global).

    use super::*;
    use sequencer::audiograph::{self, LiveGraphPtr};

    fn reading(last: f32, min: f32, max: f32, seq: u64) -> ProbeReading {
        ProbeReading { last, min, max, seq }
    }

    fn manifest_with_probes() -> sequencer::lisp_host::DGenManifest {
        sequencer::lisp_host::parse_manifest(
            r#"{"processAbi": "dgen-host-abi-v1",
                "outputs": [{"channel": 0}, {"channel": 1}],
                "probes": [
                  {"id": "cut", "occurrence": 0, "channel": 2, "view": "number"},
                  {"id": "cut", "occurrence": 1, "channel": 3, "view": "scope"}
                ]}"#,
        )
        .expect("manifest parses")
    }

    #[test]
    fn value_moved_uses_a_relative_epsilon_and_compares_non_finite_by_bits() {
        assert!(!value_moved(1000.0, 1000.05), "below 4 significant digits");
        assert!(value_moved(1000.0, 1000.5));
        assert!(!value_moved(0.0, 1e-8), "denormal-level jitter around zero");
        assert!(value_moved(0.0, 1e-3));
        assert!(value_moved(-0.5, 0.5));
        assert!(!value_moved(f32::NAN, f32::NAN), "NaN held steady");
        assert!(value_moved(0.0, f32::NAN));
        assert!(value_moved(f32::INFINITY, f32::NEG_INFINITY));
    }

    #[test]
    fn probe_track_publishes_on_movement_and_stale_flips_only() {
        let t0 = Instant::now();
        let mut track = ProbeTrack::new("probe:t:a#0".to_string());
        let first = track.observe(reading(0.5, 0.25, 0.75, 1), None, t0).expect("first frame");
        assert_eq!((first.revision, first.last, first.stale), (1, 0.5, false));

        // Capture advanced, values unchanged: no redraw.
        let t1 = t0 + Duration::from_millis(33);
        assert_eq!(track.observe(reading(0.5, 0.25, 0.75, 9), None, t1), None);
        // Jitter below epsilon: no redraw.
        assert_eq!(track.observe(reading(0.500_01, 0.25, 0.75, 20), None, t1), None);
        // A real move publishes.
        let moved = track.observe(reading(0.6, 0.25, 0.75, 30), None, t1).expect("moved");
        assert_eq!((moved.revision, moved.last, moved.seq), (2, 0.6, 30));

        // The counter stops (display voice idle): stale once it has held for
        // PROBE_STALE_AFTER, not on the very next poll.
        assert_eq!(
            track.observe(reading(0.6, 0.25, 0.75, 30), None, t1 + Duration::from_millis(33)),
            None
        );
        let stale = track
            .observe(reading(0.6, 0.25, 0.75, 30), None, t1 + PROBE_STALE_AFTER)
            .expect("stale flips");
        assert!(stale.stale);
        assert_eq!(stale.last, 0.6, "the held value stays visible");
        assert_eq!(
            track.observe(reading(0.6, 0.25, 0.75, 30), None, t1 + PROBE_STALE_AFTER * 2),
            None,
            "stays stale without republishing"
        );
        // Capture resumes.
        let live = track
            .observe(reading(0.6, 0.25, 0.75, 31), None, t1 + PROBE_STALE_AFTER * 3)
            .expect("resumes");
        assert!(!live.stale);
    }

    #[test]
    fn probe_track_republishes_a_scope_only_when_the_trace_changes() {
        let now = Instant::now();
        let mut track = ProbeTrack::new("probe:t:a#0".to_string());
        let trace = vec![(-0.5, 0.5); 4];
        let first = track.observe(reading(0.0, -0.5, 0.5, 1), Some(&trace), now).unwrap();
        assert_eq!(first.scope.as_deref(), Some(&trace));
        assert_eq!(
            track.observe(reading(0.0, -0.5, 0.5, 2), Some(&trace), now),
            None,
            "identical trace"
        );
        assert_eq!(
            track.observe(reading(0.0, -0.5, 0.5, 2), None, now),
            None,
            "no new blocks: the last trace is kept"
        );
        let next = vec![(-0.25, 0.25); 4];
        let moved = track.observe(reading(0.0, -0.5, 0.5, 3), Some(&next), now).unwrap();
        assert_eq!(moved.scope.as_deref(), Some(&next));
    }

    #[test]
    fn scope_target_range_pads_includes_zero_and_widens_flat_lines() {
        let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5;
        // Crosses zero: extent plus 8% headroom.
        let range = scope_target_range(&[(-1.0, 1.0)]).unwrap();
        assert!(close(range, (-1.16, 1.16)), "{range:?}");
        // Near edge within half the far edge: 0 joins the axis.
        let range = scope_target_range(&[(0.2, 0.9)]).unwrap();
        assert!(close(range, (-0.072, 0.972)), "{range:?}");
        let range = scope_target_range(&[(-0.9, -0.2)]).unwrap();
        assert!(close(range, (-0.972, 0.072)), "{range:?}");
        // A steady value with a little wobble keeps a tight axis.
        let range = scope_target_range(&[(439.0, 441.0)]).unwrap();
        assert!(range.0 > 438.0 && range.1 < 442.0, "{range:?}");
        // A flat line gets ±10%.
        let range = scope_target_range(&[(2.0, 2.0)]).unwrap();
        assert!(close(range, (1.8, 2.2)), "{range:?}");
        // Non-finite values are skipped; nothing finite, no range.
        assert!(close(scope_target_range(&[(f32::NAN, 1.0), (-1.0, f32::INFINITY)]).unwrap(), (-1.16, 1.16)));
        assert_eq!(scope_target_range(&[(f32::NAN, f32::NAN)]), None);
        assert_eq!(scope_target_range(&[]), None);
    }

    #[test]
    fn scope_display_range_widens_at_once_and_narrows_per_frame() {
        let now = Instant::now();
        let mut track = ProbeTrack::new("probe:t:s#0".to_string());
        let wide = vec![(-1.0, 1.0); 4];
        let first = track.observe(reading(0.0, -1.0, 1.0, 1), Some(&wide), now).unwrap();
        let wide_range = scope_target_range(&wide).unwrap();
        assert_eq!(first.display_range, Some(wide_range), "first frame shows its target");

        // The signal shrinks: each frame closes 20% of the gap, even with no
        // new blocks, until it settles and publishing stops.
        let narrow = vec![(-0.1, 0.1); 4];
        let narrow_range = scope_target_range(&narrow).unwrap();
        let step = track.observe(reading(0.0, -0.1, 0.1, 2), Some(&narrow), now).unwrap();
        let (lo, hi) = step.display_range.unwrap();
        assert!((hi - (wide_range.1 + (narrow_range.1 - wide_range.1) * 0.2)).abs() < 1e-6, "{hi}");
        assert!((lo - (wide_range.0 + (narrow_range.0 - wide_range.0) * 0.2)).abs() < 1e-6, "{lo}");
        let mut last = step.display_range.unwrap();
        let mut frames = 0;
        while let Some(frame) = track.observe(reading(0.0, -0.1, 0.1, 2), None, now) {
            let range = frame.display_range.unwrap();
            assert!(range.1 <= last.1 && range.0 >= last.0, "only narrows: {range:?} after {last:?}");
            last = range;
            frames += 1;
            assert!(frames < 200, "easing must settle");
        }
        assert_eq!(last, narrow_range, "settles exactly on the target");

        // A spike widens at once.
        let spike = vec![(-3.0, 3.0); 4];
        let widened = track.observe(reading(0.0, -3.0, 3.0, 3), Some(&spike), now).unwrap();
        assert_eq!(widened.display_range, scope_target_range(&spike));

        // Number views carry no range.
        let mut number = ProbeTrack::new("probe:t:n#0".to_string());
        assert_eq!(number.observe(reading(1.0, 1.0, 1.0, 1), None, now).unwrap().display_range, None);
    }

    fn layout(widget_type: &str, path: Option<&str>, width: f32, children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode {
            widget_id: 0,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: widget_type.to_string(),
            rect: eseqlisp::layout::Rect { row: 0.0, col: 0.0, width, height: 10.0 },
            props: path
                .map(|path| ("path".to_string(), Value::String(path.to_string())))
                .into_iter()
                .collect(),
            children,
            focusable: false,
            animation: Default::default(),
        }
    }

    #[test]
    fn collects_paths_of_patchers_with_a_visible_rect() {
        let root = layout(
            "v-stack",
            None,
            100.0,
            vec![
                layout("patcher", Some("/a/dsp.lisp"), 50.0, Vec::new()),
                layout("patcher", Some("/hidden/dsp.lisp"), 0.0, Vec::new()),
                layout("text-input", Some("/not-a-patcher"), 50.0, Vec::new()),
            ],
        );
        let mut paths = HashSet::new();
        collect_visible_patcher_paths(&root, &mut paths);
        assert_eq!(paths, HashSet::from(["/a/dsp.lisp".to_string()]));
    }

    #[test]
    fn patcher_probe_sources_name_each_session_target() {
        let mut sessions = EditSessionState::default();
        assert!(patcher_probe_sources(&sessions).is_empty());
        sessions.instrument_edit_session =
            Some(crate::edit_sessions::InstrumentEditSession::begin_edit_existing(
                "synth".to_string(),
                "/inst/dsp.lisp".into(),
                "*instrument-patcher:synth*".to_string(),
                4,
                2,
                String::new(),
                sequencer::sequencer::CustomInstrumentRunMode::Instrument,
                crate::edit_sessions::EditorSurface::Patch,
            ));
        sessions.effect_edit_session =
            Some(crate::edit_sessions::EffectEditSession::begin_edit_existing(
                "fx".to_string(),
                "/fx/dsp.lisp".into(),
                "*effect-patcher:fx*".to_string(),
                EffectEditTarget::Bus { bus: 1, slot: 3 },
                String::new(),
                crate::edit_sessions::EditorSurface::Patch,
            ));
        assert_eq!(
            patcher_probe_sources(&sessions),
            vec![
                PatcherProbeSource {
                    path: "/inst/dsp.lisp".to_string(),
                    target: PatcherProbeTarget::Instrument { track: 2 },
                },
                PatcherProbeSource {
                    path: "/fx/dsp.lisp".to_string(),
                    target: PatcherProbeTarget::Effect(EffectEditTarget::Bus { bus: 1, slot: 3 }),
                },
            ]
        );
    }

    fn test_app(name: &std::ffi::CStr) -> (app::App, LiveGraphPtr) {
        unsafe { audiograph::initialize_engine(64, 44_100) };
        let lg = LiveGraphPtr(unsafe { audiograph::create_live_graph(32, 64, name.as_ptr(), 2) });
        assert!(!lg.0.is_null());
        let (keyboard_tx, _keyboard_rx) = std::sync::mpsc::channel();
        let app = app::App::new(
            Arc::new(sequencer::sequencer::SequencerState::new(0, Vec::new())),
            lg,
            44_100,
            app::AudioBuses {
                bus_l_id: 0,
                bus_r_id: 0,
                default_bus_nodes: Vec::new(),
                bus_effect_runtime: Arc::new(std::sync::Mutex::new(Arc::new(Vec::new()))),
                reverb_bus_id: 0,
                reverb_node_id: 0,
            },
            Arc::new(sequencer::recorder::MasterRecorder::new(44_100, 2)),
            keyboard_tx,
        );
        (app, lg)
    }

    #[test]
    fn resolves_each_target_to_its_live_instance() {
        let (mut app, lg) = test_app(c"probe-resolve");
        app.graph_controller().add_blank_sampler_track().expect("add track");
        let slot = sequencer::effects::BUILTIN_SLOT_COUNT;

        let instrument = PatcherProbeTarget::Instrument { track: 0 };
        app.graph.track_engine_ids[0] = None;
        assert_eq!(resolve_probe_instance(&app, &instrument), None, "no engine, no instance");
        app.graph.track_engine_ids[0] = Some(6);
        assert_eq!(
            resolve_probe_instance(&app, &instrument),
            Some(ProbeInstance::Instrument { engine_id: 6 })
        );
        assert_eq!(
            resolve_probe_instance(&app, &PatcherProbeTarget::Instrument { track: 9 }),
            None
        );

        let track_effect = PatcherProbeTarget::Effect(EffectEditTarget::Track { track: 0, slot });
        assert_eq!(resolve_probe_instance(&app, &track_effect), None, "empty slot");
        app.state.pattern.effect_chains[0][slot].node_id.store(41, Ordering::Relaxed);
        assert_eq!(
            resolve_probe_instance(&app, &track_effect),
            Some(ProbeInstance::Effect { node_id: 41 })
        );
        // A recompile replaces the node; the next resolve follows it.
        app.state.pattern.effect_chains[0][slot].node_id.store(42, Ordering::Relaxed);
        assert_eq!(
            resolve_probe_instance(&app, &track_effect),
            Some(ProbeInstance::Effect { node_id: 42 })
        );
        app.state.pattern.effect_chains[0][slot].node_id.store(0, Ordering::Relaxed);

        let bus = app.buses.len();
        app.buses.push(app::BusChannelState::new(sequencer::sequencer::BusId(77), "probe bus"));
        let bus_effect = PatcherProbeTarget::Effect(EffectEditTarget::Bus { bus, slot: 1 });
        assert_eq!(resolve_probe_instance(&app, &bus_effect), None);
        app.buses[bus].effect_slots[1].node_id = 55;
        assert_eq!(
            resolve_probe_instance(&app, &bus_effect),
            Some(ProbeInstance::Effect { node_id: 55 })
        );
        assert_eq!(
            resolve_probe_instance(
                &app,
                &PatcherProbeTarget::Effect(EffectEditTarget::Bus { bus: bus + 1, slot: 1 })
            ),
            None
        );

        drop(app);
        unsafe { audiograph::destroy_live_graph(lg.0) };
    }

    #[test]
    fn watch_guard_is_held_exactly_while_a_visible_patcher_shows_probes() {
        let node_id = 77;
        let token = lisp_host::register_effect_probes(&manifest_with_probes(), 0x10).unwrap();
        lisp_host::bind_effect_probe_node(token, node_id);
        let live = Some(ProbeInstance::Effect { node_id });
        let path = "/watch/dsp.lisp".to_string();
        let mut publisher = ProbePublisher::new();
        let now = Instant::now();

        assert!(!lisp_host::probes_watched());
        assert!(!publisher.sync(&[], now), "nothing visible, nothing to do");
        assert!(publisher.sync(&[(path.clone(), live)], now), "binding published");
        assert!(publisher.watching() && lisp_host::probes_watched());
        assert_eq!(live_audio::patch_probe_instance(&path).as_deref(), Some("fx77"));
        assert!(
            live_audio::patch_probe_frame(&path, "cut", 0).is_none(),
            "nothing captured yet: the view shows —"
        );
        assert!(!publisher.sync(&[(path.clone(), live)], now), "steady pass, no frame yet");
        assert!(publisher.watching());

        // The target lost its instance (slot emptied): unbind and close the gate.
        publisher.sync(&[(path.clone(), None)], now);
        assert!(!publisher.watching() && !lisp_host::probes_watched());
        assert!(live_audio::patch_probe_instance(&path).is_none());

        // An instance without probes never opens the gate.
        publisher.sync(&[(path.clone(), Some(ProbeInstance::Effect { node_id: 5 }))], now);
        assert!(!lisp_host::probes_watched());

        // The patcher is hidden.
        publisher.sync(&[(path.clone(), live)], now);
        assert!(lisp_host::probes_watched());
        publisher.sync(&[], now);
        assert!(!lisp_host::probes_watched());
        assert!(live_audio::patch_probe_instance(&path).is_none());

        // Project load drops the gate too, and dropping the publisher does.
        publisher.sync(&[(path.clone(), live)], now);
        assert!(publisher.clear());
        assert!(!lisp_host::probes_watched());
        publisher.sync(&[(path.clone(), live)], now);
        drop(publisher);
        assert!(!lisp_host::probes_watched());
        assert!(live_audio::patch_probe_instance(&path).is_some(), "binding is cleared by the next pass");
        lisp_host::clear_effect_probes(node_id);
    }

    fn render_blocks(lg: LiveGraphPtr) {
        let mut out = vec![0.0f32; 64 * 2];
        for _ in 0..8 {
            unsafe { audiograph::process_next_block(lg.0, out.as_mut_ptr(), 64) };
        }
    }

    /// Compile `source` as an effect on the configured compiler and install
    /// it on track 0's first custom slot, like an effect patcher's preview.
    fn install_probe_effect(source: &str, name: &std::ffi::CStr) -> (app::App, LiveGraphPtr, ProbeInstance) {
        let compiled = lisp_host::compile_and_load_uncached_with_asset_base(source, 44_100, None)
            .expect("compile probe effect");
        assert!(!compiled.manifest.probes.is_empty(), "the compiler must emit probes[]");
        let (mut app, lg) = test_app(name);
        app.graph_controller().add_blank_sampler_track().expect("add track");
        let slot = sequencer::effects::BUILTIN_SLOT_COUNT;
        app.apply_compiled_effect_to_slot_sync(compiled, "probe-fx", slot, 0)
            .expect("install effect");
        let instance = resolve_probe_instance(
            &app,
            &PatcherProbeTarget::Effect(EffectEditTarget::Track { track: 0, slot }),
        )
            .expect("live effect node");
        (app, lg, instance)
    }

    /// End to end: compile an effect with a probe, install it on a track
    /// slot, render through the live graph, resolve the slot as a visible
    /// effect patcher would, and publish. Needs the pinned DGenLisp
    /// (v0.1.32+ knows `probe`) or `ESEQ_DGENLISP_TOOL`.
    #[test]
    fn compiled_effect_probe_reaches_a_published_probe_frame() {
        let (mut app, lg, instance) = install_probe_effect(
            r#"
(def in_l (in 1 @name left))
(def in_r (in 2 @name right))
(out in_l 1 @name left)
(out in_r 2 @name right)
(probe (+ 0.25 (* 0 in_l)) @id "s" @view scope)
"#,
            c"probe-publish",
        );
        let ProbeInstance::Effect { node_id } = instance else {
            panic!("an effect target resolves to an effect node");
        };
        let path = "/probe-fx/dsp.lisp".to_string();
        let bindings = [(path.clone(), Some(instance))];
        let mut publisher = ProbePublisher::new();

        // The first pass opens the gate; blocks rendered after it capture.
        publisher.sync(&bindings, Instant::now());
        assert!(lisp_host::probes_watched());
        render_blocks(lg);
        assert!(publisher.sync(&bindings, Instant::now()));

        let instance_key = live_audio::probe_instance_key_for_effect(node_id);
        assert_eq!(live_audio::patch_probe_instance(&path).as_deref(), Some(instance_key.as_str()));
        let s = live_audio::probe_frame(&live_audio::probe_frame_key(&instance_key, "s", 0))
            .expect("published under probe:fx<node>:s#0");
        assert_eq!((s.last, s.min, s.max, s.stale), (0.25, 0.25, 0.25, false));
        assert_eq!(live_audio::patch_probe_frame(&path, "s", 0), Some(s.clone()));
        let trace = s.scope.as_ref().expect("a scope view carries a trace");
        assert!(!trace.is_empty());
        assert!(trace.iter().all(|&pair| pair == (0.25, 0.25)), "{trace:?}");

        // A static signal publishes nothing more once the scope window is
        // full (until then each poll adds history, which is a change).
        let blocks_to_fill = PROBE_SCOPE_PUBLISH_PAIRS / lisp_host::PROBE_SCOPE_POINTS_PER_BLOCK;
        for _ in 0..blocks_to_fill.div_ceil(8) {
            render_blocks(lg);
        }
        publisher.sync(&bindings, Instant::now());
        render_blocks(lg);
        assert!(!publisher.sync(&bindings, Instant::now()));

        // Project load: the bulk teardown frees the effect's probes.
        drop(publisher);
        app.graph_controller().clear_all_tracks();
        assert!(lisp_host::probe_infos(instance).is_empty());
        drop(app);
        unsafe { audiograph::destroy_live_graph(lg.0) };
    }

    /// Two probes must read their own channels. Probe channels are never
    /// routed, so this rests on audiograph giving every unconnected output
    /// port a private discard buffer (eseq-d1xr.8); a shared one let the last
    /// probe written win.
    #[test]
    fn compiled_effect_probes_keep_separate_channels() {
        let (app, lg, instance) = install_probe_effect(
            r#"
(def in_l (in 1 @name left))
(def in_r (in 2 @name right))
(out in_l 1 @name left)
(out in_r 2 @name right)
(probe 0.75 @id "k")
(probe 0.25 @id "s")
"#,
            c"probe-channels",
        );
        let path = "/probe-channels/dsp.lisp".to_string();
        let bindings = [(path.clone(), Some(instance))];
        let mut publisher = ProbePublisher::new();
        publisher.sync(&bindings, Instant::now());
        render_blocks(lg);
        publisher.sync(&bindings, Instant::now());
        let k = live_audio::patch_probe_frame(&path, "k", 0).expect("k published");
        let s = live_audio::patch_probe_frame(&path, "s", 0).expect("s published");
        assert_eq!((k.last, s.last), (0.75, 0.25));
        drop(publisher);
        drop(app);
        unsafe { audiograph::destroy_live_graph(lg.0) };
    }
}
