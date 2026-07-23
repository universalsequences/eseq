/*!
Parses a graph-mode `def-sequencer` form into a `GraphManifest`.

`graph_mode_present` detects whether a `def-sequencer` arg list carries a
`def-node` sub-form (which switches the sequencer from step mode to graph
mode), and `parse_graph_manifest` turns the full arg list — shape, node
prototype, `edges` sets, energy decay/reset/seed, max-poly, duration and
swing — into the manifest the scheduler-side runtime instantiates as a
`crate::graph` engine. Uses the value/spec parsers from the sibling
`graph_dsl` module; called from `def-sequencer` in `sequencer_natives`.
*/

use super::graph_dsl::*;
use super::super::*;
use crate::graph::{
    EdgeSetSpec, EventSelect, GraphDurationSpec, GraphManifest, GraphSwingSpec, NodeProto,
    Reduce as GraphReduce, SeedFrom, ShapeSpec, Topology,
};

fn graph_reduce(value: &EValue) -> GraphReduce {
    match graph_keyword(value).as_deref() {
        Some("max") => GraphReduce::Max,
        Some("min") => GraphReduce::Min,
        Some("product") => GraphReduce::Product,
        Some("count") => GraphReduce::Count,
        _ => GraphReduce::Sum,
    }
}

/// `:event` payload-selection policy on `def-node` (Layer A). Unknown/absent keeps the
/// historical last-writer-wins (`:newest`).
fn graph_event_select(value: &EValue) -> EventSelect {
    match graph_keyword(value).as_deref() {
        Some("loudest") => EventSelect::Loudest,
        Some("seed-priority") | Some("seed") => EventSelect::SeedPriority,
        Some("strongest") => EventSelect::Strongest,
        _ => EventSelect::Newest,
    }
}

/// `:off`/`none`/`nil` → no quantize; else a timebase.
fn graph_quantize(value: &EValue) -> Result<Option<Timebase>, String> {
    match graph_keyword(value).as_deref() {
        Some("off") | Some("none") | Some("nil") | Some("false") => Ok(None),
        _ => Ok(Some(graph_timebase(value)?)),
    }
}

/// `:route` → follow the node route; `:off`/`nil`/empty list → no track seed;
/// a number → single track; a list → track set.
fn graph_seed_from(value: &EValue) -> SeedFrom {
    match graph_keyword(value).as_deref() {
        Some("route") => return SeedFrom::Route,
        Some("off") | Some("none") | Some("nil") | Some("false") => {
            return SeedFrom::Tracks(Vec::new());
        }
        _ => {}
    }
    match value {
        EValue::Nil => SeedFrom::Tracks(Vec::new()),
        EValue::Number(n) if *n >= 0.0 => SeedFrom::Tracks(vec![*n as usize]),
        EValue::List(_) => {
            let tracks = graph_list_items(value)
                .unwrap_or_default()
                .iter()
                .filter_map(|v| graph_number(v).filter(|n| *n >= 0.0).map(|n| n as usize))
                .collect();
            SeedFrom::Tracks(tracks)
        }
        _ => SeedFrom::Route,
    }
}

fn graph_route(value: &EValue) -> Option<usize> {
    match value {
        EValue::Number(n) if *n >= 0.0 => Some(*n as usize),
        _ => None,
    }
}

fn graph_parse_node_proto(items: &[EValue]) -> Result<NodeProto, String> {
    let name = match items.get(1) {
        Some(EValue::Symbol(s) | EValue::String(s)) => s.clone(),
        _ => return Err("def-node expects a name".to_string()),
    };
    let mut proto = NodeProto {
        name,
        ..NodeProto::default()
    };
    let mut i = 2;
    while i < items.len() {
        let Some(key) = graph_keyword(&items[i]) else {
            return Err("def-node expects keyword/value pairs".to_string());
        };
        i += 1;
        let Some(value) = items.get(i) else {
            return Err(format!("def-node missing value for :{key}"));
        };
        match key.as_str() {
            "resolution" | "res" => proto.resolution = graph_timebase(value)?,
            "delay" | "delay-steps" => {
                proto.delay_steps = graph_number(value).unwrap_or(0.0).max(0.0) as u32
            }
            "quantize" | "q" => proto.quantize = graph_quantize(value)?,
            "route" => proto.route = graph_route(value),
            "seed-from" => proto.seed_from = graph_seed_from(value),
            "duration" | "dur" => proto.duration = Some(graph_parse_duration_spec(value)?),
            "swing" => proto.swing = Some(graph_parse_swing_spec(value)?),
            "reduce" => proto.reduce = graph_reduce(value),
            "event" | "event-select" => proto.event_select = graph_event_select(value),
            "params" => proto.params = graph_parse_param_list(value),
            "state" => proto.state = graph_parse_state_list(value),
            "update" => proto.update_source = Some(eseqlisp::vm::format_lisp_source(value)),
            _ => return Err(format!("def-node unknown key :{key}")),
        }
        i += 1;
    }
    Ok(proto)
}

