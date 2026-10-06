//! Graph sequencers (spec §14, stage 7g): every graph-mode sequencer the
//! scheduler runs (a created kind's instance, such as `neural`, or a script's
//! `def-sequencer`) as a `graph`, its active nodes as `graph-node`s, their
//! outgoing edges as `graph-edge`s and the node and edge params as
//! `graph-param`s; `project.graphs`; and `track.active-notes`.
//!
//! Identity: graphs are positional (`(index)`, in publish order), the
//! instance kept by sequencer id across reorders (`registry::reconcile`) and
//! replaced on a project load (instance ids restart with the project). A
//! created instance's graph has the instance's id as its `gid`, so
//! `(graph-of self)` finds it; creating or deleting the instance publishes or
//! unpublishes its sequencer, which registers or drops the graph (a held
//! handle goes stale). Nodes are keyed (graph instance id, node index): the
//! model numbers them, so a node-count change adds or drops the last ones.
//! Edges are keyed (source node instance id, target index), params (node or
//! edge instance id, name: a stable key per name, [`GraphShared::param_key`],
//! so a re-evaluated prototype that reorders its `:params` keeps each
//! handle on its param, and one that renames or drops a param drops its
//! instance, which goes stale rather than retarget); both register on the
//! first read of `n.params`, `n.edges` or `e.params` (the reader hook), like
//! a device's params, and are then kept current with their graph.
//!
//! Feeds: the model fields behind one key ([`GraphKey`]: the published
//! sequencer version, the scenes revision (every override edit moves it),
//! the current scene, the groups' generation (rack members) and the track and
//! group instances), compared without allocating; when it moves the sync
//! fetches the manifests (only when the published version moved) and the
//! current scene's overrides once and re-derives only the graphs whose
//! manifest, overrides or rack members changed ([`GraphSource`]), none
//! reading the history revision; a sync that re-derived nothing and kept
//! every graph instance pushes nothing. The values
//! derive through the legacy reads' helpers (`graph_node_intrinsic_value`,
//! `graph_node_param_value`, `graph_edge_param_value`,
//! `graph_config_field_value`, `graph_group_cells`,
//! `graph_seed_follows_route`). Live (observed only, compared in place with
//! the last push into scratch buffers, so an idle tick allocates nothing):
//! a graph's playback (`active`, `beat`, `energy`, `triggers`, `dampening`,
//! read in place from the scheduler's visualization snapshot with the legacy
//! `SEQ.graph-visualizations` display transforms), a node's `sounding` (the
//! legacy `graph-node-notes` read, nodes kept in an [`ObservedList`] and read
//! under one snapshot lock per graph) and `track.active-notes`
//! (`active_note_activity_into`, the legacy `SEQ.track-active-notes`).

use super::*;
use sequencer::graph::{
    graph_group_cells, GraphManifest, GraphRuntimeConfig, ParamSpec, ProjectGraphOverrides,
    NODE_SOUNDING_DISPLAY,
};
use sequencer::lisp_host::{
    graph_config_field_value, graph_edge_param_value, graph_node_intrinsic_value,
    graph_node_param_value, graph_seed_follows_route,
};
use sequencer::sequencer::{ActiveNoteActivity, Timebase};

/// What a graph's fields (and its nodes', edges' and params') derive from,
/// as of its last sync; the reader hook registers parts from it.
pub(crate) struct GraphSource {
    pub(super) manifest: GraphManifest,
    pub(super) overrides: Option<ProjectGraphOverrides>,
    pub(super) config: GraphRuntimeConfig,
    /// A rack-owned graph's member tracks in member order (its routes and
    /// seeds are member indices); `None` when the project owns it.
    pub(super) members: Option<Vec<usize>>,
}

impl GraphSource {
    /// The track a route or seed index names (a member index on a
    /// rack-owned graph).
    fn track_of(&self, index: usize) -> Option<usize> {
        match &self.members {
            Some(members) => members.get(index).copied(),
            None => Some(index),
        }
    }

    fn edge_params(&self) -> &[ParamSpec] {
        self.manifest
            .edge_sets
            .first()
            .map_or(&[][..], |set| set.params.as_slice())
    }
}

