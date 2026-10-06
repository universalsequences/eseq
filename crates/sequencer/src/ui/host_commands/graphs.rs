//! The graph kinds' setters (kind-bindings spec §14, stage 7g): `set-graph`
//! (`:graph-id`, `:field`, `:value`; `:node` for a node's field, with
//! `:param` for its param or `:to` and `:param` for an edge's; `:row` and
//! `:col` for a group matrix cell; `:track-id` for a route; `:restart` for a
//! generator).
//!
//! A graph is named by its sequencer id (matched as the number Lisp holds,
//! so a legacy hashed id past 2^53 still resolves), a node by its index
//! among the active nodes, an edge by its endpoints, a param by its name, a
//! track by its stable `TrackId`, all resolved when the command lands; a
//! gone graph, a node past the active count, a missing edge or an unknown
//! param is an error. Edits act only where the current scene's resolved
//! value differs and write the override field the legacy `graph-*` natives
//! write (`ProjectGraphOverrides`), through `App::apply_graph_override_edit`:
//! one undo entry each, per field ([`GraphOverrideSlot`]: undo restores that
//! field as it was, leaving every other edit of the graph alone). Values
//! follow the value rule ([`SetValue`]): a label among its options
//! (case-insensitive), a number finite and in range (no clamping, unlike the
//! legacy natives), an integer whole, a track a track of the graph's owner
//! (a rack's member on a rack-owned graph), a generator a generator instance
//! of the graph's owner other than the graph's own; anything else is an
//! error that changes nothing. Gestures as [`super::ScriptEdit`]: a numeric
//! field's `set!`s while the pointer is down join one entry per field.

use super::track_settings::{command_track, SetValue};
use crate::*;
use sequencer::graph::{
    GraphConfigField, GraphManifest, GraphNodeField, GraphOverrideSlot, GraphRuntimeConfig,
    GroupMatrix, ParamSpec, ProjectGraphOverrides, ProjectGraphQuantizeOverride,
    ProjectGraphRouteOverride, ProjectGraphSeedFrom,
};
use sequencer::neural::NeuralMaxPolySelection;
use sequencer::sequencer::Timebase;
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &["set-graph"];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// What `set-graph` asks for: the override field to write and whether a
/// drag moves it (continuous); `None` when the graph already is so.
type Request = Result<Option<(GraphOverrideSlot, bool)>, String>;

/// The graph `:graph-id` names now, with the current scene's overrides of
/// it and its resolved config (`edit-process` addresses a node's patch by
/// it too).
pub(super) struct Graph {
    pub(super) manifest: GraphManifest,
    overrides: Option<ProjectGraphOverrides>,
    pub(super) config: GraphRuntimeConfig,
}

impl Graph {
    pub(super) fn of(app: &app::App, map: &Payload) -> Result<Self, String> {
        let gid = match SetValue::of(map, "graph-id", "graph-id").value() {
            Value::Number(gid) if gid.is_finite() => *gid,
            _ => return Err("needs a :graph-id".to_string()),
        };
        let manifest =
            sequencer::lisp_host::published_graph_manifest(&app.state, |id| id as f64 == gid)
                .ok_or("the graph is gone")?;
        let overrides =
            sequencer::lisp_host::resolved_graph_overrides_for_manifest(&app.state, &manifest);
        let config = manifest.runtime_config_with_overrides(overrides.as_ref());
        Ok(Self {
            manifest,
            overrides,
            config,
        })
    }

    /// The active node `:node` names.
    pub(super) fn node(&self, map: &Payload) -> Result<usize, String> {
        let count = self.config.nodes.len();
        let node = SetValue::of(map, "node", "node");
        match count {
            0 => Err("the graph has no nodes".to_string()),
            count => node
                .integer(0, count - 1)
                .map_err(|_| "the node is gone".to_string()),
        }
    }

    /// Whether node `node` seeds from its route.
    fn seed_follows(&self, node: usize) -> bool {
        sequencer::lisp_host::graph_seed_follows_route(
            &self.manifest,
            self.overrides.as_ref(),
            node,
        )
    }

    /// The route or seed index of `track` on this graph: the track itself,
    /// or its member index on a rack-owned graph.
    fn route_index(&self, app: &app::App, track: usize) -> Result<usize, String> {
        let Some(rack) = self.manifest.owner_rack else {
            return Ok(track);
        };
        let group = (app.groups.iter()).find(|group| group.id == rack);
        group
            .and_then(|group| group.members.iter().position(|member| *member == track))
            .ok_or_else(|| "the track is not a member of the graph's rack".to_string())
    }

