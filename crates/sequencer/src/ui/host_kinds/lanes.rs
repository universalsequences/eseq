//! Process lanes (spec §14.2h, stage 7c): a track's process chain
//! (`process`, `t.processes`; legacy `SEQ.track-process-slots`,
//! `SEQ.process-slots`), its lanes (`lane`, `t.lanes`; legacy
//! `SEQ.track-process-lanes`, `SEQ.track-process-lane-values`,
//! `SEQ.process-lanes`), a graph node's process patch (`process`,
//! `graph-node.processes`; legacy `graph-node-process-chain`,
//! `graph-node-lane-patch`), each process's numeric inlets (`inlet`), ports and
//! their fan-out entries (`port`, `fanout`; with `process.in-ports` the
//! patchbay, legacy `SEQ.track-lane-patch`), its state cells and their
//! scope (`state-cell`, legacy `SEQ.track-process-scopes`), its latest run
//! error (`process.error`), and the library (`process-class`,
//! `process-library`; legacy `SEQ.process-library`).
//!
//! **Identity.** A process is keyed (track instance id, its stable
//! `ProcessInstanceId`), so a reorder keeps the instance (only `index`
//! moves) and a project lane is a process of every track (its lanes,
//! inlets and bindings fork per track). Lanes, inlets, ports and state
//! cells are keyed (process instance id, index) in its class's order,
//! fan-out entries (port instance id, index); a process whose class
//! changes (an expr card's body) gets fresh ones (old handles go stale).
//! Removing a process drops it; a project load drops the tracks and with
//! them every process. A graph node's processes are keyed (node instance
//! id, slot id) and dropped with their node (a node-count change, the
//! graph's instance deleted, a project load). Classes are positional, the instance kept by the
//! class id (`registry::reconcile`), replaced on a project load.
//!
//! **Feeds.** A track's processes are registered lazily, on the first read
//! of `t.processes` or `t.lanes` (the reader hook, or the tick once
//! either is observed), like a device's params; then the tick keeps that
//! track current behind its [`LaneKey`]: the track's process generation
//! (`UiInvalidationQueue::process_generation`: process chain and lane
//! edits, whole-track and project invalidations), the library's version
//! and the class instances. The global triggers (the pattern epoch: a
//! chain edit, a scene switch; the scenes revision: the project layer, an
//! undo) do not mark every track: when either moves the tick fetches the
//! project layer once and compares it with the last fetch, and each
//! registered track's own chain and project lane overrides in place with
//! those it was composed from; only a track whose inputs differ is synced.
//! None reads the history revision or the UI epoch; a tick with nothing
//! moved costs a few loads per registered track and allocates nothing.
//! A node's processes register on the first read of `n.processes`, then
//! sync with their graph (`graphs`: every override edit, a node chain edit
//! included, re-derives its graph) and when the library moved, each time
//! compared with the chain and library they were last synced from.
//! A sync composes the chain and compares it (and the overrides' forked
//! lanes) with the last synced one: unchanged, nothing is pushed; changed
//! only in some slots' lane values (a lane drag), only those processes'
//! lanes are re-derived; else every process is, each push compared with
//! its cell. The library snapshot is fetched once per library version.
//! The derivations are the legacy publishers' (`process_slot_lane_entries`,
//! `process_port_view`, `process_port_readers`, `process_scalar_inlet_view`,
//! `resolve_process_inlet_target`, `process_library_defs`). Live:
//! `process.error` and `state-cell.values` (a node's slots under their own
//! id, the node runner's runtime id), kept in `ObservedList`s and
//! re-read only when the scheduler's run errors (resp. scope histories)
//! moved or the observed set did, reading the one cell in place
//! (`SequencerState::with_process_scope_cell`).

use super::*;
use sequencer::process::{
    ProcessInstanceId, ProjectLaneOverrides, ProjectSlotOverride,
    PublishedProcessAuthoringSnapshot, PublishedProcessDef, TrackProcessChain, TrackProcessSlot,
};

/// What a registered track's processes were last synced under (beside the
/// global triggers, see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LaneKey {
    generation: u64,
    authoring: u64,
    classes: u64,
}

impl LaneKey {
    fn of(sources: &KindsHandles, lanes: &LaneShared, track: usize) -> Self {
        Self {
            generation: sources.ui_invalidations.process_generation(track),
            authoring: sources.state.published_process_authoring_version(),
            classes: lanes.classes_generation,
        }
    }
}

/// One track whose processes are registered.
pub(super) struct TrackLanes {
    /// The track position and the key of the last sync, and whether a
    /// global trigger found its inputs moved since (due regardless).
    position: usize,
    key: Option<LaneKey>,
    stale: bool,
    /// What the composed chain was built from: the project layer's serial
    /// ([`LaneShared::project`]), the track's own chain and its project
    /// lane overrides; and the composed chain.
    project: u64,
    own: TrackProcessChain,
    overrides: ProjectLaneOverrides,
    chain: TrackProcessChain,
    /// `t.processes` and `t.lanes` as of the last sync, and whether the
    /// tick has pushed them since (a cold read registers without pushing).
    processes: Vec<InstanceId>,
    lanes: Vec<InstanceId>,
    pushed: bool,
    /// The state cell instances of its processes.
    cells: Vec<InstanceId>,
}

/// One graph node whose processes are registered.
pub(super) struct NodeLanes {
    /// The library (its version and the class instances' generation) and
    /// the node's chain they were last synced from; `None` forces a sync.
    library: Option<(u64, u64)>,
    chain: TrackProcessChain,
    /// `n.processes` and their state cells as of that sync.
    processes: Vec<InstanceId>,
    cells: Vec<InstanceId>,
}

/// Whose chain a process runs in.
#[derive(Clone, Copy)]
pub(super) enum ProcessOwner {
    /// A track (its position and instance): its composed chain.
    Track(usize, InstanceId),
    /// A graph node instance: its patch, with no layers and no lanes (every
    /// inlet is a scalar), its slots running under their own ids.
    Node(InstanceId),
}

impl ProcessOwner {
    fn id(self) -> InstanceId {
        match self {
            Self::Track(_, id) | Self::Node(id) => id,
        }
    }

    /// The `track` field of its processes (and lanes).
    fn track(self) -> Value {
        match self {
            Self::Track(_, id) => Value::Instance(id),
            Self::Node(_) => Value::Nil,
        }
    }