/// What the reader hook and the tick share about graphs.
#[derive(Default)]
pub(crate) struct GraphShared {
    /// Per graph instance, its source as of its last sync.
    pub(super) sources: HashMap<InstanceId, Rc<GraphSource>>,
    /// Nodes whose params, nodes whose edges, and edges whose params are
    /// registered (read once): kept current with their graph.
    pub(super) node_params: HashSet<InstanceId>,
    pub(super) node_edges: HashSet<InstanceId>,
    pub(super) edge_params: HashSet<InstanceId>,
    /// A param's key under its owner, by name: stable across re-evaluations
    /// (never cleared), so a param keeps its instance when the prototype
    /// reorders its `:params`.
    param_keys: HashMap<String, u64>,
    /// Graph model syncs, and graphs re-derived (tests: an edit elsewhere
    /// re-derives none).
    pub(crate) syncs: u64,
    pub(crate) derives: u64,
}

impl GraphShared {
    /// The key of the param named `name` (see `param_keys`).
    fn param_key(&mut self, name: &str) -> u64 {
        if let Some(key) = self.param_keys.get(name) {
            return *key;
        }
        let key = self.param_keys.len() as u64;
        self.param_keys.insert(name.to_string(), key);
        key
    }

    fn retain_live(&mut self, live: impl Fn(InstanceId) -> bool) {
        self.sources.retain(|id, _| live(*id));
        self.node_params.retain(|id| live(*id));
        self.node_edges.retain(|id| live(*id));
        self.edge_params.retain(|id| live(*id));
    }
}

/// The inputs of the graph model sync, compared without allocating.
struct GraphKey {
    published: u64,
    scenes: u64,
    scene: usize,
    groups: u64,
    tracks: Vec<Option<InstanceId>>,
    group_ids: Vec<Option<InstanceId>>,
}

/// A graph's live fields as last pushed (`flat` is the dampening matrix,
/// row-major, `nodes` wide).
#[derive(Clone, Default, PartialEq)]
struct GraphLive {
    active: bool,
    beat: f64,
    nodes: usize,
    energy: Vec<f64>,
    triggers: Vec<f64>,
    dampening: Vec<f64>,
}

/// What the graph half of the sync keeps across ticks.
#[derive(Default)]
pub(crate) struct GraphState {
    key: Option<GraphKey>,
    /// The published graph manifests, with the published version they were
    /// read at.
    manifests: Option<(u64, Rc<[GraphManifest]>)>,
    /// Sequencer id → graph instance (dropped on a project load).
    pub(super) instances: HashMap<u64, InstanceId>,
    ids: Vec<Option<InstanceId>>,
    /// Every node instance, and the observed ones (`sounding`).
    nodes: Vec<InstanceId>,
    node_observed: ObservedList,
    /// Live values as last pushed, per observing graph, node and track,
    /// with a scratch buffer each so a tick compares in place.
    live: HashMap<InstanceId, GraphLive>,
    live_scratch: GraphLive,
    sounding: HashMap<InstanceId, Vec<f32>>,
    /// The observed nodes' reads this tick, (graph sequencer id, node
    /// index, node instance), grouped by graph; their (note, velocity)
    /// pairs flattened, each node's ending at its `sounding_ends` entry.
    sounding_reads: Vec<(u64, usize, InstanceId)>,
    sounding_scratch: Vec<f32>,
    sounding_ends: Vec<usize>,
    pub(super) active_notes: HashMap<InstanceId, Vec<ActiveNoteActivity>>,
    notes_scratch: Vec<ActiveNoteActivity>,
}

impl GraphState {
    /// One graph instance (the stale check's representative: a hot reload
    /// drops a kind's instances together).
    pub(super) fn representative(&self) -> Option<&InstanceId> {
        self.ids.iter().flatten().next()
    }

    /// Re-derive every graph at the next tick (a schema change or a hot
    /// reload dropped instances); the registered instances are kept.
    pub(super) fn invalidate(&mut self, shared: &RefCell<KindsShared>) {
        self.key = None;
        self.manifests = None;
        self.live.clear();
        self.sounding.clear();
        self.active_notes.clear();
        self.node_observed.reset();
        shared.borrow_mut().graphs.sources.clear();
    }

    /// The graph instances, for a project load (ids restart with it).
    pub(super) fn drain(&mut self) -> impl Iterator<Item = (u64, InstanceId)> + '_ {
        self.key = None;
        self.instances.drain()
    }
}

impl GraphKey {
    fn read(app: &app::App, groups: u64) -> (u64, u64, usize, u64) {
        let state = &app.state;
        (
            state.published_sequencers_version(),
            state.project_scenes_revision(),
            state.current_scene_index(),
            groups,
        )
    }

    fn matches(
        &self,
        read: (u64, u64, usize, u64),
        tracks: &[Option<InstanceId>],
        group_ids: &[Option<InstanceId>],
    ) -> bool {
        (self.published, self.scenes, self.scene, self.groups) == read
            && self.tracks == tracks
            && self.group_ids == group_ids
    }
}