    /// Whether `id` is a generator instance of the graph's owner (a jaki of
    /// its rack, or of the project), other than the graph's own instance.
    fn gates(&self, app: &app::App, id: u64) -> bool {
        let owner = self.manifest.owner_rack;
        id != self.manifest.id
            && (app.instances.list.iter())
                .any(|instance| instance.id == id && instance.owner.rack() == owner)
            && (app.state.published_sequencers().iter()).any(|published| {
                published.id == id && published.graph.is_none() && published.owner_rack == owner
            })
    }

    /// Node `node`'s `field` set to its value.
    fn node_field(&self, node: usize, field: GraphNodeField, continuous: bool) -> Request {
        let slot = GraphOverrideSlot::NodeField {
            group: self.manifest.node.name.clone(),
            instance: node,
            field,
        };
        Ok(Some((slot, continuous)))
    }
}

/// A timebase label (`graph-timebase-options`), case-insensitively.
fn timebase(field: &str, label: &str) -> Result<Timebase, String> {
    let value = SetValue::new(field, Value::String(label.to_string()));
    let index = value.choice(&Timebase::LABELS)?;
    Ok(Timebase::from_index(index as u32))
}

/// The labels `field` takes: one (`single`), or a cycle's non-empty list.
fn labels_for(field: &str, single: bool, value: &SetValue<'_>) -> Result<Vec<String>, String> {
    if single {
        return Ok(vec![value.label()?.to_string()]);
    }
    let Value::List(items) = value.value() else {
        return value.fail("a list of labels");
    };
    let labels: Option<Vec<String>> = (items.iter())
        .map(|item| match &*item.borrow() {
            Value::String(label) => Some(label.clone()),
            _ => None,
        })
        .collect();
    match labels {
        Some(labels) if !labels.is_empty() => Ok(labels),
        _ => Err(format!("{field} takes a non-empty list of labels")),
    }
}

/// A node or edge param's request: `:param` names one of `specs` (the
/// `owner`'s), whose value `current` reads; `slot` builds the override. The
/// value is finite and in the param's range (an int param's whole).
fn param_request(
    value: &SetValue<'_>,
    map: &Payload,
    (owner, specs): (&str, &[ParamSpec]),
    current: impl FnOnce(&str) -> Option<f64>,
    slot: impl FnOnce(String, f64) -> GraphOverrideSlot,
) -> Request {
    let name = map_string(map, "param").ok_or("needs a :param")?;
    let spec = (specs.iter())
        .find(|spec| spec.name == name)
        .ok_or_else(|| format!("the {owner} has no param {name}"))?;
    let v = if spec.is_int {
        let range = (spec.min.ceil() as i64, spec.max.floor() as i64);
        value.signed(range.0, range.1).map(|v| v as f64)
    } else {
        value.number(spec.min, spec.max)
    }?;
    if current(&name) == Some(v) {
        return Ok(None);
    }
    Ok(Some((slot(name, v), true)))
}