    /// The `node` field of its processes.
    fn node(self) -> Value {
        match self {
            Self::Track(..) => Value::Nil,
            Self::Node(id) => Value::Instance(id),
        }
    }

    /// The id `slot` runs under (its run error and scope).
    fn runtime_id(self, slot: &TrackProcessSlot) -> u64 {
        match self {
            Self::Track(track, _) => {
                sequencer::process::track_process_slot_runtime_id(slot, track).0
            }
            Self::Node(_) => slot.instance_id.0,
        }
    }

    /// The name `slot` shows: its instance name, else its class (on a node,
    /// the class's node label: `transpose` for `neural-transpose`).
    fn name(self, slot: &TrackProcessSlot) -> String {
        match (&slot.instance_name, self) {
            (Some(name), _) => name.clone(),
            (None, Self::Track(..)) => slot.class_name.clone(),
            (None, Self::Node(_)) => {
                sequencer::lisp_host::graph_node_process_label(&slot.class_name)
            }
        }
    }
}

/// The process lanes' share of [`KindsShared`] (the reader hook's cold
/// reads and live values use it too).
#[derive(Default)]
pub(crate) struct LaneShared {
    /// The tracks whose processes are registered.
    pub(super) tracks: HashMap<InstanceId, TrackLanes>,
    /// The graph nodes whose processes are registered.
    pub(super) nodes: HashMap<InstanceId, NodeLanes>,
    /// Process or state cell instances changed since the live loop's id
    /// lists were built.
    relisted: bool,
    /// Each process instance's runtime id on its track (`process.error`),
    /// each state cell's runtime id and name (`state-cell.values`).
    process_runtime: HashMap<InstanceId, u64>,
    state_cells: HashMap<InstanceId, (u64, String)>,
    /// The library's classes by name and their generation (moved when the
    /// set changes).
    classes: HashMap<String, InstanceId>,
    classes_generation: u64,
    /// The published library and the version it was fetched at.
    published: Option<(u64, Rc<PublishedProcessAuthoringSnapshot>)>,
    /// The project layer as of the last fetch and its serial (moved when a
    /// fetch differs), and whether this tick fetched it.
    project: (u64, TrackProcessChain),
    project_fetched: bool,
    /// Tracks synced (their composed chain compared) and processes
    /// re-derived, for tests.
    pub(crate) syncs: u64,
    pub(crate) process_syncs: u64,
}

impl LaneShared {
    /// Forget a registered track (gone, or about to be re-recorded) with its
    /// processes' and state cells' runtime records.
    pub(super) fn forget_track(&mut self, id: InstanceId) -> Option<TrackLanes> {
        let track = self.tracks.remove(&id)?;
        self.forget_parts(&track.processes, &track.cells);
        Some(track)
    }

    /// Forget the registered graph nodes not in `live` (the graphs'
    /// nodes: a dropped node's processes went with it).
    pub(super) fn retain_nodes(&mut self, live: &[InstanceId]) {
        let gone: Vec<InstanceId> = (self.nodes.keys())
            .filter(|id| !live.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(node) = self.nodes.remove(&id) {
                self.forget_parts(&node.processes, &node.cells);
                self.relisted = true;
            }
        }
    }

    fn forget_parts(&mut self, processes: &[InstanceId], cells: &[InstanceId]) {
        for id in processes {
            self.process_runtime.remove(id);
        }
        for id in cells {
            self.state_cells.remove(id);
        }
    }

    /// Record what [`sync_processes`] derived (each process's and state
    /// cell's runtime record) in place of `previous`'s (its processes and
    /// state cells, still recorded); returns the processes, lanes and state
    /// cells. The live loop re-reads (`relisted`) when an instance list or
    /// a runtime record moved: a project lane's runtime id follows its
    /// track's position, a named slot's its name.
    fn record_parts(
        &mut self,
        derived: DerivedProcesses,
        previous: Option<(&[InstanceId], &[InstanceId])>,
    ) -> (Vec<InstanceId>, Vec<InstanceId>, Vec<InstanceId>) {
        let runtime_moved = (derived.runtime.iter())
            .any(|(id, runtime_id)| self.process_runtime.get(id) != Some(runtime_id))
            || (derived.cells.iter()).any(|(id, runtime_id, name)| {
                (self.state_cells.get(id))
                    .is_none_or(|(was_id, was_name)| was_id != runtime_id || was_name != name)
            });
        if let Some((processes, cells)) = previous {
            self.forget_parts(processes, cells);
        }
        self.relisted |= runtime_moved;
        self.process_runtime.extend(derived.runtime);
        let mut cells = Vec::with_capacity(derived.cells.len());
        for (id, runtime_id, name) in derived.cells {
            cells.push(id);
            self.state_cells.insert(id, (runtime_id, name));
        }
        self.relisted |= previous.is_none_or(|(processes, previous_cells)| {
            processes != derived.processes.as_slice() || previous_cells != cells.as_slice()
        });
        (derived.processes, derived.lanes, cells)
    }

    /// The published library, fetched again only when its version moved.
    fn published(&mut self, state: &SequencerState) -> Rc<PublishedProcessAuthoringSnapshot> {
        let version = state.published_process_authoring_version();
        if let Some((fetched, published)) = &self.published {
            if *fetched == version {
                return published.clone();
            }
        }
        let published = Rc::new(state.published_process_authoring());
        self.published = Some((version, published.clone()));
        published
    }

    /// Fetch the project layer (once per tick unless `again`); its serial
    /// moves when it differs from the last fetch.
    fn fetch_project(&mut self, state: &SequencerState, again: bool) -> u64 {
        if again || !self.project_fetched {
            self.project_fetched = true;
            let project = state.project_process_chain();
            if project != self.project.1 {
                self.project = (self.project.0 + 1, project);
            }
        }
        self.project.0
    }
}

