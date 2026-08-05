/*!
Registers the authoring/UI-side natives for graph-mode sequencers.

`register_graph_authoring_natives` installs the GRAPH reactive namespace (a
writable mirror of resolved graph values that `reactive-set` dirties) plus the
natives a graph-editing UI script uses: reactive bindings (`bind-graph`,
`bind-graph-edge`, `bind-graph-config`, `graph-key`/`graph-edge-key`/
`graph-config-key`), inspection (`graph-list`, `graph-describe`, `graph-node`,
`graph-param`, `graph-edge`, `graph-config`), and the duration/swing spec
forms (`steps`, `beats`, `swing`, `delay`, `seed`). Spec values are parsed
with the helpers in the sibling `graph_dsl` module; the runtime node `:update`
natives live in `graph_update`.
*/

use super::graph_dsl::*;
use super::super::*;

pub fn register_graph_authoring_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
) {
    // Writable mirror of resolved graph values; `bind-graph` reads it, `reactive-set`
    // dirties it. Dynamic-field namespace (no declared fields), like SEQV.
    runtime.register_reactive(GRAPH_REACTIVE_NS, vec![], true);

    runtime.register_native_with_docs(
        "steps",
        "(steps n)",
        "Graph duration form for n steps of the edited node's resolution.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("steps expects exactly one numeric step count".to_string());
            }
            let Some(EValue::Number(n)) = args.first() else {
                return Err("steps expects a numeric step count".to_string());
            };
            Ok(lisp_list(vec![
                EValue::Symbol("steps".to_string()),
                EValue::Number(n.max(0.0)),
            ]))
        },
    );
    runtime.register_native_with_docs(
        "delay",
        "(delay)",
        "Graph duration form for the edited node's propagation delay.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("delay expects no arguments".to_string());
            }
            Ok(lisp_list(vec![EValue::Symbol("delay".to_string())]))
        },
    );
    runtime.register_native_with_docs(
        "seed",
        "(seed)",
        "Graph duration form that inherits the incoming seed/payload duration.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("seed expects no arguments".to_string());
            }
            Ok(EValue::Keyword("seed".to_string()))
        },
    );
    runtime.register_native_with_docs(
        "beats",
        "(beats :16) | (beats 0.25)",
        "Graph duration helper for a fixed beat duration.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("beats expects exactly one number or timebase".to_string());
            }
            if let Some(EValue::Number(n)) = args.first() {
                return Ok(EValue::Number(n.max(0.0)));
            }
            let timebase = parse_timebase_arg(&args, 0)?;
            Ok(EValue::Number(
                timebase.step_beats(crate::graph::GRAPH_RESOLUTION_REF_STEPS),
            ))
        },
    );
    runtime.register_native_with_docs(
        "swing",
        "(swing pct [:16|:8|:4|:2])",
        "Graph swing timing policy: 50 is straight, 75 is maximum delayed swing.",
        move |args, _ctx| graph_swing_value_from_args(&args),
    );

    let state_for_graph_list = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-list",
        "(graph-list)",
        "Return graph-mode sequencer definitions with current-pattern overrides.",
        move |_args, _ctx| {
            Ok(lisp_list(
                state_for_graph_list
                    .published_sequencers()
                    .into_iter()
                    .filter_map(|published| published.graph)
                    .map(|manifest| {
                        let graph_overrides = state_for_graph_list.current_graph_overrides();
                        let overrides = graph_overrides_for_manifest(&graph_overrides, &manifest);
                        graph_manifest_to_value(&manifest, overrides)
                    })
                    .collect(),
            ))
        },
    );

    let state_for_graph_describe = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-describe",
        "(graph-describe id-or-name)",
        "Return one graph-mode sequencer definition.",
        move |args, _ctx| {
            let reference = args
                .first()
                .ok_or_else(|| "graph-describe expects graph id or name".to_string())?;
            let manifest = resolve_graph_manifest(&state_for_graph_describe, reference)?;
            let graph_overrides = state_for_graph_describe.current_graph_overrides();
            let overrides = graph_overrides_for_manifest(&graph_overrides, &manifest);
            Ok(graph_manifest_to_value(&manifest, overrides))
        },
    );

    let state_for_graph_node_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-value",
        "(graph-node-value sequencer node-index :delay)",
        "Return one resolved current-pattern graph node intrinsic value.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err(
                    "graph-node-value expects graph id/name, node index, and field".to_string(),
                );
            }
            let manifest = resolve_graph_manifest(&state_for_graph_node_value, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            let field = graph_key_string(&args[2])
                .ok_or_else(|| "graph-node-value expects a field name".to_string())?;
            resolved_graph_node_value(&state_for_graph_node_value, &manifest, instance, &field)
        },
    );

    let state_for_graph_param_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-param-value",
        "(graph-param-value sequencer node-index :threshold)",
        "Return one resolved current-pattern graph node param value.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err(
                    "graph-param-value expects graph id/name, node index, and param".to_string(),
                );
            }
            let manifest = resolve_graph_manifest(&state_for_graph_param_value, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            let param = graph_key_string(&args[2])
                .ok_or_else(|| "graph-param-value expects a param name".to_string())?;
            resolved_graph_param_value(&state_for_graph_param_value, &manifest, instance, &param)
        },
    );

    let state_for_graph_edge_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-edge-value",
        "(graph-edge-value sequencer :from 0 :to 1 :weight)",
        "Return one resolved current-pattern graph edge param value.",
        move |args, _ctx| {
            if args.len() < 4 {
                return Err(
                    "graph-edge-value expects graph, from/to coordinates, and param".to_string(),
                );
            }
            let manifest = resolve_graph_manifest(&state_for_graph_edge_value, &args[0])?;
            let query = parse_graph_edge_query(&manifest, &args[1..])?;
            resolved_graph_edge_value(&state_for_graph_edge_value, &manifest, query)
        },
    );

    let state_for_graph_node = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node",
        "(graph-node sequencer node-index :delay 2 :route 0 :seed-from 1)",
        "Set sparse per-pattern graph node intrinsic overrides.",
        move |args, ctx| {
            if args.len() < 2 {
                return Err("graph-node expects graph id/name and node index".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_node, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            if instance >= graph_capacity_node_count(&manifest) {
                return Err("graph-node node index out of range".to_string());
            }
            let edit = parse_graph_node_edit(&args[2..])?;
            let sequencer_name = manifest.name.clone();
            state_for_graph_node.edit_current_graph_overrides(|graphs| {
                let graph = ensure_graph_overrides(graphs, &manifest);
                let node = ensure_graph_node_intrinsic(graph, &manifest.node.name, instance);
                apply_graph_node_edit(node, edit);
                Ok(())
            })?;
            ctx.set_status(format!("updated graph '{sequencer_name}' node {instance}"));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_graph_param = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-param",
        "(graph-param sequencer node-index :threshold 0.75)",
        "Set one sparse per-pattern graph node param override.",
        move |args, ctx| {
            if args.len() != 4 {
                return Err("graph-param expects graph, node index, param, value".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_param, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            if instance >= graph_capacity_node_count(&manifest) {
                return Err("graph-param node index out of range".to_string());
            }
            let param = graph_key_string(&args[2]).ok_or("graph-param expects param name")?;
            let value = graph_number(&args[3]).ok_or("graph-param value must be numeric")?;
            let sequencer_name = manifest.name.clone();
            state_for_graph_param.edit_current_graph_overrides(|graphs| {
                let graph = ensure_graph_overrides(graphs, &manifest);
                upsert_graph_node_param(graph, &manifest.node.name, instance, &param, value);
                Ok(())
            })?;
            ctx.set_status(format!(
                "updated graph '{sequencer_name}' node {instance} param {param}"
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_graph_edge = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-edge",
        "(graph-edge sequencer :from 0 :to 1 :weight 0.5)",
        "Set one sparse per-pattern graph edge param override.",
        move |args, ctx| {
            if args.len() < 6 {
                return Err("graph-edge expects graph, :from, :to, and a param".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_edge, &args[0])?;
            let edit = parse_graph_edge_edit(&manifest, &args[1..])?;
            let sequencer_name = manifest.name.clone();
            let param_name = edit.param.clone();
            state_for_graph_edge.edit_current_graph_overrides(|graphs| {
                let graph = ensure_graph_overrides(graphs, &manifest);
                upsert_graph_edge_param(graph, edit);
                Ok(())
            })?;
            ctx.set_status(format!(
                "updated graph '{sequencer_name}' edge {param_name}"
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_bind_graph = Arc::clone(&state);
    let bindings_for_bind_graph = runtime.reactive_binding_store();
    runtime.register_native_with_docs(
        "bind-graph",
        "(bind-graph sequencer node-index :delay [options])",
        "Reactive handle to a graph node param/intrinsic, seeded with the resolved \
         current-pattern value. Numeric fields bind directly; pass an options list to \
         bind an enum field (route/resolution/quantize) as a dropdown index.",
        move |args, _ctx| {
            if args.len() < 3 {
                return Err("bind-graph expects graph, node index, and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_bind_graph, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            if instance >= graph_active_node_count(&state_for_bind_graph, &manifest) {
                return Err("bind-graph node index out of range".to_string());
            }
            let field = graph_key_string(&args[2])
                .ok_or_else(|| "bind-graph expects a field name".to_string())?;
            let value = match args.get(3) {
                Some(options) => {
                    let display = graph_node_display_value(
                        &state_for_bind_graph,
                        &manifest,
                        instance,
                        &field,
                    )?;
                    graph_option_index(options, &display)
                }
                None => {
                    graph_node_numeric_value(&state_for_bind_graph, &manifest, instance, &field)?
                }
            };
            Ok(graph_seeded_reactive_ref(
                &bindings_for_bind_graph,
                graph_node_reactive_field(manifest.id, instance, &field),
                value,
            ))
        },
    );

    let state_for_graph_key = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-key",
        "(graph-key sequencer node-index :delay)",
        "Canonical GRAPH reactive field name for a node field. Use with \
         `(reactive-set \"GRAPH\" (graph-key ...) value)` to dirty a `bind-graph` handle.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err("graph-key expects graph, node index, and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_key, &args[0])?;
            let instance = parse_nonnegative_usize(&args[1], "node index")?;
            let field = graph_key_string(&args[2])
                .ok_or_else(|| "graph-key expects a field name".to_string())?;
            Ok(EValue::String(graph_node_reactive_field(
                manifest.id,
                instance,
                &field,
            )))
        },
    );

    let state_for_bind_graph_edge = Arc::clone(&state);
    let bindings_for_bind_graph_edge = runtime.reactive_binding_store();
    runtime.register_native_with_docs(
        "bind-graph-edge",
        "(bind-graph-edge sequencer from to :weight)",
        "Reactive handle to a graph edge param (weight/dampening/delay), seeded with \
         the resolved current-pattern value.",
        move |args, _ctx| {
            if args.len() != 4 {
                return Err("bind-graph-edge expects graph, from, to, and a param".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_bind_graph_edge, &args[0])?;
            let from = parse_nonnegative_usize(&args[1], "from")?;
            let to = parse_nonnegative_usize(&args[2], "to")?;
            let param = graph_key_string(&args[3])
                .ok_or_else(|| "bind-graph-edge expects a param name".to_string())?;
            let edge_set = manifest
                .edge_sets
                .first()
                .ok_or_else(|| "bind-graph-edge requires an edge set".to_string())?;
            let active_nodes = graph_active_node_count(&state_for_bind_graph_edge, &manifest);
            if from >= active_nodes || to >= active_nodes {
                return Err("bind-graph-edge from/to index out of range".to_string());
            }
            let query = GraphEdgeQuery {
                group: crate::graph::edge_set_group_id(edge_set),
                from,
                to,
                param: param.clone(),
            };
            let value = graph_number(&resolved_graph_edge_value(
                &state_for_bind_graph_edge,
                &manifest,
                query,
            )?)
            .ok_or_else(|| format!("bind-graph-edge param :{param} is not numeric"))?;
            Ok(graph_seeded_reactive_ref(
                &bindings_for_bind_graph_edge,
                graph_edge_reactive_field(manifest.id, from, to, &param),
                value,
            ))
        },
    );

    let state_for_graph_edge_key = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-edge-key",
        "(graph-edge-key sequencer from to :weight)",
        "Canonical GRAPH reactive field name for an edge param.",
        move |args, _ctx| {
            if args.len() != 4 {
                return Err("graph-edge-key expects graph, from, to, and a param".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_edge_key, &args[0])?;
            let from = parse_nonnegative_usize(&args[1], "from")?;
            let to = parse_nonnegative_usize(&args[2], "to")?;
            let param = graph_key_string(&args[3])
                .ok_or_else(|| "graph-edge-key expects a param name".to_string())?;
            Ok(EValue::String(graph_edge_reactive_field(
                manifest.id,
                from,
                to,
                &param,
            )))
        },
    );

    let state_for_graph_config_value = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-config-value",
        "(graph-config-value sequencer :reset-bars)",
        "Resolved sequencer-level config (:reset-bars, :max-poly, :max-poly-selection, or :node-count), override-or-manifest.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("graph-config-value expects graph and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_config_value, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "graph-config-value expects a field name".to_string())?;
            resolved_graph_config_value(&state_for_graph_config_value, &manifest, &field)
        },
    );

    let state_for_graph_config = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-config",
        "(graph-config sequencer :reset-bars 4)",
        "Set a sequencer-level config override (:reset-bars in bars, :max-poly, :max-poly-selection, or :node-count).",
        move |args, ctx| {
            if args.len() != 3 {
                return Err("graph-config expects graph, field, value".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_config, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "graph-config expects a field name".to_string())?;
            let sequencer_name = manifest.name.clone();
            set_graph_config_value(&state_for_graph_config, &manifest, &field, &args[2])?;
            ctx.set_status(format!("updated graph '{sequencer_name}' config {field}"));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_graph_config_key = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-config-key",
        "(graph-config-key sequencer :reset-bars)",
        "Canonical GRAPH reactive field name for a sequencer-level config field.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("graph-config-key expects graph and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_graph_config_key, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "graph-config-key expects a field name".to_string())?;
            Ok(EValue::String(graph_config_reactive_field(
                manifest.id,
                &field,
            )))
        },
    );

    let state_for_bind_graph_config = Arc::clone(&state);
    let bindings_for_bind_graph_config = runtime.reactive_binding_store();
    runtime.register_native_with_docs(
        "bind-graph-config",
        "(bind-graph-config sequencer :reset-bars [options])",
        "Reactive handle to a sequencer-level config field, seeded with the resolved value. Pass an options list to bind enum fields as dropdown indices.",
        move |args, _ctx| {
            if args.len() < 2 {
                return Err("bind-graph-config expects graph and field".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_bind_graph_config, &args[0])?;
            let field = graph_key_string(&args[1])
                .ok_or_else(|| "bind-graph-config expects a field name".to_string())?;
            let value = match args.get(2) {
                Some(options) => {
                    let display = graph_config_display_value(
                        &state_for_bind_graph_config,
                        &manifest,
                        &field,
                    )?;
                    graph_option_index(options, &display)
                }
                None => {
                    graph_config_numeric_value(&state_for_bind_graph_config, &manifest, &field)?
                }
            };
            Ok(graph_seeded_reactive_ref(
                &bindings_for_bind_graph_config,
                graph_config_reactive_field(manifest.id, &field),
                value,
            ))
        },
    );
}

#[derive(Default)]
struct GraphNodeEdit {
    resolution: Option<Vec<u8>>,
    delay_steps: Option<u32>,
    quantize: Option<crate::graph::ProjectGraphQuantizeOverride>,
    route: Option<crate::graph::ProjectGraphRouteOverride>,
    seed_from: Option<crate::graph::ProjectGraphSeedFrom>,
    seed_on_reset: Option<f64>,
    duration: Option<crate::graph::GraphDurationSpec>,
    swing: Option<crate::graph::GraphSwingSpec>,
    neural_group: Option<u8>,
}

struct GraphEdgeEdit {
    group: String,
    from: usize,
    to: usize,
    param: String,
    value: f64,
}

struct GraphEdgeQuery {
    group: String,
    from: usize,
    to: usize,
    param: String,
}

fn graph_key_string(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(k) | EValue::Symbol(k) | EValue::String(k) => Some(
            k.trim_start_matches(':')
                .trim_start_matches('@')
                .to_string(),
        ),
        _ => None,
    }
}

fn resolved_graph_overrides_for_manifest(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> Option<crate::graph::ProjectGraphOverrides> {
    state
        .current_graph_overrides()
        .into_iter()
        .find(|overrides| {
            overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
        })
}

fn graph_runtime_config_for_current_pattern(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> crate::graph::GraphRuntimeConfig {
    let graph_overrides = resolved_graph_overrides_for_manifest(state, manifest);
    manifest.runtime_config_with_overrides(graph_overrides.as_ref())
}

fn graph_active_node_count(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> usize {
    cached_graph_runtime_config(state, manifest).nodes.len()
}

fn graph_capacity_node_count(manifest: &crate::graph::GraphManifest) -> usize {
    manifest.shape.capacity_num_nodes()
}

/// Reactive namespace that mirrors resolved graph node/edge values so the UI can
/// bind widgets directly (`bind-graph`) instead of shadowing every knob in a
/// per-node `defstate`. Writes flow back via `reactive-set` + `graph-*` setters.
const GRAPH_REACTIVE_NS: &str = "GRAPH";

struct GraphConfigCacheEntry {
    manifest_id: u64,
    pattern: usize,
    snapshot_version: u64,
    published_version: u64,
    config: Rc<crate::graph::GraphRuntimeConfig>,
}

thread_local! {
    // Materializing the runtime config locks the pattern bank, clones the override
    // vec, and allocates a HashMap per node. A single panel render resolves dozens
    // of node/edge values at the same (pattern, version); memoize so the whole
    // render collapses to one materialization. Any edit bumps snapshot_version and
    // invalidates the entry, so reads can never observe a stale config.
    static GRAPH_CONFIG_CACHE: RefCell<Option<GraphConfigCacheEntry>> = const { RefCell::new(None) };
}

fn cached_graph_runtime_config(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> Rc<crate::graph::GraphRuntimeConfig> {
    let pattern = state.current_pattern_index();
    let snapshot_version = state.scheduler_snapshot_version();
    let published_version = state.published_sequencers_version();
    GRAPH_CONFIG_CACHE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(entry) = slot.as_ref() {
            if entry.manifest_id == manifest.id
                && entry.pattern == pattern
                && entry.snapshot_version == snapshot_version
                && entry.published_version == published_version
            {
                return Rc::clone(&entry.config);
            }
        }
        let config = Rc::new(graph_runtime_config_for_current_pattern(state, manifest));
        *slot = Some(GraphConfigCacheEntry {
            manifest_id: manifest.id,
            pattern,
            snapshot_version,
            published_version,
            config: Rc::clone(&config),
        });
        config
    })
}

fn graph_node_reactive_field(manifest_id: u64, instance: usize, field: &str) -> String {
    format!("{manifest_id}|n{instance}|{field}")
}

fn graph_edge_reactive_field(manifest_id: u64, from: usize, to: usize, param: &str) -> String {
    format!("{manifest_id}|e{from}_{to}|{param}")
}

/// Seed the GRAPH reactive slot with `value` (a plain float write that does NOT
/// dirty bound widgets — safe to call during render) and return a reactive handle
/// pointing at the same slot. Re-running the producing lisp on a pattern switch
/// re-seeds the slot; live edits keep it current via `reactive-set`.
fn graph_seeded_reactive_ref(
    bindings: &eseqlisp::reactive::ReactiveBindingStore,
    field: String,
    value: f64,
) -> EValue {
    bindings.seeded_float_ref(GRAPH_REACTIVE_NS, field, value)
}

/// Resolve a node field to a single float for `bind-graph`. `delay` is an
/// intrinsic; everything else falls through to behavioral params. Enum intrinsics
/// (route/resolution/quantize) are not scalars — callers must pass an options list
/// and go through the index path instead.
fn graph_node_numeric_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    field: &str,
) -> Result<f64, String> {
    match field {
        "delay" | "delay-steps" | "seed-on-reset" | "reset-seed" | "seed-route"
        | "seed-from-route" | "group" | "grp" => {
            let value = resolved_graph_node_value(state, manifest, instance, field)?;
            graph_number(&value).ok_or_else(|| format!("bind-graph field :{field} is not numeric"))
        }
        "resolution" | "res" | "quantize" | "q" | "route" | "seed-from" => Err(format!(
            "bind-graph field :{field} is an enum; pass an options list to bind its index"
        )),
        _ => {
            let value = resolved_graph_param_value(state, manifest, instance, field)?;
            graph_number(&value).ok_or_else(|| format!("bind-graph param :{field} is not numeric"))
        }
    }
}

/// Render an enum node field to the label a dropdown would display, so it can be
/// matched against the author's options list. Centralizes the route/timebase
/// formatting that the lisp demo used to spell out as nested `if` ladders.
fn graph_node_display_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    field: &str,
) -> Result<String, String> {
    let value = resolved_graph_node_value(state, manifest, instance, field)?;
    Ok(match field {
        "route" => match value {
            EValue::Number(track) => format!("Track {}", track as usize + 1),
            _ => "Off".to_string(),
        },
        _ => match value {
            EValue::String(label) => label,
            EValue::Nil => "off".to_string(),
            EValue::Number(number) => graph_format_number(number),
            other => eseqlisp::vm::format_lisp_value(&other),
        },
    })
}

fn graph_format_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

fn graph_option_index(options: &EValue, display: &str) -> f64 {
    if let EValue::List(items) = options {
        for (index, item) in items.iter().enumerate() {
            let item = item.borrow();
            let matches = match &*item {
                EValue::String(label) => label == display,
                other => eseqlisp::vm::format_lisp_value(other) == display,
            };
            if matches {
                return index as f64;
            }
        }
    }
    0.0
}

/// Beats per bar for the demo's 4/4 reset clock, matching `graph_bars_or_beats`'s
/// `(bars n) -> n * 4` parse.
const GRAPH_BEATS_PER_BAR: f64 = 4.0;

fn graph_config_reactive_field(manifest_id: u64, field: &str) -> String {
    format!("{manifest_id}|cfg|{field}")
}

/// Resolve a sequencer-level config field (override-or-manifest) to a UI value.
/// `:reset-bars` reports bars (engine stores beats); `:max-poly` reports the cap;
/// `:max-poly-selection` reports the engine enum name; `:node-count` reports the
/// resolved active count for variable line shapes.
fn resolved_graph_config_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    field: &str,
) -> Result<EValue, String> {
    let overrides = resolved_graph_overrides_for_manifest(state, manifest);
    match field {
        "reset-bars" | "reset-every-bars" => {
            let beats = overrides
                .as_ref()
                .and_then(|o| o.reset_every_beats)
                .unwrap_or(manifest.reset_every_beats);
            Ok(EValue::Number(beats / GRAPH_BEATS_PER_BAR))
        }
        "max-poly" => {
            let value = overrides
                .as_ref()
                .and_then(|o| o.max_poly)
                .unwrap_or(manifest.max_poly);
            Ok(EValue::Number(value as f64))
        }
        "max-poly-selection" | "max-poly-mode" | "poly-selection" | "poly-mode" => {
            let value = overrides
                .as_ref()
                .and_then(|o| o.max_poly_selection)
                .unwrap_or(manifest.max_poly_selection);
            Ok(EValue::String(value.as_str().to_string()))
        }
        "node-count" => {
            if !manifest.shape.is_variable_line() {
                return Err("graph config :node-count requires a variable line shape".to_string());
            }
            Ok(EValue::Number(
                manifest.shape.resolved_node_count(overrides.as_ref()) as f64,
            ))
        }
        other => Err(format!("graph config unknown field :{other}")),
    }
}

fn graph_config_numeric_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    field: &str,
) -> Result<f64, String> {
    let value = resolved_graph_config_value(state, manifest, field)?;
    graph_number(&value)
        .ok_or_else(|| format!("bind-graph-config field :{field} is an enum; pass an options list"))
}

fn graph_config_display_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    field: &str,
) -> Result<String, String> {
    let value = resolved_graph_config_value(state, manifest, field)?;
    Ok(match value {
        EValue::String(label) => label,
        EValue::Number(number) => graph_format_number(number),
        EValue::Nil => "off".to_string(),
        other => eseqlisp::vm::format_lisp_value(&other),
    })
}

fn clamp_graph_node_count(
    manifest: &crate::graph::GraphManifest,
    value: f64,
) -> Result<u32, String> {
    let Some((_default, min, max)) = manifest.shape.variable_line_bounds() else {
        return Err("graph config :node-count requires a variable line shape".to_string());
    };
    if !value.is_finite() {
        return Err("graph config :node-count expects a finite value".to_string());
    }
    Ok((value.round() as i64).clamp(min as i64, max as i64) as u32)
}

fn set_graph_config_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    field: &str,
    value: &EValue,
) -> Result<(), String> {
    enum ConfigEdit {
        ResetEveryBeats(f64),
        MaxPoly(u32),
        MaxPolySelection(NeuralMaxPolySelection),
        NodeCount(u32),
    }

    let edit = match field {
        "reset-bars" | "reset-every-bars" => {
            let value = graph_number(value)
                .ok_or_else(|| "graph config :reset-bars expects a numeric value".to_string())?;
            ConfigEdit::ResetEveryBeats((value * GRAPH_BEATS_PER_BAR).max(0.0))
        }
        "max-poly" => {
            let value = graph_number(value)
                .ok_or_else(|| "graph config :max-poly expects a numeric value".to_string())?;
            ConfigEdit::MaxPoly(value.max(0.0).round() as u32)
        }
        "max-poly-selection" | "max-poly-mode" | "poly-selection" | "poly-mode" => {
            ConfigEdit::MaxPolySelection(parse_neural_max_poly_selection(value)?)
        }
        "node-count" => {
            let value = graph_number(value)
                .ok_or_else(|| "graph config :node-count expects a numeric value".to_string())?;
            ConfigEdit::NodeCount(clamp_graph_node_count(manifest, value)?)
        }
        other => return Err(format!("graph config unknown field :{other}")),
    };

    state.edit_current_graph_overrides(|graphs| {
        let graph = ensure_graph_overrides(graphs, manifest);
        match edit {
            ConfigEdit::ResetEveryBeats(value) => graph.reset_every_beats = Some(value),
            ConfigEdit::MaxPoly(value) => graph.max_poly = Some(value),
            ConfigEdit::MaxPolySelection(value) => graph.max_poly_selection = Some(value),
            ConfigEdit::NodeCount(value) => graph.node_count = Some(value),
        }
        Ok(())
    })
}

fn graph_timebase_value(timebase: crate::sequencer::Timebase) -> EValue {
    EValue::String(timebase.label().to_string())
}

fn graph_route_value(route: Option<usize>) -> EValue {
    route
        .map(|track| EValue::Number(track as f64))
        .unwrap_or(EValue::Nil)
}

fn graph_seed_from_value(mask: u128) -> EValue {
    lisp_list(
        (0..128)
            .filter(|track| mask & (1_u128 << track) != 0)
            .map(|track| EValue::Number(track as f64))
            .collect(),
    )
}

fn resolved_graph_seed_from_route(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
) -> bool {
    let mut seed_from = crate::graph::ProjectGraphSeedFrom::from(&manifest.node.seed_from);
    if let Some(overrides) = resolved_graph_overrides_for_manifest(state, manifest) {
        for intrinsic in overrides.node_intrinsics.iter().filter(|intrinsic| {
            intrinsic.group == manifest.node.name && intrinsic.instance == instance
        }) {
            if let Some(value) = &intrinsic.seed_from {
                seed_from = value.clone();
            }
        }
    }
    matches!(seed_from, crate::graph::ProjectGraphSeedFrom::Route)
}

fn resolved_graph_node_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    field: &str,
) -> Result<EValue, String> {
    let config = cached_graph_runtime_config(state, manifest);
    let node = config
        .nodes
        .get(instance)
        .ok_or_else(|| "graph-node-value node index out of range".to_string())?;
    match field {
        "resolution" | "res" => Ok(graph_timebase_value(node.resolution)),
        // Round-robin cycle serialized as a space-separated mini-notation string, e.g.
        // "16 16 16 16 16 4" — the canonical text form the text-input widget round-trips.
        "resolution-cycle" | "res-cycle" => Ok(EValue::String(
            node.resolution_cycle
                .iter()
                .map(|tb| tb.label().to_string())
                .collect::<Vec<_>>()
                .join(" "),
        )),
        "delay" | "delay-steps" => Ok(EValue::Number(node.delay_steps as f64)),
        "quantize" | "q" => Ok(node
            .quantize
            .map(graph_timebase_value)
            .unwrap_or_else(|| EValue::String("off".to_string()))),
        "quantize-cycle" | "q-cycle" => Ok(EValue::String(
            node.quantize_cycle
                .iter()
                .map(|slot| match slot {
                    Some(tb) => tb.label().to_string(),
                    None => "off".to_string(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        )),
        "route" => Ok(graph_route_value(node.route)),
        "seed-from" => Ok(graph_seed_from_value(node.seed_track_mask)),
        "seed-route" | "seed-from-route" => Ok(EValue::Number(
            if resolved_graph_seed_from_route(state, manifest, instance) {
                1.0
            } else {
                0.0
            },
        )),
        "seed-on-reset" | "reset-seed" => Ok(EValue::Number(node.seed_on_reset)),
        "group" | "grp" => Ok(EValue::Number(node.neural_group as f64)),
        other => Err(format!("graph-node-value unknown field :{other}")),
    }
}

fn resolved_graph_param_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    param: &str,
) -> Result<EValue, String> {
    let config = cached_graph_runtime_config(state, manifest);
    let params = config
        .node_params
        .get(instance)
        .ok_or_else(|| "graph-param-value node index out of range".to_string())?;
    params
        .get(param)
        .copied()
        .or_else(|| manifest.node.param_default(param))
        .map(EValue::Number)
        .ok_or_else(|| format!("graph-param-value unknown param :{param}"))
}

fn parse_graph_edge_query(
    manifest: &crate::graph::GraphManifest,
    args: &[EValue],
) -> Result<GraphEdgeQuery, String> {
    let edge_set = manifest
        .edge_sets
        .first()
        .ok_or_else(|| "graph-edge-value requires an edge set".to_string())?;
    let default_group = crate::graph::edge_set_group_id(edge_set);
    if args.len() == 3 {
        let from = parse_nonnegative_usize(&args[0], "from")?;
        let to = parse_nonnegative_usize(&args[1], "to")?;
        let param = graph_key_string(&args[2])
            .ok_or_else(|| "graph-edge-value expects a param name".to_string())?;
        if from >= graph_capacity_node_count(manifest) || to >= graph_capacity_node_count(manifest)
        {
            return Err("graph-edge-value from/to index out of range".to_string());
        }
        return Ok(GraphEdgeQuery {
            group: default_group,
            from,
            to,
            param,
        });
    }

    let mut group = default_group.clone();
    let mut from = None;
    let mut to = None;
    let mut param = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = graph_keyword(&args[idx])
            .ok_or_else(|| "graph-edge-value expects keyword/value pairs".to_string())?;
        idx += 1;
        match key.as_str() {
            "from" => {
                let value = args
                    .get(idx)
                    .ok_or_else(|| "graph-edge-value :from expects a value".to_string())?;
                from = Some(parse_nonnegative_usize(value, "from")?);
                idx += 1;
            }
            "to" => {
                let value = args
                    .get(idx)
                    .ok_or_else(|| "graph-edge-value :to expects a value".to_string())?;
                to = Some(parse_nonnegative_usize(value, "to")?);
                idx += 1;
            }
            "group" => {
                let value = args
                    .get(idx)
                    .ok_or_else(|| "graph-edge-value :group expects a value".to_string())?;
                group = graph_key_string(value)
                    .ok_or_else(|| "graph-edge-value :group expects a symbol/string".to_string())?;
                idx += 1;
            }
            other => {
                if param.is_some() {
                    return Err("graph-edge-value expects one param".to_string());
                }
                param = Some(other.to_string());
            }
        }
    }
    let from = from.ok_or_else(|| "graph-edge-value requires :from".to_string())?;
    let to = to.ok_or_else(|| "graph-edge-value requires :to".to_string())?;
    if group != default_group {
        return Err(format!("graph-edge-value edge group not found: {group}"));
    }
    if from >= graph_capacity_node_count(manifest) || to >= graph_capacity_node_count(manifest) {
        return Err("graph-edge-value from/to index out of range".to_string());
    }
    Ok(GraphEdgeQuery {
        group,
        from,
        to,
        param: param.ok_or_else(|| "graph-edge-value requires an edge param".to_string())?,
    })
}

fn resolved_graph_edge_value(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    query: GraphEdgeQuery,
) -> Result<EValue, String> {
    let config = cached_graph_runtime_config(state, manifest);
    let edge = config
        .edges
        .iter()
        .find(|edge| edge.from == query.from && edge.to == query.to)
        .ok_or_else(|| "graph-edge-value edge not found".to_string())?;
    match query.param.as_str() {
        "weight" => Ok(EValue::Number(edge.weight)),
        "dampening" => Ok(EValue::Number(edge.dampening)),
        "delay" | "delay-steps" => Ok(EValue::Number(edge.delay_steps as f64)),
        other => Err(format!(
            "graph-edge-value unknown edge param :{} for group {}",
            other, query.group
        )),
    }
}

fn resolve_graph_manifest(
    state: &crate::sequencer::SequencerState,
    reference: &EValue,
) -> Result<crate::graph::GraphManifest, String> {
    let published = state.published_sequencers();
    match reference {
        EValue::Number(id) if id.is_finite() && *id >= 0.0 => {
            let id = *id as u64;
            published
                .into_iter()
                .filter_map(|published| published.graph)
                .find(|manifest| manifest.id == id)
                .ok_or_else(|| "graph sequencer id not found".to_string())
        }
        EValue::String(name) | EValue::Symbol(name) | EValue::Keyword(name) => {
            let name = name.trim_start_matches('@').trim_start_matches(':');
            published
                .into_iter()
                .filter_map(|published| published.graph)
                .find(|manifest| manifest.name == name)
                .ok_or_else(|| "graph sequencer name not found".to_string())
        }
        _ => Err("graph reference must be id or name".to_string()),
    }
}

fn graph_overrides_for_manifest<'a>(
    overrides: &'a [crate::graph::ProjectGraphOverrides],
    manifest: &crate::graph::GraphManifest,
) -> Option<&'a crate::graph::ProjectGraphOverrides> {
    overrides.iter().find(|overrides| {
        overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
    })
}