fn labels<'a>(labels: impl IntoIterator<Item = &'a str>) -> Value {
    list_value(labels.into_iter().map(text))
}

fn quantize_label(quantize: Option<Timebase>) -> &'static str {
    quantize.map_or("off", |timebase| timebase.label())
}

/// A param's fixed fields and value (`owner` the node or edge instance).
fn push_graph_param<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    (owner, edge): (InstanceId, bool),
    index: usize,
    spec: &sequencer::graph::ParamSpec,
    value: f64,
) {
    let (node, edge) = if edge {
        (Value::Nil, Value::Instance(owner))
    } else {
        (Value::Instance(owner), Value::Nil)
    };
    put(store, shared, id, f::GRAPH_PARAM_NODE, node);
    put(store, shared, id, f::GRAPH_PARAM_EDGE, edge);
    put(
        store,
        shared,
        id,
        f::GRAPH_PARAM_INDEX,
        number(index as f64),
    );
    put(store, shared, id, f::GRAPH_PARAM_NAME, text(&spec.name));
    let kind = if spec.is_int { "int" } else { "float" };
    put(store, shared, id, f::GRAPH_PARAM_TYPE, text(kind));
    put(store, shared, id, f::GRAPH_PARAM_MIN, number(spec.min));
    put(store, shared, id, f::GRAPH_PARAM_MAX, number(spec.max));
    put(
        store,
        shared,
        id,
        f::GRAPH_PARAM_DEFAULT,
        number(spec.default),
    );
    put(store, shared, id, f::GRAPH_PARAM_VALUE, number(value));
}

/// The params `specs` of `owner` (a node, or an edge when `edge`), keyed
/// by name: reconciled (a param the prototype no longer has goes stale,
/// a new one registers), pushed with the value `value` reads (else the
/// default), and marked registered.
fn sync_params<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    (owner, edge): (InstanceId, bool),
    specs: &[ParamSpec],
    value: impl Fn(&str) -> Option<f64>,
) -> Value {
    let keys: Vec<u64> = {
        let graphs = &mut shared.borrow_mut().graphs;
        specs
            .iter()
            .map(|spec| graphs.param_key(&spec.name))
            .collect()
    };
    let (ids, _) = reconcile_children(store, owner, GRAPH_PARAM, &keys, |_, _, _| {});
    let mut list = Vec::with_capacity(ids.len());
    for (at, (spec, id)) in specs.iter().zip(ids).enumerate() {
        let Some(id) = id else { continue };
        let value = value(&spec.name).unwrap_or(spec.default);
        push_graph_param(store, shared, id, (owner, edge), at, spec, value);
        list.push(id);
    }
    let graphs = &mut shared.borrow_mut().graphs;
    let registered = if edge {
        &mut graphs.edge_params
    } else {
        &mut graphs.node_params
    };
    registered.insert(owner);
    instance_list(list)
}

/// Node `index` of `src`'s param values.
fn node_param_reader(src: &GraphSource, index: usize) -> impl Fn(&str) -> Option<f64> + '_ {
    move |name| graph_node_param_value(&src.manifest, &src.config, index, name)
}

/// Edge `from` → `to` of `src`'s param values.
fn edge_param_reader(
    src: &GraphSource,
    (from, to): (usize, usize),
) -> impl Fn(&str) -> Option<f64> + '_ {
    let edge = (src.config.edges.iter()).find(|edge| edge.from == from && edge.to == to);
    move |name| edge.and_then(|edge| graph_edge_param_value(edge, name))
}

/// Node `node` (index `index` of graph `graph`)'s outgoing edges,
/// reconciled with the resolved config (registered edges' params kept
/// current) and pushed; marks them registered.
fn sync_node_edges<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    (graph, node): (InstanceId, InstanceId),
    index: usize,
    src: &GraphSource,
) -> Value {
    let targets: Vec<u64> = (src.config.edges.iter())
        .filter(|edge| edge.from == index)
        .map(|edge| edge.to as u64)
        .collect();
    let (ids, _) = reconcile_children(store, node, GRAPH_EDGE, &targets, |_, _, _| {});
    let mut edges = Vec::with_capacity(ids.len());
    for (to, id) in targets.iter().zip(ids) {
        let Some(id) = id else { continue };
        put(store, shared, id, f::GRAPH_EDGE_FROM, Value::Instance(node));
        let target = store.keyed(GRAPH_NODE, &[graph, *to]);
        put(store, shared, id, f::GRAPH_EDGE_TO, instance_or_nil(target));
        if shared.borrow().graphs.edge_params.contains(&id) {
            let read = edge_param_reader(src, (index, *to as usize));
            let params = sync_params(store, shared, (id, true), src.edge_params(), read);
            put(store, shared, id, f::GRAPH_EDGE_PARAMS, params);
        }
        edges.push(id);
    }
    shared.borrow_mut().graphs.node_edges.insert(node);
    instance_list(edges)
}