/// The process syncs' state (in [`HostKinds`]).
#[derive(Default)]
pub(crate) struct LaneState {
    /// Class id → instance.
    class_ids: HashMap<u64, InstanceId>,
    classes: Vec<Option<InstanceId>>,
    /// The library version of the last class sync.
    authoring: Option<u64>,
    /// The global triggers (pattern epoch, scenes revision) last checked.
    global: Option<(u64, u64)>,
    /// The tracks observed for `processes` or `lanes` (registers them).
    track_observed: ObservedList,
    /// Every process and state cell instance, for the live loop.
    process_ids: Vec<InstanceId>,
    cell_ids: Vec<InstanceId>,
    process_observed: ObservedList,
    cell_observed: ObservedList,
    /// (run error version, observer epoch) of the last `process.error`
    /// push, (scope version, observer epoch) of the last
    /// `state-cell.values` push; `None` forces one.
    process_live: Option<(u64, u64)>,
    cell_live: Option<(u64, u64)>,
    /// The library (version, class instances' generation) the registered
    /// graph nodes were last checked against; `None` forces a check.
    node_library: Option<(u64, u64)>,
}

impl LaneState {
    /// Sync everything at the next tick (a schema change, a hot reload).
    pub(super) fn invalidate(&mut self, shared: &RefCell<KindsShared>) {
        self.authoring = None;
        self.global = None;
        self.process_live = None;
        self.cell_live = None;
        self.node_library = None;
        let lanes = &mut shared.borrow_mut().lanes;
        for track in lanes.tracks.values_mut() {
            track.key = None;
            track.pushed = false;
        }
        for node in lanes.nodes.values_mut() {
            node.library = None;
        }
    }

    /// Instances whose loss (a hot reload) means a full sync.
    pub(super) fn representatives(&self) -> impl Iterator<Item = &InstanceId> {
        (self.classes.iter().flatten()).chain(self.process_ids.first())
    }

    /// Drop the classes (a project load).
    pub(super) fn drain(&mut self) -> impl Iterator<Item = (u64, InstanceId)> + '_ {
        self.authoring = None;
        self.class_ids.drain()
    }
}

fn flag(on: bool) -> Value {
    Value::Bool(on)
}

/// Pushes during one track's sync, each compared with its cell
/// ([`put`]); `changed` says whether any landed.
struct Puts<'a, S> {
    store: &'a mut S,
    shared: &'a RefCell<KindsShared>,
    changed: bool,
}

impl<S: KindStore> Puts<'_, S> {
    fn put(&mut self, id: InstanceId, key: FieldKey, value: Value) {
        self.changed |= put(self.store, self.shared, id, key, value);
    }
}

/// Whether two slots agree on everything but their lanes' values (their
/// lane names included): all the rest of a track's processes derive from.
fn same_but_lane_values(a: &TrackProcessSlot, b: &TrackProcessSlot) -> bool {
    let TrackProcessSlot {
        instance_id,
        instance_name,
        class_name,
        enabled,
        project_layer,
        inlets,
        lanes,
        bindings,
        fanout,
        unbound_ports,
        expr_source,
    } = a;
    *instance_id == b.instance_id
        && *instance_name == b.instance_name
        && *class_name == b.class_name
        && *enabled == b.enabled
        && *project_layer == b.project_layer
        && *inlets == b.inlets
        && lanes.keys().eq(b.lanes.keys())
        && *bindings == b.bindings
        && *fanout == b.fanout
        && *unbound_ports == b.unbound_ports
        && *expr_source == b.expr_source
}

/// Whether two override sets fork the same lanes (`lane.forked`).
fn same_forked(a: &ProjectLaneOverrides, b: &ProjectLaneOverrides) -> bool {
    fn forked(
        overrides: &ProjectLaneOverrides,
    ) -> impl Iterator<Item = (&ProcessInstanceId, &ProjectSlotOverride)> {
        overrides.iter().filter(|(_, own)| !own.lanes.is_empty())
    }
    forked(a).count() == forked(b).count()
        && (forked(a).zip(forked(b)))
            .all(|((a, a_own), (b, b_own))| a == b && a_own.lanes.keys().eq(b_own.lanes.keys()))
}

/// The track's processes as its chain says now: registers the missing
/// ones (and their lanes, inlets, ports, fan-out entries and state cells),
/// drops the gone ones, pushes what changed, and records the track in
/// [`LaneShared::tracks`]. With `force` false a chain, forked lanes,
/// position and library unchanged since the last sync pushes nothing, and
/// one changed only in lane values re-derives those processes' lanes
/// alone. Returns whether anything was registered, dropped or pushed;
/// `None` when the track is gone.
pub(super) fn sync_track_lanes<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
    track_id: InstanceId,
    force: bool,
) -> Option<bool> {
    let state = &sources.state;
    let own = state.track_process_chain(track)?;
    let overrides = state.project_lane_overrides(track);
    let (key, project, published, chain) = {
        let mut guard = shared.borrow_mut();
        let lanes = &mut guard.lanes;
        let project = lanes.fetch_project(state, force);
        let mut layer = lanes.project.1.clone();
        sequencer::process::apply_project_lane_overrides(&mut layer, &overrides);
        let chain = sequencer::process::compose_effective_process_chain(&layer, &own);
        let key = LaneKey::of(sources, lanes, track);
        let published = lanes.published(state);
        lanes.syncs += 1;
        if let Some(synced) = lanes.tracks.get_mut(&track_id) {
            let same_inputs = synced.key.is_some_and(|synced| {
                synced.authoring == key.authoring && synced.classes == key.classes
            });
            let comparable = !force
                && same_inputs
                && synced.position == track
                && same_forked(&synced.overrides, &overrides);
            synced.stale = false;
            if comparable && synced.chain == chain {
                synced.key = Some(key);
                synced.project = project;
                synced.own = own;
                synced.overrides = overrides;
                return Some(false);
            }
            let lane_values_only = comparable
                && synced.chain.slots.len() == chain.slots.len()
                && (synced.chain.slots.iter().zip(&chain.slots))
                    .all(|(was, now)| same_but_lane_values(was, now));
            if lane_values_only {
                let was = std::mem::replace(&mut synced.chain, chain);
                synced.key = Some(key);
                synced.project = project;
                synced.own = own;
                synced.overrides = overrides;
                drop(guard);
                return Some(resync_lane_values(
                    store, sources, shared, track, track_id, &was,
                ));
            }
        }
        (key, project, published, chain)
    };
    let mut puts = Puts {
        store,
        shared,
        changed: false,
    };
    let owner = ProcessOwner::Track(track, track_id);
    let derived = sync_processes(&mut puts, sources, owner, &chain, &published);
    let changed = puts.changed;

    let mut shared = shared.borrow_mut();
    let lanes_shared = &mut shared.lanes;
    lanes_shared.process_syncs += chain.slots.len() as u64;
    let previous = lanes_shared.tracks.remove(&track_id);
    let listed = (previous.as_ref()).map(|was| (was.processes.as_slice(), was.cells.as_slice()));
    let (processes, lanes, cells) = lanes_shared.record_parts(derived, listed);
    let pushed = previous.is_some_and(|previous| {
        previous.pushed && previous.processes == processes && previous.lanes == lanes
    });
    lanes_shared.tracks.insert(
        track_id,
        TrackLanes {
            position: track,
            key: Some(key),
            stale: false,
            project,
            own,
            overrides,
            chain,
            processes,
            lanes,
            pushed,
            cells,
        },
    );
    Some(changed)
}