fn ensure_graph_overrides<'a>(
    graphs: &'a mut Vec<crate::graph::ProjectGraphOverrides>,
    manifest: &crate::graph::GraphManifest,
) -> &'a mut crate::graph::ProjectGraphOverrides {
    if let Some(idx) = graphs.iter().position(|graph| {
        graph.sequencer_id == manifest.id || graph.sequencer_name == manifest.name
    }) {
        return &mut graphs[idx];
    }
    graphs.push(crate::graph::ProjectGraphOverrides {
        sequencer_id: manifest.id,
        sequencer_name: manifest.name.clone(),
        ..crate::graph::ProjectGraphOverrides::default()
    });
    graphs.last_mut().expect("just pushed graph overrides")
}

fn ensure_graph_node_intrinsic<'a>(
    graph: &'a mut crate::graph::ProjectGraphOverrides,
    group: &str,
    instance: usize,
) -> &'a mut crate::graph::ProjectGraphNodeIntrinsicOverride {
    if let Some(idx) = graph
        .node_intrinsics
        .iter()
        .position(|node| node.group == group && node.instance == instance)
    {
        return &mut graph.node_intrinsics[idx];
    }
    graph
        .node_intrinsics
        .push(crate::graph::ProjectGraphNodeIntrinsicOverride {
            group: group.to_string(),
            instance,
            resolution: None,
            delay_steps: None,
            quantize: None,
            route: None,
            seed_from: None,
            seed_on_reset: None,
            duration: None,
            swing: None,
            neural_group: None,
        });
    graph
        .node_intrinsics
        .last_mut()
        .expect("just pushed graph node override")
}