/// The graph instance and node index of node `node`, with its graph's
/// source.
fn node_source<S: KindStore>(
    store: &S,
    shared: &RefCell<KindsShared>,
    node: InstanceId,
) -> Option<(InstanceId, usize, Rc<GraphSource>)> {
    let &[graph, index] = store.key_of(node)? else {
        return None;
    };
    let src = shared.borrow().graphs.sources.get(&graph)?.clone();
    Some((graph, index as usize, src))
}

/// `n.params`, `n.edges` or `e.params` read for the first time (the reader
/// hook): register them from the graph's source. `None` once registered
/// (the cell holds the list) or when nothing is known about the graph.
pub(super) fn cold_graph_parts<S: KindStore>(
    store: &mut S,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    if shared.borrow().skip.contains(&key) {
        return None;
    }
    let value = match key {
        f::GRAPH_NODE_PARAMS => {
            if shared.borrow().graphs.node_params.contains(&id) {
                return None;
            }
            let (_, index, src) = node_source(store, shared, id)?;
            let specs = &src.manifest.node.params;
            sync_params(
                store,
                shared,
                (id, false),
                specs,
                node_param_reader(&src, index),
            )
        }
        f::GRAPH_NODE_EDGES => {
            if shared.borrow().graphs.node_edges.contains(&id) {
                return None;
            }
            let (graph, index, src) = node_source(store, shared, id)?;
            sync_node_edges(store, shared, (graph, id), index, &src)
        }
        f::GRAPH_EDGE_PARAMS => {
            if shared.borrow().graphs.edge_params.contains(&id) {
                return None;
            }
            let &[node, to] = store.key_of(id)? else {
                return None;
            };
            let (_, from, src) = node_source(store, shared, node)?;
            let read = edge_param_reader(&src, (from, to as usize));
            sync_params(store, shared, (id, true), src.edge_params(), read)
        }
        _ => return None,
    };
    shared.borrow_mut().count(key);
    Some(value)
}

/// A graph's live values from the scheduler's visualization snapshot (the
/// legacy `SEQ.graph-visualizations` transforms), into `live`; zeros sized
/// to its active nodes before the scheduler ran it.
fn read_graph_live(sources: &KindsHandles, src: &GraphSource, live: &mut GraphLive) {
    live.energy.clear();
    live.triggers.clear();
    live.dampening.clear();
    sources
        .state
        .with_graph_visualization(src.manifest.id, |snapshot| {
            let Some(snapshot) = snapshot else {
                let nodes = src.config.nodes.len();
                (live.active, live.beat, live.nodes) = (false, 0.0, nodes);
                live.energy.resize(nodes, 0.0);
                live.triggers.resize(nodes, 0.0);
                live.dampening.resize(nodes * nodes, 0.0);
                return;
            };
            let nodes = snapshot.num_nodes;
            (live.active, live.beat, live.nodes) = (snapshot.active, snapshot.current_beat, nodes);
            let energy = snapshot.energy.iter().take(nodes);
            live.energy
                .extend(energy.map(|value| graph_energy_display_value(*value)));
            let triggers = snapshot.trigger_activity.iter().take(nodes);
            live.triggers
                .extend(triggers.map(|value| neural_trigger_display_value(*value)));
            live.dampening.resize(nodes * nodes, 0.0);
            for edge in &snapshot.edges {
                if edge.from < nodes && edge.to < nodes {
                    live.dampening[edge.from * nodes + edge.to] =
                        neural_dampening_display_value(edge.dampening as f32);
                }
            }
        });
}

/// `values` as rows of `width` numbers (none when `width` is 0).
fn matrix<T: Copy + Into<f64>>(values: &[T], width: usize) -> Value {
    if width == 0 {
        return list_value(std::iter::empty());
    }
    list_value(values.chunks(width).map(numbers))
}