/// The lane values of the slots of `track`'s synced chain that differ from
/// `was` (the rest of the chain agrees with it): those processes' lanes
/// re-derived and pushed. Returns whether anything was pushed.
fn resync_lane_values<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track: usize,
    track_id: InstanceId,
    was: &TrackProcessChain,
) -> bool {
    let state = &sources.state;
    let published = shared.borrow_mut().lanes.published(state);
    let dirty: Vec<(usize, TrackProcessSlot)> = {
        let shared = shared.borrow();
        let synced = &shared.lanes.tracks[&track_id];
        (synced.chain.slots.iter().zip(&was.slots).enumerate())
            .filter(|(_, (now, was))| now != was)
            .map(|(index, (now, _))| (index, now.clone()))
            .collect()
    };
    shared.borrow_mut().lanes.process_syncs += dirty.len() as u64;
    let mut puts = Puts {
        store,
        shared,
        changed: false,
    };
    for (slot_index, slot) in &dirty {
        let Some(id) = puts.store.keyed(PROCESS, &[track_id, slot.instance_id.0]) else {
            continue;
        };
        let def = process_slot_def(&published, slot);
        let mut entries = Vec::new();
        process_slot_lane_entries(state, track, *slot_index, slot, def, &mut entries);
        for (index, entry) in entries.iter().enumerate() {
            if let Some(lane) = puts.store.keyed(LANE, &[id, index as u64]) {
                put_lane(&mut puts, lane, entry, None);
            }
        }
    }
    puts.changed
}

/// What [`sync_processes`] registered: `t.processes`, `t.lanes`, each
/// process's runtime id and each state cell's (cell, runtime id, name).
struct DerivedProcesses {
    processes: Vec<InstanceId>,
    lanes: Vec<InstanceId>,
    runtime: Vec<(InstanceId, u64)>,
    cells: Vec<(InstanceId, u64, String)>,
}

/// Register and push every process of `owner`'s `chain` (a track's
/// composed chain, a node's patch).
fn sync_processes<S: KindStore>(
    puts: &mut Puts<'_, S>,
    sources: &KindsHandles,
    owner: ProcessOwner,
    chain: &TrackProcessChain,
    published: &PublishedProcessAuthoringSnapshot,
) -> DerivedProcesses {
    let state = &sources.state;
    // Processes, by their stable id; a repeated id gets no second instance.
    let mut seen = HashSet::new();
    let kept: Vec<usize> = (chain.slots.iter().enumerate())
        .filter(|(_, slot)| seen.insert(slot.instance_id.0))
        .map(|(index, _)| index)
        .collect();
    let wanted: Vec<u64> = kept
        .iter()
        .map(|&index| chain.slots[index].instance_id.0)
        .collect();
    let (ids, registered) = reconcile_children(
        puts.store,
        owner.id(),
        PROCESS,
        &wanted,
        |store, id, proc_id| {
            store.push(id, f::PROCESS_TRACK, owner.track());
            store.push(id, f::PROCESS_NODE, owner.node());
            store.push(id, f::PROCESS_PROC_ID, number(proc_id as f64));
        },
    );
    puts.changed |= registered;
    // The process instance of each chain position (wiring targets).
    let mut at_slot: Vec<Option<InstanceId>> = vec![None; chain.slots.len()];
    for (&index, id) in kept.iter().zip(&ids) {
        at_slot[index] = *id;
    }
    // The inlets a cable writes (in ports beside the lanes and gates).
    let mut wired: HashSet<(usize, &str)> = HashSet::new();
    for slot in &chain.slots {
        let Some(def) = process_slot_def(published, slot) else {
            continue;
        };
        for port in def.ports.iter().filter(|port| port.is_connectable()) {
            let readers = process_port_readers(chain, slot, &port.name);
            wired.extend(readers.into_iter().map(|(_, index, inlet)| (index, inlet)));
        }
    }
    let entries = match owner {
        ProcessOwner::Track(track, _) => {
            process_lane_entries_for_chain(state, track, chain, published)
        }
        ProcessOwner::Node(_) => Vec::new(),
    };
    let shared = puts.shared;
    let lane_shared = shared.borrow();
    let classes = &lane_shared.lanes.classes;
    let mut derived = DerivedProcesses {
        processes: Vec::new(),
        lanes: Vec::new(),
        runtime: Vec::new(),
        cells: Vec::new(),
    };
    for (&slot_index, id) in kept.iter().zip(&ids) {
        let Some(id) = *id else { continue };
        let slot = &chain.slots[slot_index];
        let def = process_slot_def(published, slot);
        // A new class (an expr card's body) gets fresh children.
        let class_was = puts.store.field(id, f::PROCESS_CLASS_NAME.1);
        if class_was.is_some_and(|was| was != text("") && was != text(&slot.class_name)) {
            for kind in [LANE, INLET, PORT, STATE_CELL] {
                puts.changed |= drop_children_past(puts.store, id, kind, 0);
            }
        }
        derived.processes.push(id);
        let class = classes.get(&slot.class_name).copied();
        put_process(puts, id, owner, (slot_index, slot), def, class);
        let in_ports = def.map_or_else(Vec::new, |def| {
            (def.inlets.iter())
                .filter(|inlet| {
                    lane_patch_in_port(inlet, wired.contains(&(slot_index, inlet.name.as_str())))
                })
                .map(|inlet| &inlet.name)
                .collect()
        });
        puts.put(id, f::PROCESS_IN_PORTS, strings(in_ports));

        // Lanes: this slot's entries, in class order, at their positions in
        // `t.lanes`.
        let first = derived.lanes.len();
        let slot_entries: Vec<&ProcessLaneUiEntry> = (entries.iter())
            .filter(|entry| entry.slot_index == slot_index)
            .collect();
        let lane_ids = sync_lanes(puts, id, owner, &slot_entries, first);
        derived.lanes.extend(lane_ids.iter().copied());
        puts.put(id, f::PROCESS_LANES, instance_list(lane_ids));
        let node = matches!(owner, ProcessOwner::Node(_));
        let inlet_ids = sync_inlets(puts, id, slot, def, node);
        puts.put(id, f::PROCESS_INLETS, instance_list(inlet_ids));
        let target_process = |target: &sequencer::process::ParamTarget| {
            resolve_process_inlet_target(chain, slot, target)
                .and_then(|(index, _)| at_slot.get(index).copied().flatten())
        };
        let port_ids = sync_ports(puts, id, slot, def, &target_process);
        puts.put(id, f::PROCESS_PORTS, instance_list(port_ids));

        // State cells (the scope), under the slot's runtime id.
        let runtime_id = owner.runtime_id(slot);
        derived.runtime.push((id, runtime_id));
        let cells = sync_cells(puts, id, def);
        puts.put(
            id,
            f::PROCESS_CELLS,
            instance_list(cells.iter().map(|(id, _)| *id)),
        );
        let cells = cells
            .into_iter()
            .map(|(cell, name)| (cell, runtime_id, name));
        derived.cells.extend(cells);
    }
    derived
}