/// What `set-graph` asks for.
fn graph_request(app: &app::App, graph: &Graph, map: &Payload) -> Request {
    let (field, value) = SetValue::field(map)?;
    let resolved = |map: &Payload| -> Result<(usize, &sequencer::graph::GraphNode), String> {
        let node = graph.node(map)?;
        Ok((node, &graph.config.nodes[node]))
    };
    match field.as_str() {
        "resolution" | "resolution-cycle" => {
            let labels = labels_for(&field, field == "resolution", &value)?;
            let cycle = (labels.iter())
                .map(|label| timebase(&field, label))
                .collect::<Result<Vec<_>, _>>()?;
            let (node, current) = resolved(map)?;
            if current.resolution_cycle == cycle {
                return Ok(None);
            }
            let indices = cycle.iter().map(|timebase| *timebase as u8).collect();
            graph.node_field(node, GraphNodeField::Resolution(Some(indices)), false)
        }
        "quantize" | "quantize-cycle" => {
            let labels = labels_for(&field, field == "quantize", &value)?;
            let off = |label: &String| label.eq_ignore_ascii_case("off");
            let quantize = match labels.as_slice() {
                [only] if off(only) => ProjectGraphQuantizeOverride::Off,
                labels if labels.iter().any(off) => {
                    return value.fail("off alone, or labels of graph-timebase-options");
                }
                labels => ProjectGraphQuantizeOverride::Timebase(
                    (labels.iter())
                        .map(|label| timebase(&field, label).map(|timebase| timebase as u8))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            };
            let cycle: Vec<Option<Timebase>> = match &quantize {
                ProjectGraphQuantizeOverride::Off => vec![None],
                ProjectGraphQuantizeOverride::Timebase(indices) => (indices.iter())
                    .map(|index| Some(Timebase::from_index(u32::from(*index))))
                    .collect(),
            };
            let (node, current) = resolved(map)?;
            if current.quantize_cycle == cycle {
                return Ok(None);
            }
            graph.node_field(node, GraphNodeField::Quantize(Some(quantize)), false)
        }
        "delay" => {
            let delay = value.integer(0, u32::MAX as usize)? as u32;
            let (node, current) = resolved(map)?;
            if current.delay_steps == delay {
                return Ok(None);
            }
            graph.node_field(node, GraphNodeField::Delay(Some(delay)), true)
        }
        "route" => {
            let track = SetValue::of(map, "track-id", "route").id_or_nil("a track or nil")?;
            let route = match track {
                None => None,
                Some(_) => Some(graph.route_index(app, command_track(app, map)?)?),
            };
            let (node, current) = resolved(map)?;
            if current.route == route && current.gate_target.is_none() {
                return Ok(None);
            }
            let route = route.map_or(
                ProjectGraphRouteOverride::None,
                ProjectGraphRouteOverride::Track,
            );
            graph.node_field(node, GraphNodeField::Route(Some(route)), false)
        }
        "generator" => {
            let id = value.id("a generator instance id")?;
            if !graph.gates(app, id) {
                return value.fail("a generator instance of the graph's owner");
            }
            let restart = SetValue::of(map, "restart", "restart").flag_or(false)?;
            let target = sequencer::graph::GateTarget { id, restart };
            let (node, current) = resolved(map)?;
            if current.gate_target == Some(target) {
                return Ok(None);
            }
            let route = if restart {
                ProjectGraphRouteOverride::GeneratorRestart(id)
            } else {
                ProjectGraphRouteOverride::Generator(id)
            };
            graph.node_field(node, GraphNodeField::Route(Some(route)), false)
        }
        "seed-route" => {
            let on = value.flag()?;
            let node = graph.node(map)?;
            if on == graph.seed_follows(node) {
                return Ok(None);
            }
            let seed = if on {
                ProjectGraphSeedFrom::Route
            } else {
                ProjectGraphSeedFrom::Tracks(Vec::new())
            };
            graph.node_field(node, GraphNodeField::SeedFrom(Some(seed)), false)
        }
        "seeds" => {
            let seeds: Result<Vec<_>, _> = (value.tracks(app)?.into_iter())
                .map(|track| graph.route_index(app, track))
                .collect();
            let mut seeds = seeds?;
            seeds.sort_unstable();
            seeds.dedup();
            if seeds.iter().any(|seed| *seed >= 128) {
                return value.fail("tracks among the first 128");
            }
            let (node, current) = resolved(map)?;
            let mask = seeds.iter().fold(0u128, |mask, seed| mask | 1 << seed);
            if !graph.seed_follows(node) && current.seed_track_mask == mask {
                return Ok(None);
            }
            let seed = ProjectGraphSeedFrom::Tracks(seeds);
            graph.node_field(node, GraphNodeField::SeedFrom(Some(seed)), false)
        }
        "seed-on-reset" => {
            let energy = value.from(0.0)?;
            let (node, current) = resolved(map)?;
            if current.seed_on_reset == energy {
                return Ok(None);
            }
            graph.node_field(node, GraphNodeField::SeedOnReset(Some(energy)), true)
        }
        "group" => {
            let max = sequencer::graph::NEURAL_GROUP_MAX as usize - 1;
            let neural_group = value.integer(0, max)? as u8;
            let (node, current) = resolved(map)?;
            if current.neural_group == neural_group {
                return Ok(None);
            }
            graph.node_field(node, GraphNodeField::Group(Some(neural_group)), false)
        }
        "param" => {
            let node = graph.node(map)?;
            let (manifest, config) = (&graph.manifest, &graph.config);
            param_request(
                &value,
                map,
                ("node", &manifest.node.params),
                |name| sequencer::lisp_host::graph_node_param_value(manifest, config, node, name),
                |param, value| GraphOverrideSlot::NodeParam {
                    group: manifest.node.name.clone(),
                    instance: node,
                    param,
                    value: Some(value),
                },
            )
        }
        "edge-param" => {
            let from = graph.node(map)?;
            let to = SetValue::of(map, "to", "to").integer(0, usize::MAX)?;
            let edge = (graph.config.edges.iter())
                .find(|edge| edge.from == from && edge.to == to)
                .ok_or("the edge is gone")?;
            let set = (graph.manifest.edge_sets.first()).ok_or("the graph has no edges")?;
            param_request(
                &value,
                map,
                ("edge", &set.params),
                |name| sequencer::lisp_host::graph_edge_param_value(edge, name),
                |param, value| GraphOverrideSlot::EdgeParam {
                    group: sequencer::graph::edge_set_group_id(set),
                    from,
                    to,
                    param,
                    value: Some(value),
                },
            )
        }
        field => config_request(graph, field, &value, map),
    }
}

/// A sequencer-level field's request (see [`graph_request`]).
fn config_request(graph: &Graph, field: &str, value: &SetValue<'_>, map: &Payload) -> Request {
    let current = |field| {
        let count = graph.config.nodes.len();
        let overrides = graph.overrides.as_ref();
        let value = sequencer::lisp_host::graph_config_field_value(
            &graph.manifest,
            overrides,
            || count,
            field,
        );
        value.ok()
    };
    let number = |n: f64| Some(Value::Number(n));
    let config = match field {
        "node-count" => {
            let (_, min, max) = (graph.manifest.shape.variable_line_bounds())
                .ok_or("node-count: the graph's node count is fixed")?;
            let count = value.integer(min, max)?;
            if current(field) == number(count as f64) {
                return Ok(None);
            }
            GraphConfigField::NodeCount(Some(count as u32))
        }
        "reset-bars" => {
            let bars = value.from(0.0)?;
            if current(field) == number(bars) {
                return Ok(None);
            }
            GraphConfigField::ResetEveryBeats(Some(bars * 4.0))
        }
        "max-poly" => {
            let poly = value.integer(0, u32::MAX as usize)?;
            if current(field) == number(poly as f64) {
                return Ok(None);
            }
            GraphConfigField::MaxPoly(Some(poly as u32))
        }
        "max-poly-selection" => {
            let names = NeuralMaxPolySelection::ALL.map(NeuralMaxPolySelection::as_str);
            let selection = NeuralMaxPolySelection::ALL[value.choice(&names)?];
            if current(field) == Some(Value::String(selection.as_str().to_string())) {
                return Ok(None);
            }
            GraphConfigField::MaxPolySelection(Some(selection))
        }
        "group-trace-decay" | "group-coupling-scale" | "group-excite-floor" => {
            let max = match field {
                "group-coupling-scale" => sequencer::graph::GROUP_COUPLING_SCALE_MAX,
                _ => 1.0,
            };
            let v = value.number(0.0, max)?;
            if current(field) == number(v) {
                return Ok(None);
            }
            match field {
                "group-trace-decay" => GraphConfigField::GroupTraceDecay(Some(v)),
                "group-coupling-scale" => GraphConfigField::GroupCouplingScale(Some(v)),
                _ => GraphConfigField::GroupExciteFloor(Some(v)),
            }
        }
        "group-gain" | "group-coupling" => {
            use sequencer::graph::{
                group_matrix_cell, GROUP_COUPLING_MAX, GROUP_COUPLING_MIN, GROUP_GAIN_MAX,
                GROUP_GAIN_MIN, NEURAL_GROUP_MAX,
            };
            let k = NEURAL_GROUP_MAX as usize;
            let row = SetValue::of(map, "row", "row").integer(0, k - 1)?;
            let col = SetValue::of(map, "col", "col").integer(0, k - 1)?;
            let (matrix, (min, max)) = if field == "group-gain" {
                (GroupMatrix::Gain, (GROUP_GAIN_MIN, GROUP_GAIN_MAX))
            } else {
                (
                    GroupMatrix::Coupling,
                    (GROUP_COUPLING_MIN, GROUP_COUPLING_MAX),
                )
            };
            let v = value.number(min, max)?;
            // Row-major: the source group's row, the target group's column.
            let index = row * k + col;
            let cells = (graph.overrides.as_ref()).and_then(|overrides| matrix.cells(overrides));
            if group_matrix_cell(cells, index, matrix.default_cell()) == v {
                return Ok(None);
            }
            let slot = GraphOverrideSlot::GroupCell {
                matrix,
                index,
                value: Some(v),
            };
            return Ok(Some((slot, true)));
        }
        other => return Err(format!("unknown field '{other}'")),
    };
    let continuous = field != "max-poly-selection";
    Ok(Some((GraphOverrideSlot::ConfigField(config), continuous)))
}

/// Apply `set-graph` as a script edit.
fn set_graph(app: &mut app::App, ctx: &mut LoopCtx<'_>, map: &Payload) -> Result<(), String> {
    let graph = Graph::of(app, map)?;
    let Some((slot, continuous)) = graph_request(app, &graph, map)? else {
        return Ok(());
    };
    super::ScriptEdit::run(app, ctx, continuous, |app| {
        app.apply_graph_override_edit(&graph.manifest, slot)
    })
    .map(|_| ())
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Value::Map(map) = &payload else {
        editor.handle_host_event(HostEvent::Error(format!(
            "{name}: the payload is not a dict"
        )));
        return;
    };
    if let Err(message) = set_graph(app, ctx, map) {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