/// The notes node `index` of `snapshot`'s graph sounds at `sample`, as
/// (note, velocity) pairs appended to `out` (the legacy `graph-node-notes`
/// read: at most `NODE_SOUNDING_DISPLAY`).
fn node_sounding(
    snapshot: Option<&sequencer::graph::GraphVisualizationSnapshot>,
    index: usize,
    sample: u64,
    out: &mut Vec<f32>,
) {
    let Some(notes) = snapshot.and_then(|snapshot| snapshot.node_sounding.get(index)) else {
        return;
    };
    let sounding = notes.iter().filter(|note| note.is_sounding_at(sample));
    for note in sounding.take(NODE_SOUNDING_DISPLAY) {
        out.extend([note.note, note.velocity]);
    }
}

fn active_note_rows(notes: &[ActiveNoteActivity]) -> Value {
    list_value(notes.iter().map(|activity| {
        list_value([
            number(activity.note),
            number(activity.velocity),
            number(activity.trigger_id as f64),
        ])
    }))
}

/// A graph's, a node's or a track's live field (the reader hook's cold
/// read; the tick compares in place instead).
pub(super) fn graph_live_value<S: KindStore>(
    store: &S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    key: FieldKey,
) -> Option<Value> {
    match key {
        f::GRAPH_NODE_SOUNDING => {
            let (_, index, src) = node_source(store, shared, id)?;
            let mut pairs = Vec::new();
            let state = &sources.state;
            if state.is_playing() {
                let sample = state.audio_rendered_sample();
                state.with_graph_visualization(src.manifest.id, |snapshot| {
                    node_sounding(snapshot, index, sample, &mut pairs)
                });
            }
            Some(matrix(&pairs, 2))
        }
        f::TRACK_ACTIVE_NOTES => {
            let track = *store.key_of(id)?.first()? as usize;
            sources
                .track_exists(track)
                .then(|| active_note_rows(&sources.state.active_note_activity(track)))
        }
        _ => {
            let src = shared.borrow().graphs.sources.get(&id)?.clone();
            let mut live = GraphLive::default();
            read_graph_live(sources, &src, &mut live);
            Some(graph_live_field(&live, key))
        }
    }
}

fn graph_live_field(live: &GraphLive, key: FieldKey) -> Value {
    match key {
        f::GRAPH_ACTIVE => Value::Bool(live.active),
        f::GRAPH_BEAT => number(live.beat),
        f::GRAPH_ENERGY => numbers(&live.energy),
        f::GRAPH_TRIGGERS => numbers(&live.triggers),
        _ => matrix(&live.dampening, live.nodes),
    }
}