/// A process's own fields (`slot` at `slot_index` in its chain).
fn put_process<S: KindStore>(
    puts: &mut Puts<'_, S>,
    id: InstanceId,
    owner: ProcessOwner,
    (slot_index, slot): (usize, &TrackProcessSlot),
    def: Option<&PublishedProcessDef>,
    class: Option<InstanceId>,
) {
    puts.put(id, f::PROCESS_INDEX, number(slot_index as f64));
    puts.put(id, f::PROCESS_CLASS_REF, instance_or_nil(class));
    puts.put(id, f::PROCESS_CLASS_NAME, text(&slot.class_name));
    let instance_name = slot.instance_name.as_deref().unwrap_or("");
    puts.put(id, f::PROCESS_NAME, Value::String(owner.name(slot)));
    puts.put(id, f::PROCESS_INSTANCE_NAME, text(instance_name));
    puts.put(id, f::PROCESS_PROJECT, flag(slot.project_layer));
    let default_lane = sequencer::process::is_default_lane_slot(slot);
    puts.put(id, f::PROCESS_DEFAULT_LANE, flag(default_lane));
    let roster = sequencer::process::is_track_roster_slot(slot);
    puts.put(id, f::PROCESS_ROSTER, flag(roster));
    puts.put(id, f::PROCESS_ENABLED, flag(slot.enabled));
    let doc = def.and_then(|def| def.doc.as_deref()).unwrap_or("");
    puts.put(id, f::PROCESS_DOC, text(doc));
    let source = def.and_then(|def| def.source_path.as_deref()).unwrap_or("");
    puts.put(id, f::PROCESS_SOURCE_PATH, text(source));
    let target = def
        .map(|def| process_ports_label(&def.ports))
        .unwrap_or_default();
    puts.put(id, f::PROCESS_TARGET, Value::String(target));
    puts.put(id, f::PROCESS_EXPR, flag(slot.is_expr_card()));
    let line = slot.expr_preview_line().unwrap_or_default();
    puts.put(id, f::PROCESS_EXPR_LINE, Value::String(line));
    let error = slot.expr_compile_error(def.is_some()).unwrap_or_default();
    puts.put(id, f::PROCESS_COMPILE_ERROR, Value::String(error));
    puts.put(id, f::PROCESS_KNOWN, flag(def.is_some()));
    let source = slot.expr_source.as_deref().unwrap_or("");
    puts.put(id, f::PROCESS_EXPR_SOURCE, text(source));
    let (promoted, reason) = sequencer::lisp_host::process_slot_as_expr(slot, def);
    puts.put(id, f::PROCESS_PROMOTED_EXPR, flag(promoted));
    let reason = reason.unwrap_or_default();
    puts.put(id, f::PROCESS_AS_EXPR_REASON, Value::String(reason));
}

/// A process's lanes, one per entry, the first at `first` in `t.lanes`
/// (none on a node).
fn sync_lanes<S: KindStore>(
    puts: &mut Puts<'_, S>,
    id: InstanceId,
    owner: ProcessOwner,
    entries: &[&ProcessLaneUiEntry],
    first: usize,
) -> Vec<InstanceId> {
    let lane_ids = indexed_children(puts.store, id, LANE, entries.len(), |store, lane, at| {
        store.push(lane, f::LANE_PROCESS, Value::Instance(id));
        store.push(lane, f::LANE_TRACK, owner.track());
        store.push(lane, f::LANE_INDEX, number(at as f64));
    });
    for (offset, (lane, entry)) in lane_ids.iter().zip(entries).enumerate() {
        put_lane(puts, *lane, entry, Some(first + offset));
    }
    lane_ids
}

/// One lane's fields (its `position` in `t.lanes` when given).
fn put_lane<S: KindStore>(
    puts: &mut Puts<'_, S>,
    lane: InstanceId,
    entry: &ProcessLaneUiEntry,
    position: Option<usize>,
) {
    if let Some(position) = position {
        puts.put(lane, f::LANE_POSITION, number(position as f64));
    }
    puts.put(lane, f::LANE_INLET, text(&entry.inlet_name));
    puts.put(lane, f::LANE_LABEL, text(&entry.label));
    puts.put(lane, f::LANE_SHORT_LABEL, text(&entry.short_label));
    puts.put(lane, f::LANE_TYPE, text(&entry.kind));
    puts.put(lane, f::LANE_MIN, number(entry.min));
    puts.put(lane, f::LANE_MAX, number(entry.max));
    puts.put(lane, f::LANE_DEFAULT, number(entry.default));
    puts.put(lane, f::LANE_DECIMALS, number(entry.decimals));
    puts.put(lane, f::LANE_FORKED, flag(entry.forked));
    puts.put(lane, f::LANE_VALUES, numbers(&entry.values));
}