fn parse_graph_route_override(
    value: &EValue,
) -> Result<crate::graph::ProjectGraphRouteOverride, String> {
    match graph_keyword(value).as_deref() {
        Some("none") | Some("nil") | Some("off") => {
            Ok(crate::graph::ProjectGraphRouteOverride::None)
        }
        _ => parse_nonnegative_usize(value, "route")
            .map(crate::graph::ProjectGraphRouteOverride::Track),
    }
}

fn parse_graph_seed_from(value: &EValue) -> Result<crate::graph::ProjectGraphSeedFrom, String> {
    match graph_keyword(value).as_deref() {
        Some("route") => return Ok(crate::graph::ProjectGraphSeedFrom::Route),
        Some("off") | Some("none") | Some("nil") | Some("false") => {
            return Ok(crate::graph::ProjectGraphSeedFrom::Tracks(Vec::new()));
        }
        _ => {}
    }
    match value {
        EValue::Nil => Ok(crate::graph::ProjectGraphSeedFrom::Tracks(Vec::new())),
        EValue::Number(_) => Ok(crate::graph::ProjectGraphSeedFrom::Tracks(vec![
            parse_nonnegative_usize(value, "seed-from")?,
        ])),
        EValue::List(_) => Ok(crate::graph::ProjectGraphSeedFrom::Tracks(
            graph_list_items(value)
                .unwrap_or_default()
                .iter()
                .map(|value| parse_nonnegative_usize(value, "seed-from track"))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        _ => Err("seed-from expects :route, :off, track, or track list".to_string()),
    }
}

fn parse_graph_seed_on_reset(value: &EValue) -> Result<f64, String> {
    let value = graph_number(value).ok_or("seed-on-reset expects a number")?;
    if !value.is_finite() {
        return Err("seed-on-reset expects a finite number".to_string());
    }
    Ok(value.max(0.0))
}

/// Is this token an "off"/"none" marker rather than a timebase?
fn graph_timebase_token_is_off(value: &EValue) -> bool {
    matches!(
        graph_keyword(value).as_deref(),
        Some("off") | Some("none") | Some("nil") | Some("false")
    )
}

/// Split a cycle field into its individual timebase tokens. Accepts a space-separated
/// string ("16 16 16 16 16 4" — the text-input mini-notation), a list of tokens, or a
/// single value (a length-1 cycle). This is the one place the cycle grammar lives, so
/// the text-input widget and any future text-buffer surface share it.
fn graph_cycle_tokens(value: &EValue) -> Vec<EValue> {
    match value {
        EValue::String(text) => text
            .split_whitespace()
            .map(|token| EValue::String(token.to_string()))
            .collect(),
        EValue::List(_) => graph_list_items(value).unwrap_or_default(),
        other => vec![other.clone()],
    }
}

/// Parse a resolution cycle into timebase indices. Unparseable tokens are dropped (so a
/// half-typed text field doesn't nuke the override); an empty result yields `None` so
/// the caller can leave the field inheriting the prototype.
fn parse_graph_resolution_cycle(value: &EValue) -> Option<Vec<u8>> {
    let indices: Vec<u8> = graph_cycle_tokens(value)
        .iter()
        .filter_map(|token| graph_timebase(token).ok().map(|tb| tb as u8))
        .collect();
    if indices.is_empty() {
        None
    } else {
        Some(indices)
    }
}

fn parse_graph_quantize_override(
    value: &EValue,
) -> Result<crate::graph::ProjectGraphQuantizeOverride, String> {
    // Per-slot off within a cycle isn't representable in v1; "off" tokens are dropped and
    // a field that is entirely off (or empty) collapses to `Off`.
    let indices: Vec<u8> = graph_cycle_tokens(value)
        .iter()
        .filter(|token| !graph_timebase_token_is_off(token))
        .filter_map(|token| graph_timebase(token).ok().map(|tb| tb as u8))
        .collect();
    if indices.is_empty() {
        Ok(crate::graph::ProjectGraphQuantizeOverride::Off)
    } else {
        Ok(crate::graph::ProjectGraphQuantizeOverride::Timebase(
            indices,
        ))
    }
}

fn parse_graph_node_edit(args: &[EValue]) -> Result<GraphNodeEdit, String> {
    let mut edit = GraphNodeEdit::default();
    let mut idx = 0;
    while idx < args.len() {
        let key = graph_keyword(&args[idx])
            .ok_or_else(|| "graph-node expects keyword/value pairs".to_string())?;
        idx += 1;
        let value = args
            .get(idx)
            .ok_or_else(|| format!("graph-node :{key} expects a value"))?;
        match key.as_str() {
            "resolution" | "res" => edit.resolution = parse_graph_resolution_cycle(value),
            "delay" | "delay-steps" => edit.delay_steps = Some(parse_u32_value(value, "delay")?),
            "quantize" | "q" => edit.quantize = Some(parse_graph_quantize_override(value)?),
            "route" => edit.route = Some(parse_graph_route_override(value)?),
            "seed-from" => edit.seed_from = Some(parse_graph_seed_from(value)?),
            "seed-on-reset" | "reset-seed" => {
                edit.seed_on_reset = Some(parse_graph_seed_on_reset(value)?)
            }
            "duration" | "dur" => edit.duration = Some(graph_parse_duration_spec(value)?),
            "swing" => edit.swing = Some(graph_parse_swing_spec(value)?),
            "group" | "grp" => {
                let group = parse_u32_value(value, "group")?;
                edit.neural_group =
                    Some(group.min(crate::graph::NEURAL_GROUP_MAX as u32 - 1) as u8);
            }
            other => return Err(format!("graph-node unknown argument :{other}")),
        }
        idx += 1;
    }
    Ok(edit)
}

fn apply_graph_node_edit(
    node: &mut crate::graph::ProjectGraphNodeIntrinsicOverride,
    edit: GraphNodeEdit,
) {
    if edit.resolution.is_some() {
        node.resolution = edit.resolution;
    }
    if edit.delay_steps.is_some() {
        node.delay_steps = edit.delay_steps;
    }
    if edit.quantize.is_some() {
        node.quantize = edit.quantize;
    }
    if edit.route.is_some() {
        node.route = edit.route;
    }
    if edit.seed_from.is_some() {
        node.seed_from = edit.seed_from;
    }
    if edit.seed_on_reset.is_some() {
        node.seed_on_reset = edit.seed_on_reset;
    }
    if edit.duration.is_some() {
        node.duration = edit.duration;
    }
    if edit.swing.is_some() {
        node.swing = edit.swing;
    }
    if edit.neural_group.is_some() {
        node.neural_group = edit.neural_group;
    }
}

fn upsert_graph_node_param(
    graph: &mut crate::graph::ProjectGraphOverrides,
    group: &str,
    instance: usize,
    param: &str,
    value: f64,
) {
    if let Some(existing) = graph
        .node_params
        .iter_mut()
        .find(|entry| entry.group == group && entry.instance == instance && entry.param == param)
    {
        existing.value = value;
        return;
    }
    graph
        .node_params
        .push(crate::graph::ProjectGraphNodeParamOverride {
            group: group.to_string(),
            instance,
            param: param.to_string(),
            value,
        });
}

fn parse_graph_edge_edit(
    manifest: &crate::graph::GraphManifest,
    args: &[EValue],
) -> Result<GraphEdgeEdit, String> {
    let edge_set = manifest
        .edge_sets
        .first()
        .ok_or_else(|| "graph-edge requires an edge set".to_string())?;
    let mut group = crate::graph::edge_set_group_id(edge_set);
    let mut from = None;
    let mut to = None;
    let mut param = None;
    let mut value = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = graph_keyword(&args[idx])
            .ok_or_else(|| "graph-edge expects keyword/value pairs".to_string())?;
        idx += 1;
        let arg = args
            .get(idx)
            .ok_or_else(|| format!("graph-edge :{key} expects a value"))?;
        match key.as_str() {
            "from" => from = Some(parse_nonnegative_usize(arg, "from")?),
            "to" => to = Some(parse_nonnegative_usize(arg, "to")?),
            "group" => {
                group = graph_key_string(arg)
                    .ok_or_else(|| "graph-edge :group expects a symbol/string".to_string())?
            }
            other => {
                param = Some(other.to_string());
                value = Some(graph_number(arg).ok_or("graph-edge param value must be numeric")?);
            }
        }
        idx += 1;
    }
    let from = from.ok_or_else(|| "graph-edge requires :from".to_string())?;
    let to = to.ok_or_else(|| "graph-edge requires :to".to_string())?;
    if from >= graph_capacity_node_count(manifest) || to >= graph_capacity_node_count(manifest) {
        return Err("graph-edge from/to index out of range".to_string());
    }
    Ok(GraphEdgeEdit {
        group,
        from,
        to,
        param: param.ok_or_else(|| "graph-edge requires an edge param".to_string())?,
        value: value.ok_or_else(|| "graph-edge requires an edge param value".to_string())?,
    })
}

fn upsert_graph_edge_param(graph: &mut crate::graph::ProjectGraphOverrides, edit: GraphEdgeEdit) {
    if let Some(existing) = graph.edge_params.iter_mut().find(|entry| {
        entry.group == edit.group
            && entry.from == edit.from
            && entry.to == edit.to
            && entry.param == edit.param
    }) {
        existing.value = edit.value;
        return;
    }
    graph
        .edge_params
        .push(crate::graph::ProjectGraphEdgeParamOverride {
            group: edit.group,
            from: edit.from,
            to: edit.to,
            param: edit.param,
            value: edit.value,
        });
}

fn graph_manifest_to_value(
    manifest: &crate::graph::GraphManifest,
    overrides: Option<&crate::graph::ProjectGraphOverrides>,
) -> EValue {
    let mut map: HashMap<String, Rc<RefCell<EValue>>> = HashMap::new();
    map.insert("id".to_string(), lisp_number(manifest.id as f64));
    map.insert("name".to_string(), lisp_string(manifest.name.clone()));
    map.insert(
        "nodes".to_string(),
        lisp_number(manifest.shape.resolved_node_count(overrides) as f64),
    );
    map.insert(
        "capacity".to_string(),
        lisp_number(manifest.shape.capacity_num_nodes() as f64),
    );
    let max_poly = overrides
        .and_then(|o| o.max_poly)
        .unwrap_or(manifest.max_poly);
    let max_poly_selection = overrides
        .and_then(|o| o.max_poly_selection)
        .unwrap_or(manifest.max_poly_selection);
    map.insert("max-poly".to_string(), lisp_number(max_poly as f64));
    map.insert(
        "max-poly-selection".to_string(),
        lisp_string(max_poly_selection.as_str().to_string()),
    );
    map.insert(
        "node-group".to_string(),
        lisp_string(manifest.node.name.clone()),
    );
    map.insert(
        "overrides".to_string(),
        lisp_number(
            overrides
                .map(|o| o.node_intrinsics.len() + o.node_params.len() + o.edge_params.len())
                .unwrap_or(0) as f64,
        ),
    );
    EValue::Map(map)
}