impl HostKinds {
    /// The graphs, their nodes and registered parts and `project.graphs`,
    /// when [`GraphKey`] moved. Runs after the model sync (tracks, groups).
    pub(super) fn sync_graph_model(&mut self, pusher: &mut Pusher<'_>, app: &app::App) {
        let read = GraphKey::read(app, self.groups_generation);
        let (tracks, group_ids) = (&self.track_ids, &self.group_ids);
        let graphs = &mut self.graphs;
        if (graphs.key.as_ref()).is_some_and(|key| key.matches(read, tracks, group_ids)) {
            return;
        }
        let context_moved = !graphs
            .key
            .as_ref()
            .is_some_and(|key| key.tracks == *tracks && key.group_ids == *group_ids);
        // The manifests only move with the published version: a scenes
        // move (a step edit) copies no published sequencer, and leaves every
        // source's manifest as it was.
        let published_moved =
            !matches!(&graphs.manifests, Some((version, _)) if *version == read.0);
        let manifests = match &graphs.manifests {
            Some((_, manifests)) if !published_moved => manifests.clone(),
            _ => {
                let manifests: Rc<[GraphManifest]> = (app.state.published_sequencers())
                    .into_iter()
                    .filter_map(|published| published.graph)
                    .collect();
                graphs.manifests = Some((read.0, manifests.clone()));
                manifests
            }
        };
        let model: Vec<u64> = manifests.iter().map(|manifest| manifest.id).collect();
        if !distinct(&model) {
            return; // retried next tick
        }
        let ids = reconcile(pusher, GRAPH, &mut graphs.instances, &model);
        let rt = &*pusher.rt;
        pusher
            .shared
            .borrow_mut()
            .graphs
            .retain_live(|id| rt.instance_is_live(id));
        // The live caches of graphs and tracks that are gone.
        graphs.live.retain(|id, _| ids.contains(&Some(*id)));
        graphs
            .active_notes
            .retain(|id, _| tracks.contains(&Some(*id)));
        let ids_moved = ids != graphs.ids;
        let mut overrides = if manifests.is_empty() {
            Vec::new()
        } else {
            app.state.current_graph_overrides()
        };
        let mut derived = false;
        for (index, (manifest, id)) in manifests.iter().zip(&ids).enumerate() {
            let Some(id) = *id else { continue };
            pusher.push(id, f::GRAPH_INDEX, number(index as f64));
            let overrides = (overrides.iter())
                .position(|overrides| manifest.matches_overrides(overrides))
                .map(|at| overrides.swap_remove(at));
            let members = manifest.owner_rack.map(|rack| {
                let group = app.groups.iter().find(|group| group.id == rack);
                group.map_or_else(Vec::new, |group| group.members.clone())
            });
            let shared = pusher.shared.borrow();
            let kept = (shared.graphs.sources.get(&id)).is_some_and(|src| {
                !context_moved
                    && (!published_moved || src.manifest == *manifest)
                    && src.overrides == overrides
                    && src.members == members
            });
            drop(shared);
            if kept {
                continue;
            }
            let config = manifest.runtime_config_with_overrides(overrides.as_ref());
            let src = Rc::new(GraphSource {
                manifest: manifest.clone(),
                overrides,
                config,
                members,
            });
            let mut shared = pusher.shared.borrow_mut();
            shared.graphs.sources.insert(id, src.clone());
            shared.graphs.derives += 1;
            drop(shared);
            self.push_graph(pusher, id, &src);
            derived = true;
        }
        let graphs = &mut self.graphs;
        if derived || ids_moved {
            let mut nodes = Vec::with_capacity(graphs.nodes.len());
            for id in ids.iter().flatten() {
                let count = (pusher.shared.borrow().graphs.sources.get(id))
                    .map_or(0, |src| src.config.nodes.len());
                nodes.extend(
                    (0..count).filter_map(|node| {
                        pusher.rt.keyed_instance(GRAPH_NODE, &[*id, node as u64])
                    }),
                );
            }
            if let Some(project) = pusher.singleton(PROJECT) {
                pusher.push(project, f::PROJECT_GRAPHS, listed_instances(&ids));
            }
            if nodes != graphs.nodes {
                graphs.nodes = nodes;
                graphs.node_observed.reset();
            }
        }
        graphs.ids = ids;
        pusher.shared.borrow_mut().graphs.syncs += 1;
        graphs.key = Some(GraphKey {
            published: read.0,
            scenes: read.1,
            scene: read.2,
            groups: read.3,
            tracks: self.track_ids.clone(),
            group_ids: self.group_ids.clone(),
        });
    }

    /// One graph's fields, its nodes' and their registered parts'.
    fn push_graph(&self, pusher: &mut Pusher<'_>, id: InstanceId, src: &GraphSource) {
        let (manifest, overrides, config) = (&src.manifest, src.overrides.as_ref(), &src.config);
        pusher.push(id, f::GRAPH_GID, number(manifest.id as f64));
        pusher.push(id, f::GRAPH_NAME, text(&manifest.name));
        let owner = manifest
            .owner_rack
            .and_then(|gid| self.groups.get(&gid).copied());
        pusher.push(id, f::GRAPH_OWNER, instance_or_nil(owner));
        let shape = &manifest.shape;
        let (min, max) = shape
            .variable_line_bounds()
            .map_or((shape.num_nodes(), shape.num_nodes()), |(_, min, max)| {
                (min, max)
            });
        pusher.push(id, f::GRAPH_VARIABLE, Value::Bool(shape.is_variable_line()));
        pusher.push(id, f::GRAPH_MIN_NODES, number(min as f64));
        pusher.push(id, f::GRAPH_MAX_NODES, number(max as f64));
        let count = config.nodes.len();
        pusher.push(id, f::GRAPH_NODE_COUNT, number(count as f64));
        let config_fields = [
            (f::GRAPH_RESET_BARS, "reset-bars"),
            (f::GRAPH_MAX_POLY, "max-poly"),
            (f::GRAPH_MAX_POLY_SELECTION, "max-poly-selection"),
            (f::GRAPH_GROUP_TRACE_DECAY, "group-trace-decay"),
            (f::GRAPH_GROUP_COUPLING_SCALE, "group-coupling-scale"),
            (f::GRAPH_GROUP_EXCITE_FLOOR, "group-excite-floor"),
        ];
        for (key, field) in config_fields {
            if let Ok(value) = graph_config_field_value(manifest, overrides, || count, field) {
                pusher.push(id, key, value);
            }
        }
        let gain = graph_group_cells(
            overrides.and_then(|o| o.group_gain.as_ref()),
            sequencer::graph::GROUP_GAIN_DEFAULT,
        );
        pusher.push(id, f::GRAPH_GROUP_GAIN, numbers(&gain));
        let coupling = graph_group_cells(
            overrides.and_then(|o| o.group_coupling.as_ref()),
            sequencer::graph::GROUP_COUPLING_DEFAULT,
        );
        pusher.push(id, f::GRAPH_GROUP_COUPLING, numbers(&coupling));
        let dropped = drop_children_past(&mut *pusher.rt, id, GRAPH_NODE, count);
        let mut added = false;
        let nodes = indexed_children(&mut *pusher.rt, id, GRAPH_NODE, count, |_, _, _| {
            added = true;
        });
        pusher.changed |= dropped || added;
        for index in 0..count {
            if let Some(node) = pusher.rt.keyed_instance(GRAPH_NODE, &[id, index as u64]) {
                self.push_node(pusher, (id, node), index, src);
            }
        }
        pusher.push(id, f::GRAPH_NODES, instance_list(nodes));
    }