/// A process's numeric inlets (with its lane inlets on a node's: `lanes`).
fn sync_inlets<S: KindStore>(
    puts: &mut Puts<'_, S>,
    id: InstanceId,
    slot: &TrackProcessSlot,
    def: Option<&PublishedProcessDef>,
    lanes: bool,
) -> Vec<InstanceId> {
    let views: Vec<(String, ProcessInletView)> = process_inlet_names(slot, def, lanes)
        .into_iter()
        .filter_map(|name| {
            let view = process_scalar_inlet_view(slot, def, &name, process_inlet_def(def, &name))?;
            Some((name, view))
        })
        .collect();
    let inlet_ids = indexed_children(puts.store, id, INLET, views.len(), |store, inlet, at| {
        store.push(inlet, f::INLET_PROCESS, Value::Instance(id));
        store.push(inlet, f::INLET_INDEX, number(at as f64));
    });
    for (&inlet, (name, view)) in inlet_ids.iter().zip(&views) {
        puts.put(inlet, f::INLET_NAME, text(name));
        puts.put(inlet, f::INLET_TYPE, text(view.kind));
        puts.put(inlet, f::INLET_OPTIONS, strings(&view.options));
        puts.put(inlet, f::INLET_VALUE, number(view.value));
        puts.put(inlet, f::INLET_DEFAULT, number(view.default));
        puts.put(inlet, f::INLET_MIN, number(view.min));
        puts.put(inlet, f::INLET_MAX, number(view.max));
        puts.put(inlet, f::INLET_DECIMALS, number(view.decimals));
        puts.put(inlet, f::INLET_DOC, text(&view.doc));
    }
    inlet_ids
}

/// A process's ports and their fan-out entries; `target_process` resolves
/// a target to the process it wires into.
fn sync_ports<S: KindStore>(
    puts: &mut Puts<'_, S>,
    id: InstanceId,
    slot: &TrackProcessSlot,
    def: Option<&PublishedProcessDef>,
    target_process: &dyn Fn(&sequencer::process::ParamTarget) -> Option<InstanceId>,
) -> Vec<InstanceId> {
    let port_defs = process_slot_port_defs(slot, def);
    let port_ids = indexed_children(puts.store, id, PORT, port_defs.len(), |store, port, at| {
        store.push(port, f::PORT_PROCESS, Value::Instance(id));
        store.push(port, f::PORT_INDEX, number(at as f64));
    });
    for (&port, port_def) in port_ids.iter().zip(&port_defs) {
        let view = process_port_view(slot, port_def);
        puts.put(port, f::PORT_NAME, text(&port_def.name));
        let label = process_port_label(&port_def.name);
        puts.put(port, f::PORT_LABEL, text(label));
        puts.put(port, f::PORT_HINT, text(&view.hint));
        puts.put(port, f::PORT_TARGET, text(&view.target_label));
        puts.put(port, f::PORT_STATUS, text(view.status));
        puts.put(port, f::PORT_MANUAL, flag(view.manual));
        puts.put(port, f::PORT_DISCONNECTED, flag(view.disconnected));
        puts.put(port, f::PORT_MAPPABLE, flag(port_def.is_mappable()));
        puts.put(port, f::PORT_CONNECTABLE, flag(port_def.is_connectable()));
        puts.put(port, f::PORT_BINDABLE, flag(view.bindable));
        puts.put(port, f::PORT_TARGET_KIND, text(&view.target_kind));
        let binding = view.binding;
        let wired_to = binding.and_then(target_process);
        puts.put(port, f::PORT_TARGET_PROCESS, instance_or_nil(wired_to));
        let inlet = binding.and_then(param_target_inlet).unwrap_or("");
        puts.put(port, f::PORT_TARGET_INLET, text(inlet));
        let step_param = binding.and_then(param_target_step_param).unwrap_or("");
        puts.put(port, f::PORT_TARGET_STEP_PARAM, text(step_param));
        let fanout_ids = indexed_children(
            puts.store,
            port,
            FANOUT,
            view.fanout.len(),
            |store, entry, at| {
                store.push(entry, f::FANOUT_PORT, Value::Instance(port));
                store.push(entry, f::FANOUT_INDEX, number(at as f64));
            },
        );
        for (&entry, fanout) in fanout_ids.iter().zip(view.fanout) {
            let label = process_param_target_label(&fanout.target);
            puts.put(entry, f::FANOUT_TARGET, Value::String(label));
            let wired_to = target_process(&fanout.target);
            puts.put(entry, f::FANOUT_TARGET_PROCESS, instance_or_nil(wired_to));
            let inlet = param_target_inlet(&fanout.target).unwrap_or("");
            puts.put(entry, f::FANOUT_TARGET_INLET, text(inlet));
            let step_param = param_target_step_param(&fanout.target).unwrap_or("");
            puts.put(entry, f::FANOUT_TARGET_STEP_PARAM, text(step_param));
            puts.put(entry, f::FANOUT_LO, number(fanout.lo));
            puts.put(entry, f::FANOUT_HI, number(fanout.hi));
        }
        puts.put(port, f::PORT_FANOUT, instance_list(fanout_ids));
    }
    port_ids
}

/// A process's state cells (its class's), with their names.
fn sync_cells<S: KindStore>(
    puts: &mut Puts<'_, S>,
    id: InstanceId,
    def: Option<&PublishedProcessDef>,
) -> Vec<(InstanceId, String)> {
    let names: Vec<&String> = def.map_or_else(Vec::new, |def| {
        def.state.iter().map(|cell| &cell.name).collect()
    });
    let cell_ids = indexed_children(
        puts.store,
        id,
        STATE_CELL,
        names.len(),
        |store, cell, at| {
            store.push(cell, f::STATE_CELL_PROCESS, Value::Instance(id));
            store.push(cell, f::STATE_CELL_INDEX, number(at as f64));
        },
    );
    (cell_ids.into_iter().zip(names))
        .map(|(cell, name)| {
            puts.put(cell, f::STATE_CELL_NAME, text(name));
            (cell, name.clone())
        })
        .collect()
}

/// A cold read of `t.processes` or `t.lanes` (the reader hook): registers
/// the track's processes the first time; afterwards the model field
/// answers (`None`).
pub(super) fn cold_track_lanes<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    track_id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    let listed = |shared: &KindsShared| {
        let synced = shared.lanes.tracks.get(&track_id)?;
        Some(instance_list(match key {
            f::TRACK_LANES => synced.lanes.iter().copied(),
            _ => synced.processes.iter().copied(),
        }))
    };
    {
        let shared = shared.borrow();
        if shared.skip.contains(&key) {
            return None;
        }
        if let Some(synced) = shared.lanes.tracks.get(&track_id) {
            // Registered: the cell answers once the tick pushed it.
            return if synced.pushed { None } else { listed(&shared) };
        }
    }
    let track = *store.key_of(track_id)?.first()? as usize;
    if !sources.track_exists(track) {
        return None;
    }
    sync_track_lanes(store, sources, shared, track, track_id, true)?;
    let mut shared = shared.borrow_mut();
    shared.count(key);
    listed(&shared)
}