fn graph_parse_edge_set(items: &[EValue]) -> Result<EdgeSetSpec, String> {
    let mut set = EdgeSetSpec {
        from: String::new(),
        to: String::new(),
        topology: Topology::AllToAll,
        distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
        gather_source: None,
        params: Vec::new(),
    };
    let mut i = 1;
    while i < items.len() {
        let Some(key) = graph_keyword(&items[i]) else {
            return Err("edges expects keyword/value pairs".to_string());
        };
        i += 1;
        let Some(value) = items.get(i) else {
            return Err(format!("edges missing value for :{key}"));
        };
        match key.as_str() {
            "from" => set.from = graph_symbol_string(value),
            "to" => set.to = graph_symbol_string(value),
            "topology" => set.topology = graph_parse_topology(value)?,
            "distribution" | "scatter" => set.distribution = graph_edge_distribution(value)?,
            "gather" => set.gather_source = Some(eseqlisp::vm::format_lisp_source(value)),
            "params" => set.params = graph_parse_param_list(value),
            _ => return Err(format!("edges unknown key :{key}")),
        }
        i += 1;
    }
    Ok(set)
}

/// True if these `def-sequencer` args carry a `def-node` sub-form (graph mode).
pub fn graph_mode_present(args: &[EValue]) -> bool {
    args.iter().any(|arg| {
        graph_list_items(arg)
            .map(|items| graph_head_symbol(&items).as_deref() == Some("def-node"))
            .unwrap_or(false)
    })
}

/// Parse a graph-mode `def-sequencer` arg list (including the leading name) into a
/// [`GraphManifest`].
pub fn parse_graph_manifest(args: &[EValue]) -> Result<GraphManifest, String> {
    let name = match args.first() {
        Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => {
            s.trim_start_matches('@').to_string()
        }
        _ => return Err("def-sequencer expects a name".to_string()),
    };
    let id = stable_sequencer_id(&name);
    let mut shape: Option<ShapeSpec> = None;
    let mut energy_decay = 0.9;
    let mut reset_every_beats = 0.0;
    let mut seed_on_reset = 0.0;
    let mut max_poly = 0u32;
    let mut max_poly_selection = NeuralMaxPolySelection::Deterministic;
    let mut duration = GraphDurationSpec::default();
    let mut swing = GraphSwingSpec::default();
    let mut node: Option<NodeProto> = None;
    let mut edge_sets: Vec<EdgeSetSpec> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        // `def-node` / `edges` sub-forms (positional list data).
        if let Some(items) = graph_list_items(&args[i]) {
            match graph_head_symbol(&items).as_deref() {
                Some("def-node") => {
                    if node.is_some() {
                        return Err(
                            "graph-mode def-sequencer allows one def-node in v1".to_string()
                        );
                    }
                    node = Some(graph_parse_node_proto(&items)?);
                    i += 1;
                    continue;
                }
                Some("edges") => {
                    edge_sets.push(graph_parse_edge_set(&items)?);
                    i += 1;
                    continue;
                }
                _ => {}
            }
        }
        // keyword/value config.
        let Some(key) = graph_keyword(&args[i]) else {
            return Err(format!(
                "graph-mode def-sequencer: unexpected form at position {i}"
            ));
        };
        i += 1;
        let Some(value) = args.get(i) else {
            return Err(format!("def-sequencer missing value for :{key}"));
        };
        match key.as_str() {
            "shape" => shape = Some(graph_parse_shape(value)?),
            "energy-decay" => energy_decay = graph_number(value).unwrap_or(0.9),
            "reset-every" => reset_every_beats = graph_bars_or_beats(value),
            "seed-on-reset" => seed_on_reset = graph_number(value).unwrap_or(0.0),
            "max-poly" => max_poly = graph_number(value).unwrap_or(0.0).max(0.0) as u32,
            "max-poly-selection" => max_poly_selection = parse_neural_max_poly_selection(value)?,
            "duration" | "dur" => duration = graph_parse_duration_spec(value)?,
            "swing" => swing = graph_parse_swing_spec(value)?,
            // Resolution is per-node, not sequencer-level.
            "resolution" | "res" => {}
            _ => return Err(format!("graph-mode def-sequencer unknown key :{key}")),
        }
        i += 1;
    }

    let shape = shape.ok_or_else(|| "graph-mode def-sequencer requires :shape".to_string())?;
    let node = node.ok_or_else(|| "graph-mode def-sequencer requires a def-node".to_string())?;
    Ok(GraphManifest {
        id,
        name,
        shape,
        energy_decay,
        reset_every_beats,
        seed_on_reset,
        max_poly,
        max_poly_selection,
        duration,
        swing,
        node,
        edge_sets,
    })
}