    /// One node's fields and its registered parts'.
    fn push_node(
        &self,
        pusher: &mut Pusher<'_>,
        (graph, id): (InstanceId, InstanceId),
        index: usize,
        src: &GraphSource,
    ) {
        let node = &src.config.nodes[index];
        pusher.push(id, f::GRAPH_NODE_GRAPH, Value::Instance(graph));
        pusher.push(id, f::GRAPH_NODE_INDEX, number(index as f64));
        pusher.push(id, f::GRAPH_NODE_RESOLUTION, text(node.resolution.label()));
        let cycle = node
            .resolution_cycle
            .iter()
            .map(|timebase| timebase.label());
        pusher.push(id, f::GRAPH_NODE_RESOLUTION_CYCLE, labels(cycle));
        pusher.push(
            id,
            f::GRAPH_NODE_QUANTIZE,
            text(quantize_label(node.quantize)),
        );
        let cycle = node
            .quantize_cycle
            .iter()
            .map(|quantize| quantize_label(*quantize));
        pusher.push(id, f::GRAPH_NODE_QUANTIZE_CYCLE, labels(cycle));
        let intrinsic = |field| graph_node_intrinsic_value(node, || false, field);
        if let Ok(value) = intrinsic("delay") {
            pusher.push(id, f::GRAPH_NODE_DELAY, value);
        }
        let track = |index: usize| {
            let track = src.track_of(index)?;
            self.track_ids.get(track).copied().flatten()
        };
        let route = node.route.and_then(track);
        pusher.push(id, f::GRAPH_NODE_ROUTE, instance_or_nil(route));
        let gate = node.gate_target;
        let generator = gate.map_or(-1.0, |gate| gate.id as f64);
        pusher.push(id, f::GRAPH_NODE_GENERATOR, number(generator));
        let restart = gate.is_some_and(|gate| gate.restart);
        pusher.push(id, f::GRAPH_NODE_RESTART, Value::Bool(restart));
        let follows = graph_seed_follows_route(&src.manifest, src.overrides.as_ref(), index);
        pusher.push(id, f::GRAPH_NODE_SEED_ROUTE, Value::Bool(follows));
        let mask = node.seed_track_mask;
        let seeds = (0..128).filter(|bit| mask & (1u128 << bit) != 0);
        pusher.push(
            id,
            f::GRAPH_NODE_SEEDS,
            instance_list(seeds.filter_map(track)),
        );
        pusher.push(id, f::GRAPH_NODE_SEED_ON_RESET, number(node.seed_on_reset));
        pusher.push(id, f::GRAPH_NODE_GROUP, number(node.neural_group));
        let (params, edges) = {
            let shared = pusher.shared.borrow();
            let graphs = &shared.graphs;
            (
                graphs.node_params.contains(&id),
                graphs.node_edges.contains(&id),
            )
        };
        if params {
            let specs = &src.manifest.node.params;
            let read = node_param_reader(src, index);
            let list = sync_params(&mut *pusher.rt, pusher.shared, (id, false), specs, read);
            pusher.push(id, f::GRAPH_NODE_PARAMS, list);
        }
        if edges {
            let list = sync_node_edges(&mut *pusher.rt, pusher.shared, (graph, id), index, src);
            pusher.push(id, f::GRAPH_NODE_EDGES, list);
        }
    }