/// Graph node `node`'s processes as `chain` (its patch; `None`: as last
/// synced, for a library change) says: registers the missing ones and their
/// parts, drops the gone ones, pushes what changed and records the node in
/// [`LaneShared::nodes`]. Returns `n.processes` and whether anything was
/// registered, dropped or pushed; `None` when the chain and the library are
/// those of the last sync (or the node was never synced and no chain is
/// given).
pub(super) fn sync_node_lanes<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    node: InstanceId,
    chain: Option<&TrackProcessChain>,
) -> Option<(Value, bool)> {
    let state = &sources.state;
    let library = |lanes: &LaneShared| {
        let version = state.published_process_authoring_version();
        (version, lanes.classes_generation)
    };
    let chain = {
        let lanes = &shared.borrow().lanes;
        let synced = lanes.nodes.get(&node);
        let current = synced.is_some_and(|synced| synced.library == Some(library(lanes)));
        match (chain, synced) {
            (Some(chain), Some(synced)) if current && synced.chain == *chain => return None,
            (Some(chain), _) => chain.clone(),
            (None, Some(_)) if current => return None,
            (None, Some(synced)) => synced.chain.clone(),
            (None, None) => return None,
        }
    };
    // A chain that arrived before its expr bodies were compiled (a project
    // load, a copy) compiles them now, as the legacy read does (which can
    // move the library: read after).
    sequencer::lisp_host::ensure_graph_node_expr_classes(state, &chain);
    let (library, published) = {
        let lanes = &mut shared.borrow_mut().lanes;
        (library(lanes), lanes.published(state))
    };
    let mut puts = Puts {
        store,
        shared,
        changed: false,
    };
    let derived = sync_processes(
        &mut puts,
        sources,
        ProcessOwner::Node(node),
        &chain,
        &published,
    );
    let changed = puts.changed;
    let mut shared = shared.borrow_mut();
    let lanes = &mut shared.lanes;
    lanes.process_syncs += chain.slots.len() as u64;
    let previous = lanes.nodes.remove(&node);
    let listed = (previous.as_ref()).map(|was| (was.processes.as_slice(), was.cells.as_slice()));
    let (processes, _, cells) = lanes.record_parts(derived, listed);
    let list = instance_list(processes.iter().copied());
    let record = NodeLanes {
        library: Some(library),
        chain,
        processes,
        cells,
    };
    lanes.nodes.insert(node, record);
    Some((list, changed))
}

/// One live field of a process (`error`) or a state cell (`values`).
pub(super) fn lane_live_value(
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    let state = &sources.state;
    let shared = shared.borrow();
    match key {
        f::PROCESS_ERROR => {
            let runtime_id = *shared.lanes.process_runtime.get(&id)?;
            Some(Value::String(
                state.process_run_error(runtime_id).unwrap_or_default(),
            ))
        }
        f::STATE_CELL_VALUES => {
            let (runtime_id, name) = shared.lanes.state_cells.get(&id)?;
            Some(state.with_process_scope_cell(*runtime_id, name, |values| {
                numbers(values.unwrap_or(&[]))
            }))
        }
        _ => None,
    }
}

/// The library's class ids: each class's id when they are distinct, else
/// its name's stable hash.
fn class_ids(defs: &[&PublishedProcessDef]) -> Vec<u64> {
    let ids: Vec<u64> = defs.iter().map(|def| def.id).collect();
    if distinct(&ids) {
        return ids;
    }
    defs.iter()
        .map(|def| sequencer::process::stable_process_id(&def.name))
        .collect()
}

impl HostKinds {
    /// The process syncs (see the module docs): the library, the global
    /// triggers, the tracks registered or newly observed, and the
    /// registered graph nodes the library moved under (their chains sync
    /// with their graph, `sync_graph_model`).
    pub(super) fn sync_lane_model(&mut self, pusher: &mut Pusher<'_>) {
        if (self.lanes.representatives()).any(|id| !pusher.rt.instance_is_live(*id)) {
            // A hot reload dropped process instances.
            self.lanes.invalidate(pusher.shared);
        }
        let shared = pusher.shared;
        let state = &pusher.sources.state;
        shared.borrow_mut().lanes.project_fetched = false;
        let authoring = state.published_process_authoring_version();
        if self.lanes.authoring != Some(authoring) {
            self.lanes.authoring = Some(authoring);
            let published = shared.borrow_mut().lanes.published(state);
            self.sync_process_classes(pusher, &published);
        }
        let tracks = &self.track_ids;
        let names = [f::TRACK_PROCESSES.1, f::TRACK_LANES.1];
        (self.lanes.track_observed).refresh(pusher.rt, &names, || {
            tracks.iter().flatten().copied().collect()
        });
        {
            let lanes = &mut shared.borrow_mut().lanes;
            let gone: Vec<InstanceId> = (lanes.tracks.keys())
                .filter(|id| !pusher.rt.instance_is_live(**id))
                .copied()
                .collect();
            for id in gone {
                lanes.forget_track(id);
                lanes.relisted = true;
            }
        }
        self.check_lane_globals(pusher);
        for (position, id) in self.track_ids.iter().enumerate() {
            let Some(id) = *id else { continue };
            let due = {
                let shared = shared.borrow();
                match shared.lanes.tracks.get(&id) {
                    Some(track) => {
                        let key = LaneKey::of(pusher.sources, &shared.lanes, position);
                        track.position != position
                            || track.key != Some(key)
                            || track.stale
                            || !track.pushed
                    }
                    None => (self.lanes.track_observed.entries.iter()).any(|entry| entry.0 == id),
                }
            };
            if !due || !pusher.sources.track_exists(position) {
                continue;
            }
            let Some(changed) =
                sync_track_lanes(&mut *pusher.rt, pusher.sources, shared, position, id, false)
            else {
                continue;
            };
            pusher.changed |= changed;
            let (processes, lanes) = {
                let mut shared = shared.borrow_mut();
                let track = shared.lanes.tracks.get_mut(&id).expect("just synced");
                if track.pushed {
                    continue;
                }
                track.pushed = true;
                (
                    instance_list(track.processes.iter().copied()),
                    instance_list(track.lanes.iter().copied()),
                )
            };
            pusher.push(id, f::TRACK_PROCESSES, processes);
            pusher.push(id, f::TRACK_LANES, lanes);
        }
        // Registered nodes sync with their graph; here only when the library
        // moved (each from the chain it last synced).
        let library = (authoring, shared.borrow().lanes.classes_generation);
        if self.lanes.node_library == Some(library) {
            return;
        }
        self.lanes.node_library = Some(library);
        let nodes: Vec<InstanceId> = shared.borrow().lanes.nodes.keys().copied().collect();
        for node in nodes {
            let synced = sync_node_lanes(&mut *pusher.rt, pusher.sources, shared, node, None);
            if let Some((processes, changed)) = synced {
                pusher.changed |= changed;
                pusher.push(node, f::GRAPH_NODE_PROCESSES, processes);
            }
        }
    }