    /// The observed live fields of every graph (playback) and node
    /// (`sounding`), compared in place with the last push.
    pub(super) fn sync_graph_live(&mut self, pusher: &mut Pusher<'_>) {
        let graphs = &mut self.graphs;
        for id in graphs.ids.iter().flatten().copied() {
            let mask = pusher.rt.host_fields_observed(id, &GRAPH_LIVE.names);
            if mask == 0 {
                graphs.live.remove(&id);
                continue;
            }
            let Some(src) = pusher.shared.borrow().graphs.sources.get(&id).cloned() else {
                continue;
            };
            let scratch = &mut graphs.live_scratch;
            read_graph_live(pusher.sources, &src, scratch);
            let last = graphs.live.get(&id);
            for (bit, key) in GRAPH_LIVE.keys.iter().enumerate() {
                if mask & (1 << bit) == 0 {
                    continue;
                }
                let changed = last.is_none_or(|last| match *key {
                    f::GRAPH_ACTIVE => last.active != scratch.active,
                    f::GRAPH_BEAT => last.beat != scratch.beat,
                    f::GRAPH_ENERGY => last.energy != scratch.energy,
                    f::GRAPH_TRIGGERS => last.triggers != scratch.triggers,
                    _ => last.nodes != scratch.nodes || last.dampening != scratch.dampening,
                });
                pusher.push_computed_if(id, *key, changed, || graph_live_field(scratch, *key));
            }
            match graphs.live.get_mut(&id) {
                Some(last) if *last == *scratch => {}
                Some(last) => std::mem::swap(last, scratch),
                None => {
                    graphs.live.insert(id, scratch.clone());
                }
            }
        }
        let nodes = &graphs.nodes;
        let names = &GRAPH_NODE_LIVE.names;
        graphs
            .node_observed
            .refresh(pusher.rt, names, || nodes.clone());
        // The nodes whose `sounding` is observed, grouped by graph, each
        // graph's read under one snapshot lock.
        let sounding_bit = GRAPH_NODE_LIVE.bit(f::GRAPH_NODE_SOUNDING);
        let reads = &mut graphs.sounding_reads;
        reads.clear();
        for (id, mask, _) in &graphs.node_observed.entries {
            if mask & sounding_bit == 0 {
                continue;
            }
            if let Some((_, index, src)) = node_source(&*pusher.rt, pusher.shared, *id) {
                reads.push((src.manifest.id, index, *id));
            }
        }
        graphs
            .sounding
            .retain(|id, _| reads.iter().any(|read| read.2 == *id));
        if reads.is_empty() {
            return;
        }
        reads.sort_unstable_by_key(|read| read.0);
        let (pairs, ends) = (&mut graphs.sounding_scratch, &mut graphs.sounding_ends);
        pairs.clear();
        ends.clear();
        let state = &pusher.sources.state;
        let (playing, sample) = (state.is_playing(), state.audio_rendered_sample());
        for group in reads.chunk_by(|a, b| a.0 == b.0) {
            if !playing {
                // None while stopped.
                ends.extend(group.iter().map(|_| pairs.len()));
                continue;
            }
            state.with_graph_visualization(group[0].0, |snapshot| {
                for (_, index, _) in group {
                    node_sounding(snapshot, *index, sample, pairs);
                    ends.push(pairs.len());
                }
            });
        }
        let mut start = 0;
        for ((_, _, id), end) in reads.iter().zip(ends.iter()) {
            let node = &pairs[start..*end];
            start = *end;
            let changed = (graphs.sounding.get(id)).is_none_or(|last| last != node);
            let key = f::GRAPH_NODE_SOUNDING;
            pusher.push_computed_if(*id, key, changed, || matrix(node, 2));
            if changed {
                let last = graphs.sounding.entry(*id).or_default();
                last.clear();
                last.extend_from_slice(node);
            }
        }
    }

    /// `t.active-notes` while observed: pushed when the track's sounding
    /// notes changed since the last push (read into a scratch buffer and
    /// compared in place).
    pub(super) fn sync_active_notes(
        &mut self,
        pusher: &mut Pusher<'_>,
        track: usize,
        id: InstanceId,
    ) {
        let graphs = &mut self.graphs;
        let notes = &mut graphs.notes_scratch;
        pusher.sources.state.active_note_activity_into(track, notes);
        let changed = (graphs.active_notes.get(&id)).is_none_or(|last| last != notes);
        let key = f::TRACK_ACTIVE_NOTES;
        pusher.push_computed_if(id, key, changed, || active_note_rows(notes));
        if changed {
            std::mem::swap(graphs.active_notes.entry(id).or_default(), notes);
        }
    }
}