    /// When a global trigger (the pattern epoch, the scenes revision)
    /// moved: fetch the project layer once and mark due the registered
    /// tracks whose composed chain may have moved (the project layer did,
    /// or their own chain or project lane overrides did).
    fn check_lane_globals(&mut self, pusher: &Pusher<'_>) {
        let state = &pusher.sources.state;
        let global = (
            state.transport.pattern_epoch.load(Ordering::Relaxed),
            state.project_scenes_revision(),
        );
        if self.lanes.global == Some(global) {
            return;
        }
        self.lanes.global = Some(global);
        let mut shared = pusher.shared.borrow_mut();
        let lanes = &mut shared.lanes;
        if lanes.tracks.is_empty() {
            return;
        }
        let project = lanes.fetch_project(state, false);
        for track in lanes.tracks.values_mut() {
            let moved = track.project != project
                || !state.track_process_chain_is(track.position, &track.own)
                || !state.project_lane_overrides_are(track.position, &track.overrides);
            track.stale |= moved;
        }
    }

    /// The library's classes (`process-library.classes`), when its version
    /// moved; a changed class set moves `classes_generation`, so every
    /// registered track re-resolves its processes' classes.
    fn sync_process_classes(
        &mut self,
        pusher: &mut Pusher<'_>,
        published: &PublishedProcessAuthoringSnapshot,
    ) {
        let defs: Vec<&PublishedProcessDef> = process_library_defs(published).collect();
        let ids = reconcile(
            pusher,
            PROCESS_CLASS,
            &mut self.lanes.class_ids,
            &class_ids(&defs),
        );
        let mut by_name = HashMap::new();
        for (index, (id, def)) in ids.iter().zip(&defs).enumerate() {
            let Some(id) = *id else { continue };
            by_name.insert(def.name.clone(), id);
            pusher.push(id, f::CLASS_INDEX, number(index as f64));
            pusher.push(id, f::CLASS_NAME, text(&def.name));
            let doc = def.doc.clone().unwrap_or_default();
            pusher.push(id, f::CLASS_DOC, Value::String(doc));
            let source = def.source_path.clone().unwrap_or_default();
            pusher.push(id, f::CLASS_SOURCE_PATH, Value::String(source));
            let target = process_ports_label(&def.ports);
            pusher.push(id, f::CLASS_TARGET, Value::String(target));
            let lanes = def.inlets.iter().filter(|inlet| inlet.lane).count();
            pusher.push(id, f::CLASS_LANE_COUNT, number(lanes as f64));
            let ports = def.ports.iter().map(|port| &port.name);
            pusher.push(id, f::CLASS_PORTS, strings(ports));
        }
        if let Some(library) = pusher.singleton(PROCESS_LIBRARY) {
            let classes = listed_instances(&ids);
            pusher.push(library, f::LIBRARY_CLASSES, classes);
        }
        self.lanes.classes = ids;
        let mut shared = pusher.shared.borrow_mut();
        if shared.lanes.classes != by_name {
            shared.lanes.classes = by_name;
            shared.lanes.classes_generation += 1;
        }
    }

    /// `process.error` of the observed processes when the scheduler's run
    /// errors moved, `state-cell.values` of the observed cells when its
    /// scope histories did, or either observed set did. The id lists are
    /// rebuilt when a track's or a node's instances changed.
    pub(super) fn sync_lane_live(&mut self, pusher: &mut Pusher<'_>) {
        let lanes = &mut self.lanes;
        let mut shared = pusher.shared.borrow_mut();
        if std::mem::take(&mut shared.lanes.relisted) {
            let tracks = shared.lanes.tracks.values();
            let nodes = shared.lanes.nodes.values();
            let owners = (tracks.map(|t| (&t.processes, &t.cells)))
                .chain(nodes.map(|n| (&n.processes, &n.cells)));
            lanes.process_ids.clear();
            lanes.cell_ids.clear();
            for (processes, cells) in owners {
                lanes.process_ids.extend_from_slice(processes);
                lanes.cell_ids.extend_from_slice(cells);
            }
            lanes.process_observed.reset();
            lanes.cell_observed.reset();
            lanes.process_live = None;
            lanes.cell_live = None;
        }
        drop(shared);
        let (processes, cells) = (&lanes.process_ids, &lanes.cell_ids);
        (lanes.process_observed).refresh(pusher.rt, &PROCESS_LIVE.names, || processes.clone());
        (lanes.cell_observed).refresh(pusher.rt, &STATE_CELL_LIVE.names, || cells.clone());
        let state = &pusher.sources.state;
        let epoch = pusher.rt.instance_observer_epoch();
        let live = |observed: &ObservedList, version: u64| {
            (!observed.entries.is_empty()).then_some((version, epoch))
        };
        let process_live = live(&lanes.process_observed, state.process_run_errors_version());
        let cell_live = live(&lanes.cell_observed, state.process_scope_values_version());
        if process_live != lanes.process_live {
            lanes.process_live = process_live;
            lanes.process_observed.push_masked(pusher, &PROCESS_LIVE);
        }
        if cell_live != lanes.cell_live {
            lanes.cell_live = cell_live;
            lanes.cell_observed.push_masked(pusher, &STATE_CELL_LIVE);
        }
    }
}
