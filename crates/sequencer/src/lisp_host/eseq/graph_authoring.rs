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
    register_graph_node_process_natives(runtime, Arc::clone(&state));

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

    let state_for_graph_owner = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-owner",
        "(graph-owner id-or-name)",
        "The drum rack group id that owns this graph sequencer, or nil when the \
         project owns it (docs/rack-clips-and-break-kits-spec.md §5).",
        move |args, _ctx| {
            let reference = args
                .first()
                .ok_or_else(|| "graph-owner expects graph id or name".to_string())?;
            let manifest = resolve_graph_manifest(&state_for_graph_owner, reference)?;
            Ok(manifest
                .owner_rack
                .map(|group_id| EValue::Number(group_id as f64))
                .unwrap_or(EValue::Nil))
        },
    );

    let state_for_graph_route_tracks = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-route-tracks",
        "(graph-route-tracks id-or-name)",
        "For a rack-owned graph sequencer, the track index behind each route \
         option (member order, so route n is member n); nil when the project \
         owns it and routes are plain track indices.",
        move |args, _ctx| {
            let reference = args
                .first()
                .ok_or_else(|| "graph-route-tracks expects graph id or name".to_string())?;
            let manifest = resolve_graph_manifest(&state_for_graph_route_tracks, reference)?;
            let Some(group_id) = manifest.owner_rack else {
                return Ok(EValue::Nil);
            };
            let memberships = state_for_graph_route_tracks.rack_memberships();
            let members = crate::graph::rack_members(&memberships, group_id).unwrap_or(&[]);
            Ok(lisp_list(
                members.iter().map(|track| EValue::Number(*track as f64)).collect(),
            ))
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
                // A route is already an index into the route option list
                // (track n / rack member n at position n), with "Off" as the
                // list's last entry; label matching would tie the binding to
                // the "Track n" spelling, which rack-owned instances do not use.
                Some(options) if field == "route" => {
                    let option_count = match options {
                        EValue::List(items) => items.len(),
                        _ => 0,
                    };
                    match resolved_graph_node_value(&state_for_bind_graph, &manifest, instance, &field)? {
                        EValue::Number(route) if (route as usize) < option_count => route,
                        _ => option_count.saturating_sub(1) as f64,
                    }
                }
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

    let state_for_node_notes = Arc::clone(&state);
    let bindings_for_node_notes = runtime.reactive_binding_store();
    runtime.register_native_with_docs(
        "bind-graph-node-notes",
        "(bind-graph-node-notes sequencer node-index)",
        "Bindings to the notes a graph node is sounding right now: a map with \
         :count, :values and :levels (velocities; NODE_SOUNDING_DISPLAY element \
         bindings each), shaped for \
         `number-list`. The host republishes them from the audio clock, so a bound \
         widget repaints without re-running Lisp.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("bind-graph-node-notes expects graph and node index".to_string());
            }
            let manifest = resolve_graph_manifest(&state_for_node_notes, &args[0])?;
            let node = parse_nonnegative_usize(&args[1], "node index")?;
            let field = crate::graph::node_sounding_field(manifest.id);
            let base = node * crate::graph::NODE_SOUNDING_STRIDE;
            let display = crate::graph::NODE_SOUNDING_DISPLAY;
            let refs = |first: usize| -> EValue {
                EValue::List(
                    (first..first + display)
                        .map(|index| {
                            lisp_value(bindings_for_node_notes.indexed_float_ref("SEQ", field.clone(), index))
                        })
                        .collect(),
                )
            };
            let values = refs(base + 1);
            let levels = refs(base + 1 + display);
            let mut map = HashMap::new();
            map.insert(
                "count".to_string(),
                lisp_value(bindings_for_node_notes.indexed_float_ref("SEQ", field.clone(), base)),
            );
            map.insert("values".to_string(), lisp_value(values));
            map.insert("levels".to_string(), lisp_value(levels));
            Ok(EValue::Map(map))
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
        "Resolved sequencer-level config, override-or-manifest. Fields: :reset-bars, :max-poly, :max-poly-selection, :node-count, :group-trace-decay, :group-coupling-scale, :group-excite-floor, and the neural-group matrix cells :group-gain-<row>-<col> (default 1) / :group-coupling-<row>-<col> (default 0), row = source group, col = target group.",
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
        "Set a sequencer-level config override: :reset-bars (in bars), :max-poly, :max-poly-selection, :node-count, :group-trace-decay (0-1 per-beat activity-trace decay), :group-coupling-scale (0-2 global multiplier on the whole H matrix, default 1), :group-excite-floor (0-1 fraction of the authored threshold excitation cannot push below, default 0.25; 0 = excited groups may fire on zero energy), :group-gain-<row>-<col> (0-2, propagation gain from group row to group col), :group-coupling-<row>-<col> (-2 to 2, activity in group row offsets group col's threshold; positive suppresses, negative excites). Values clamp to their range; one call writes one matrix cell.",
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
        "Reactive handle to a sequencer-level config field (see graph-config for the field list, including the :group-gain-<row>-<col> / :group-coupling-<row>-<col> matrix cells), seeded with the resolved value. Pass an options list to bind enum fields as dropdown indices.",
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
        .find(|overrides| manifest.matches_overrides(overrides))
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
        if std::env::var_os("ESEQ_DEBUG_GRAPH_MEMO").is_some() {
            let overrides = resolved_graph_overrides_for_manifest(state, manifest);
            eprintln!(
                "[graph-memo] miss manifest={} ({}, owner {:?}) pattern={pattern} snapshot_v={snapshot_version} published_v={published_version} nodes={} overrides={} composed_total={}",
                manifest.id,
                manifest.name,
                manifest.owner_rack,
                config.nodes.len(),
                overrides
                    .as_ref()
                    .map(|o| format!(
                        "node_count={:?} intrinsics={} edges={} id={} owner={:?}",
                        o.node_count,
                        o.node_intrinsics.len(),
                        o.edge_params.len(),
                        o.sequencer_id,
                        o.owner_rack
                    ))
                    .unwrap_or_else(|| "NONE".to_string()),
                state.current_graph_overrides().len(),
            );
        }
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

/// Parse a `<prefix>-<row>-<col>` group-matrix cell field (e.g. `group-gain-1-2`)
/// into a flat row-major cell index. One cell per field keeps the config lockstep
/// scalar-shaped: a matrix drag writes exactly one override cell.
fn parse_group_cell_index(field: &str, prefix: &str) -> Option<usize> {
    let suffix = field.strip_prefix(prefix)?;
    let (row, col) = suffix.split_once('-')?;
    let row: usize = row.parse().ok()?;
    let col: usize = col.parse().ok()?;
    let k = crate::graph::NEURAL_GROUP_MAX as usize;
    if row >= k || col >= k {
        return None;
    }
    Some(row * k + col)
}

fn group_matrix_cell(cells: Option<&Vec<f64>>, index: usize, default: f64) -> f64 {
    cells
        .and_then(|cells| cells.get(index))
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(default)
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
            // Read the count off the same memoized runtime config that
            // `bind-graph` range-checks node indices against, so one render
            // can never loop over more rows than it can bind: a panel that
            // sized itself from a fresh override read while the memo lagged
            // behind an unpublished edit failed every row past the memo's
            // node list, and the error flood stalled the tab.
            Ok(EValue::Number(graph_active_node_count(state, manifest) as f64))
        }
        "group-trace-decay" => {
            let value = overrides
                .as_ref()
                .and_then(|o| o.group_trace_decay)
                .unwrap_or(crate::graph::GROUP_TRACE_DECAY_DEFAULT);
            Ok(EValue::Number(value))
        }
        "group-coupling-scale" => {
            let value = overrides
                .as_ref()
                .and_then(|o| o.group_coupling_scale)
                .unwrap_or(crate::graph::GROUP_COUPLING_SCALE_DEFAULT);
            Ok(EValue::Number(value))
        }
        "group-excite-floor" => {
            let value = overrides
                .as_ref()
                .and_then(|o| o.group_excite_floor)
                .unwrap_or(crate::graph::GROUP_EXCITE_FLOOR_DEFAULT);
            Ok(EValue::Number(value))
        }
        other => {
            if let Some(index) = parse_group_cell_index(other, "group-gain-") {
                return Ok(EValue::Number(group_matrix_cell(
                    overrides.as_ref().and_then(|o| o.group_gain.as_ref()),
                    index,
                    crate::graph::GROUP_GAIN_DEFAULT,
                )));
            }
            if let Some(index) = parse_group_cell_index(other, "group-coupling-") {
                return Ok(EValue::Number(group_matrix_cell(
                    overrides.as_ref().and_then(|o| o.group_coupling.as_ref()),
                    index,
                    crate::graph::GROUP_COUPLING_DEFAULT,
                )));
            }
            Err(format!("graph config unknown field :{other}"))
        }
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
        GroupGainCell(usize, f64),
        GroupCouplingCell(usize, f64),
        GroupTraceDecay(f64),
        GroupCouplingScale(f64),
        GroupExciteFloor(f64),
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
        "group-trace-decay" => {
            let value = graph_number(value).ok_or_else(|| {
                "graph config :group-trace-decay expects a numeric value".to_string()
            })?;
            if !value.is_finite() {
                return Err("graph config :group-trace-decay expects a finite value".to_string());
            }
            ConfigEdit::GroupTraceDecay(value.clamp(0.0, 1.0))
        }
        "group-coupling-scale" => {
            let value = graph_number(value).ok_or_else(|| {
                "graph config :group-coupling-scale expects a numeric value".to_string()
            })?;
            if !value.is_finite() {
                return Err("graph config :group-coupling-scale expects a finite value".to_string());
            }
            ConfigEdit::GroupCouplingScale(
                value.clamp(0.0, crate::graph::GROUP_COUPLING_SCALE_MAX),
            )
        }
        "group-excite-floor" => {
            let value = graph_number(value).ok_or_else(|| {
                "graph config :group-excite-floor expects a numeric value".to_string()
            })?;
            if !value.is_finite() {
                return Err("graph config :group-excite-floor expects a finite value".to_string());
            }
            ConfigEdit::GroupExciteFloor(value.clamp(0.0, 1.0))
        }
        other => {
            if let Some(index) = parse_group_cell_index(other, "group-gain-") {
                let value = graph_number(value)
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| "graph config group-gain cell expects a number".to_string())?;
                ConfigEdit::GroupGainCell(
                    index,
                    value.clamp(crate::graph::GROUP_GAIN_MIN, crate::graph::GROUP_GAIN_MAX),
                )
            } else if let Some(index) = parse_group_cell_index(other, "group-coupling-") {
                let value = graph_number(value)
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| {
                        "graph config group-coupling cell expects a number".to_string()
                    })?;
                ConfigEdit::GroupCouplingCell(
                    index,
                    value.clamp(
                        crate::graph::GROUP_COUPLING_MIN,
                        crate::graph::GROUP_COUPLING_MAX,
                    ),
                )
            } else {
                return Err(format!("graph config unknown field :{other}"));
            }
        }
    };

    state.edit_current_graph_overrides(|graphs| {
        let graph = ensure_graph_overrides(graphs, manifest);
        match edit {
            ConfigEdit::ResetEveryBeats(value) => graph.reset_every_beats = Some(value),
            ConfigEdit::MaxPoly(value) => graph.max_poly = Some(value),
            ConfigEdit::MaxPolySelection(value) => graph.max_poly_selection = Some(value),
            ConfigEdit::NodeCount(value) => graph.node_count = Some(value),
            ConfigEdit::GroupGainCell(index, value) => {
                let cells = graph
                    .group_gain
                    .get_or_insert_with(|| {
                        vec![crate::graph::GROUP_GAIN_DEFAULT; crate::graph::NEURAL_GROUP_CELLS]
                    });
                cells.resize(crate::graph::NEURAL_GROUP_CELLS, crate::graph::GROUP_GAIN_DEFAULT);
                cells[index] = value;
            }
            ConfigEdit::GroupCouplingCell(index, value) => {
                let cells = graph.group_coupling.get_or_insert_with(|| {
                    vec![crate::graph::GROUP_COUPLING_DEFAULT; crate::graph::NEURAL_GROUP_CELLS]
                });
                cells.resize(
                    crate::graph::NEURAL_GROUP_CELLS,
                    crate::graph::GROUP_COUPLING_DEFAULT,
                );
                cells[index] = value;
            }
            ConfigEdit::GroupTraceDecay(value) => graph.group_trace_decay = Some(value),
            ConfigEdit::GroupCouplingScale(value) => graph.group_coupling_scale = Some(value),
            ConfigEdit::GroupExciteFloor(value) => graph.group_excite_floor = Some(value),
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
            let candidates: Vec<crate::graph::GraphManifest> = published
                .into_iter()
                .filter_map(|published| published.graph)
                .filter(|manifest| manifest.name == name)
                .collect();
            // A bare name is unambiguous when one instance carries it, or when
            // the host is currently evaluating a rack-attached script and one
            // of them belongs to that rack. Otherwise the caller must use the
            // numeric handle `def-sequencer` returned.
            let owner = crate::lisp_host::eseq::graph_manifest::current_graph_owner_rack();
            if let Some(manifest) = candidates
                .iter()
                .find(|manifest| owner.is_some() && manifest.owner_rack == owner)
            {
                return Ok(manifest.clone());
            }
            match candidates.len() {
                0 => Err("graph sequencer name not found".to_string()),
                1 => Ok(candidates.into_iter().next().expect("one candidate")),
                n => Err(format!(
                    "graph sequencer name '{name}' is ambiguous: {n} instances (ids {}); \
                     use the id returned by def-sequencer",
                    candidates
                        .iter()
                        .map(|manifest| manifest.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        _ => Err("graph reference must be id or name".to_string()),
    }
}

fn graph_overrides_for_manifest<'a>(
    overrides: &'a [crate::graph::ProjectGraphOverrides],
    manifest: &crate::graph::GraphManifest,
) -> Option<&'a crate::graph::ProjectGraphOverrides> {
    overrides.iter().find(|overrides| manifest.matches_overrides(overrides))
}

fn ensure_graph_overrides<'a>(
    graphs: &'a mut Vec<crate::graph::ProjectGraphOverrides>,
    manifest: &crate::graph::GraphManifest,
) -> &'a mut crate::graph::ProjectGraphOverrides {
    if let Some(idx) = graphs.iter().position(|graph| manifest.matches_overrides(graph)) {
        return &mut graphs[idx];
    }
    graphs.push(crate::graph::ProjectGraphOverrides {
        sequencer_id: manifest.id,
        sequencer_name: manifest.name.clone(),
        owner_rack: manifest.owner_rack,
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
            process_chain: None,
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
        "owner-rack".to_string(),
        match manifest.owner_rack {
            Some(group_id) => lisp_number(group_id as f64),
            None => Rc::new(RefCell::new(EValue::Nil)),
        },
    );
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


// ── Node process patches (docs/graph-node-processes-spec.md §5) ──────────────
//
// A node's process patch lives on its intrinsic override as a
// `TrackProcessChain`. These natives are the node-side twins of the track
// natives (`seq-add-track-process-slot`, `seq-set-process-inlet`,
// `seq-bind-process-port`, ...) with `(graph node)` in place of `track`, and one
// read that hands the UI everything it needs to draw the patch.

/// Node process slot ids live in their own band, below the track roster band
/// (`TRACK_ROSTER_INSTANCE_ID_BASE`), so they can never collide with roster
/// slots minted later on a track.
const GRAPH_NODE_PROCESS_ID_BASE: u64 = 1 << 45;

/// Session high-water mark of minted node slot ids. A removed slot's id is
/// never handed out again in this session, so a re-added slot can neither
/// resume the removed slot's runtime state (keyed by id) nor pick up a stale
/// cable, and a slot minted in another scene never shares an id with one
/// minted here.
static GRAPH_NODE_PROCESS_ID_HIGH_WATER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(GRAPH_NODE_PROCESS_ID_BASE);

fn next_graph_node_process_id(graphs: &[crate::graph::ProjectGraphOverrides]) -> u64 {
    let highest = graphs
        .iter()
        .flat_map(|graph| graph.node_intrinsics.iter())
        .filter_map(|node| node.process_chain.as_ref())
        .flat_map(|chain| chain.slots.iter())
        .map(|slot| slot.instance_id.0)
        .filter(|id| (GRAPH_NODE_PROCESS_ID_BASE..crate::process::TRACK_ROSTER_INSTANCE_ID_BASE).contains(id))
        .max();
    let next = highest.map(|id| id + 1).unwrap_or(GRAPH_NODE_PROCESS_ID_BASE);
    next.max(GRAPH_NODE_PROCESS_ID_HIGH_WATER.load(std::sync::atomic::Ordering::Relaxed))
}

/// Record that `id` was minted, so it is never minted again this session.
fn claim_graph_node_process_id(id: u64) {
    GRAPH_NODE_PROCESS_ID_HIGH_WATER.fetch_max(id + 1, std::sync::atomic::Ordering::Relaxed);
}

fn graph_node_process_def(
    state: &crate::sequencer::SequencerState,
    class_name: &str,
) -> Option<crate::process::PublishedProcessDef> {
    state
        .published_process_authoring()
        .defs
        .into_iter()
        .find(|def| def.name == class_name)
}

/// Node-flavoured display name for a process class: the track lane classes
/// drop their `lane-` prefix (a lane inlet is just a knob on a node), the
/// Cirklon pair spell out what they read. Data keeps the class name.
pub fn graph_node_process_label(class_name: &str) -> String {
    match class_name {
        "xpose-by-track" => "xpose by track".to_string(),
        "xpose-by-track-b" => "xpose by track +b".to_string(),
        "neural-transpose" => "transpose".to_string(),
        "neural-scale" => "scale".to_string(),
        "neural-delay" => "delay".to_string(),
        "neural-reset" => "reset".to_string(),
        other => other.strip_prefix("lane-").unwrap_or(other).to_string(),
    }
}

/// Classes whose only effect is a command the node runner ignores
/// (ratchet / roll / pattern length), or that read the track's painted steps: hidden from the
/// node picker so the list is what actually does something on a fire.
/// `lane-reset` only sends when its own gate lane is high, and nodes have no
/// lanes; it also shows as "reset", the same as `neural-reset`, which is the
/// node's real reset (its `fired` port rises after a bar/graph reset).
pub const GRAPH_NODE_HIDDEN_PROCESS_CLASSES: [&str; 5] =
    ["lane-roll", "repeater", "lane-grab", "lane-reset", "lane-length"];

fn graph_node_process_inlet_kind_value(kind: &crate::process::ProcessInletKind) -> EValue {
    use crate::process::ProcessInletKind::*;
    match kind {
        Float => EValue::String("float".into()),
        Int => EValue::String("int".into()),
        Gate => EValue::String("gate".into()),
        Track => EValue::String("track".into()),
        Field => EValue::String("field".into()),
        Any => EValue::String("any".into()),
        Enum(_) => EValue::String("enum".into()),
    }
}

/// One slot of a node patch as the UI sees it: identity, class metadata,
/// scalar inlet values and, per connectable port, what it is wired to.
fn graph_node_process_slot_value(
    state: &crate::sequencer::SequencerState,
    slot: &crate::process::TrackProcessSlot,
) -> EValue {
    let def = graph_node_process_def(state, &slot.class_name);
    let mut map = HashMap::new();
    map.insert("instance-id".to_string(), lisp_number(slot.instance_id.0 as f64));
    map.insert("class".to_string(), lisp_string(slot.class_name.clone()));
    map.insert("label".to_string(), lisp_string(graph_node_process_label(&slot.class_name)));
    map.insert("enabled".to_string(), lisp_bool(slot.enabled));
    map.insert(
        "doc".to_string(),
        lisp_string(def.as_ref().and_then(|d| d.doc.clone()).unwrap_or_default()),
    );
    map.insert("known".to_string(), lisp_bool(def.is_some()));
    let mut inlets = HashMap::new();
    for (name, value) in &slot.inlets {
        inlets.insert(name.clone(), lisp_value(value.to_value()));
    }
    map.insert("inlets".to_string(), lisp_value(EValue::Map(inlets)));
    let inlet_defs = def
        .as_ref()
        .map(|def| {
            def.inlets
                .iter()
                .map(|inlet| {
                    let mut m = HashMap::new();
                    m.insert("name".to_string(), lisp_string(inlet.name.clone()));
                    m.insert("kind".to_string(), lisp_value(graph_node_process_inlet_kind_value(&inlet.kind)));
                    m.insert(
                        "min".to_string(),
                        lisp_value(inlet.min.map(|v| EValue::Number(v as f64)).unwrap_or(EValue::Nil)),
                    );
                    m.insert(
                        "max".to_string(),
                        lisp_value(inlet.max.map(|v| EValue::Number(v as f64)).unwrap_or(EValue::Nil)),
                    );
                    m.insert("default".to_string(), lisp_value(inlet.default.to_value()));
                    // The slot's current scalar for this inlet (or the default), so
                    // the UI never has to index the inlets map by a string key.
                    m.insert(
                        "value".to_string(),
                        lisp_value(
                            slot.inlets
                                .get(&inlet.name)
                                .map(|v| v.to_value())
                                .unwrap_or_else(|| inlet.default.to_value()),
                        ),
                    );
                    m.insert("lane".to_string(), lisp_bool(inlet.lane));
                    m.insert("doc".to_string(), lisp_string(inlet.doc.clone().unwrap_or_default()));
                    let options = match &inlet.kind {
                        crate::process::ProcessInletKind::Enum(options) => {
                            lisp_list(options.iter().map(|o| EValue::String(o.clone())).collect())
                        }
                        _ => EValue::Nil,
                    };
                    m.insert("options".to_string(), lisp_value(options));
                    EValue::Map(m)
                })
                .collect()
        })
        .unwrap_or_default();
    map.insert("inlet-defs".to_string(), lisp_value(lisp_list(inlet_defs)));
    let ports = def
        .as_ref()
        .map(|def| {
            def.ports
                .iter()
                .map(|port| {
                    let mut m = HashMap::new();
                    m.insert("name".to_string(), lisp_string(port.name.clone()));
                    m.insert("connectable".to_string(), lisp_bool(port.is_connectable()));
                    m.insert("mappable".to_string(), lisp_bool(port.is_mappable()));
                    let hint = match &port.target {
                        Some(crate::process::ProcessTargetHint::StepParam { param }) => {
                            format!("step:{param}")
                        }
                        Some(other) => format!("{other:?}"),
                        None => String::new(),
                    };
                    m.insert("hint".to_string(), lisp_string(hint));
                    m.insert("unbound".to_string(), lisp_bool(slot.unbound_ports.contains(&port.name)));
                    let wired = match slot.bindings.get(&port.name) {
                        Some(Some(crate::process::ParamTarget::ProcessInlet { inlet, instance_id, .. })) => {
                            let mut w = HashMap::new();
                            w.insert(
                                "instance-id".to_string(),
                                lisp_value(
                                    instance_id
                                        .map(|id| EValue::Number(id.0 as f64))
                                        .unwrap_or(EValue::Nil),
                                ),
                            );
                            w.insert("inlet".to_string(), lisp_string(inlet.clone()));
                            EValue::Map(w)
                        }
                        _ => EValue::Nil,
                    };
                    m.insert("wired-to".to_string(), lisp_value(wired));
                    let mapped = match slot.bindings.get(&port.name) {
                        Some(Some(crate::process::ParamTarget::StepParam { param })) => {
                            EValue::String(param.clone())
                        }
                        _ => EValue::Nil,
                    };
                    m.insert("mapped-to".to_string(), lisp_value(mapped));
                    EValue::Map(m)
                })
                .collect()
        })
        .unwrap_or_default();
    map.insert("ports".to_string(), lisp_value(lisp_list(ports)));
    EValue::Map(map)
}

fn graph_node_process_args(
    state: &crate::sequencer::SequencerState,
    native: &str,
    args: &[EValue],
) -> Result<(crate::graph::GraphManifest, usize), String> {
    if args.len() < 2 {
        return Err(format!("{native} expects graph id/name and node index"));
    }
    let manifest = resolve_graph_manifest(state, &args[0])?;
    let instance = parse_nonnegative_usize(&args[1], "node index")?;
    if instance >= graph_capacity_node_count(&manifest) {
        return Err(format!("{native} node index out of range"));
    }
    Ok((manifest, instance))
}

fn graph_node_process_id_arg(value: Option<&EValue>, native: &str) -> Result<crate::process::ProcessInstanceId, String> {
    let id = value
        .and_then(graph_number)
        .ok_or_else(|| format!("{native} expects a slot instance id"))?;
    Ok(crate::process::ProcessInstanceId(id as u64))
}

/// Edit one node's chain in place; `edit` returns `Err` to reject.
fn edit_graph_node_process_chain<R>(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    instance: usize,
    edit: impl FnOnce(&mut crate::process::TrackProcessChain, u64) -> Result<R, String>,
) -> Result<R, String> {
    state.edit_current_graph_overrides(|graphs| {
        let next_id = next_graph_node_process_id(graphs);
        let graph = ensure_graph_overrides(graphs, manifest);
        let node = ensure_graph_node_intrinsic(graph, &manifest.node.name, instance);
        let mut chain = node.process_chain.take().unwrap_or_default();
        let result = edit(&mut chain, next_id)?;
        node.process_chain = if chain.slots.is_empty() { None } else { Some(chain) };
        Ok(result)
    })
}

/// Cable ids for node patches live above every track: the lane patchbay
/// folds a namespace into each port id (`(ns * 4096 + slot) * 16 + ordinal`,
/// mirroring `lane_patch_port_id` on the UI side) and tracks use their index.
pub const GRAPH_NODE_LANE_PATCH_NAMESPACE_BASE: usize = 1024;
const LANE_PATCH_PORT_STRIDE: usize = 16;
const LANE_PATCH_TRACK_STRIDE: usize = 4096;

fn lane_patch_port_id(namespace: usize, slot_index: usize, ordinal: usize) -> usize {
    (namespace * LANE_PATCH_TRACK_STRIDE + slot_index) * LANE_PATCH_PORT_STRIDE + ordinal
}

/// The node patch as the lane patchbay draws it: the same entry shape the
/// UI builds for `SEQ.track-lane-patch` (cable-level out/in ports), minus the
/// track-only `param-ports`. Keep the two in step.
fn graph_node_lane_patch_value(
    state: &crate::sequencer::SequencerState,
    chain: &crate::process::TrackProcessChain,
    namespace: usize,
) -> EValue {
    use crate::process::{ParamTarget, ProcessInletKind};
    let published = state.published_process_authoring();
    let def_for = |slot: &crate::process::TrackProcessSlot| {
        published.defs.iter().find(|def| def.name == slot.class_name)
    };
    let resolve = |target: &ParamTarget| -> Option<(usize, String)> {
        let ParamTarget::ProcessInlet { process, inlet, instance_id } = target else {
            return None;
        };
        let index = chain.slots.iter().position(|slot| {
            slot.class_name == *process && instance_id.is_none_or(|id| slot.instance_id == id)
        })?;
        Some((index, inlet.clone()))
    };
    struct Reader {
        slot_index: usize,
        inlet: String,
        source: &'static str,
        fanout_index: Option<usize>,
    }
    let mut out_ports: Vec<Vec<(String, usize, bool, Vec<Reader>)>> = Vec::new();
    let mut in_writers: std::collections::BTreeMap<(usize, String), Vec<usize>> = Default::default();
    for (slot_index, slot) in chain.slots.iter().enumerate() {
        let mut entries = Vec::new();
        let connectable = def_for(slot)
            .map(|def| def.ports.iter().filter(|port| port.is_connectable()).collect::<Vec<_>>())
            .unwrap_or_default();
        for (ordinal, port) in connectable.iter().enumerate() {
            let port_id = lane_patch_port_id(namespace, slot_index, ordinal);
            let mut readers = Vec::new();
            let primary_free = slot.unbound_ports.contains(&port.name)
                || !matches!(slot.bindings.get(&port.name), Some(Some(_)));
            if !slot.unbound_ports.contains(&port.name) {
                if let Some(Some(target)) = slot.bindings.get(&port.name) {
                    if let Some((index, inlet)) = resolve(target) {
                        readers.push(Reader { slot_index: index, inlet, source: "primary", fanout_index: None });
                    }
                }
            }
            let fanout = slot.fanout.get(&port.name).map(Vec::as_slice).unwrap_or(&[]);
            for (fanout_index, entry) in fanout.iter().enumerate() {
                if let Some((index, inlet)) = resolve(&entry.target) {
                    readers.push(Reader { slot_index: index, inlet, source: "fanout", fanout_index: Some(fanout_index) });
                }
            }
            for reader in &readers {
                in_writers.entry((reader.slot_index, reader.inlet.clone())).or_default().push(port_id);
            }
            entries.push((port.name.clone(), port_id, primary_free, readers));
        }
        out_ports.push(entries);
    }
    let kind_name = |kind: &ProcessInletKind| -> &'static str {
        match kind {
            ProcessInletKind::Float => "float",
            ProcessInletKind::Int => "int",
            ProcessInletKind::Gate => "gate",
            ProcessInletKind::Track => "track",
            ProcessInletKind::Field => "field",
            ProcessInletKind::Any => "any",
            ProcessInletKind::Enum(_) => "enum",
        }
    };
    let map = |entries: Vec<(&str, EValue)>| -> EValue {
        let mut m = HashMap::new();
        for (key, value) in entries {
            m.insert(key.to_string(), lisp_value(value));
        }
        EValue::Map(m)
    };
    lisp_list(
        chain
            .slots
            .iter()
            .enumerate()
            .map(|(slot_index, slot)| {
                let def = def_for(slot);
                let in_ports = def
                    .map(|def| {
                        def.inlets
                            .iter()
                            .filter(|inlet| {
                                inlet.lane
                                    || matches!(inlet.kind, ProcessInletKind::Gate)
                                    || in_writers.contains_key(&(slot_index, inlet.name.clone()))
                            })
                            .enumerate()
                            .map(|(ordinal, inlet)| {
                                let writers = in_writers
                                    .get(&(slot_index, inlet.name.clone()))
                                    .map(Vec::as_slice)
                                    .unwrap_or(&[]);
                                map(vec![
                                    ("name", EValue::String(inlet.name.clone())),
                                    ("ordinal", EValue::Number(ordinal as f64)),
                                    ("lane", EValue::Bool(inlet.lane)),
                                    ("kind", EValue::String(kind_name(&inlet.kind).to_string())),
                                    ("writers", lisp_list(writers.iter().map(|id| EValue::Number(*id as f64)).collect())),
                                ])
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let outs = out_ports[slot_index]
                    .iter()
                    .enumerate()
                    .map(|(ordinal, (name, port_id, primary_free, readers))| {
                        map(vec![
                            ("name", EValue::String(name.clone())),
                            ("ordinal", EValue::Number(ordinal as f64)),
                            ("port-id", EValue::Number(*port_id as f64)),
                            ("primary-free", EValue::Bool(*primary_free)),
                            (
                                "readers",
                                lisp_list(
                                    readers
                                        .iter()
                                        .map(|reader| {
                                            map(vec![
                                                ("slot-index", EValue::Number(reader.slot_index as f64)),
                                                (
                                                    "instance-id",
                                                    EValue::Number(chain.slots[reader.slot_index].instance_id.0 as f64),
                                                ),
                                                ("inlet", EValue::String(reader.inlet.clone())),
                                                ("source", EValue::String(reader.source.to_string())),
                                                (
                                                    "fanout-index",
                                                    reader.fanout_index.map(|i| EValue::Number(i as f64)).unwrap_or(EValue::Nil),
                                                ),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect::<Vec<_>>();
                let display_name = slot
                    .instance_name
                    .clone()
                    .unwrap_or_else(|| graph_node_process_label(&slot.class_name));
                map(vec![
                    ("slot-index", EValue::Number(slot_index as f64)),
                    ("instance-id", EValue::Number(slot.instance_id.0 as f64)),
                    ("name", EValue::String(display_name)),
                    ("class", EValue::String(slot.class_name.clone())),
                    ("project", EValue::Bool(false)),
                    ("enabled", EValue::Bool(slot.enabled)),
                    ("default-lane", EValue::Bool(false)),
                    ("out-ports", lisp_list(outs)),
                    ("in-ports", lisp_list(in_ports)),
                    ("param-ports", lisp_list(Vec::new())),
                ])
            })
            .collect(),
    )
}

fn register_graph_node_process_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
) {
    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-chain",
        "(graph-node-process-chain sequencer node-index)",
        "The node's process patch as a list of slot maps: :instance-id :class :enabled :doc :inlets (name -> value) :inlet-defs (name kind min max default lane options doc) :ports (name connectable mappable hint unbound wired-to {:instance-id :inlet} mapped-to step-param-name-or-nil).",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-chain", &args)?;
            let overrides = st.current_graph_overrides();
            let chain = overrides
                .iter()
                .find(|o| manifest.matches_overrides(o))
                .and_then(|o| {
                    o.node_intrinsics
                        .iter()
                        .find(|n| n.group == manifest.node.name && n.instance == instance)
                })
                .and_then(|n| n.process_chain.clone())
                .unwrap_or_default();
            Ok(lisp_list(
                chain
                    .slots
                    .iter()
                    .map(|slot| graph_node_process_slot_value(&st, slot))
                    .collect(),
            ))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-add",
        "(graph-node-process-add sequencer node-index class-name)",
        "Append a process slot of the given class to the node's patch; returns the new slot's instance id.",
        move |args, ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-add", &args)?;
            let class_name = args
                .get(2)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-add expects a class name".to_string())?;
            if graph_node_process_def(&st, &class_name).is_none()
                && !crate::process::DEFAULT_LANE_CLASSES.contains(&class_name.as_str())
            {
                return Err(format!("graph-node-process-add: unknown process class {class_name:?}"));
            }
            let id = edit_graph_node_process_chain(&st, &manifest, instance, |chain, next_id| {
                chain.slots.push(crate::process::TrackProcessSlot {
                    instance_id: crate::process::ProcessInstanceId(next_id),
                    instance_name: None,
                    class_name: class_name.clone(),
                    enabled: true,
                    project_layer: false,
                    inlets: Default::default(),
                    lanes: Default::default(),
                    fanout: Default::default(),
                    unbound_ports: Default::default(),
                    bindings: Default::default(),
                });
                Ok(next_id)
            })?;
            claim_graph_node_process_id(id);
            ctx.set_status(format!("node {instance}: added {class_name}"));
            Ok(EValue::Number(id as f64))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-remove",
        "(graph-node-process-remove sequencer node-index instance-id)",
        "Remove one slot from the node's patch and drop every wire and fan-out cable into it.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-remove", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-remove")?;
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let before = chain.slots.len();
                chain.slots.retain(|slot| slot.instance_id != id);
                let targets_removed = |target: &crate::process::ParamTarget| {
                    matches!(target, crate::process::ParamTarget::ProcessInlet { instance_id: Some(i), .. } if *i == id)
                };
                for slot in &mut chain.slots {
                    slot.bindings
                        .retain(|_, target| !target.as_ref().is_some_and(targets_removed));
                    // Fan-out cables into the removed slot go too, as
                    // `graph-node-process-fanout-remove` would drop them.
                    for entries in slot.fanout.values_mut() {
                        entries.retain(|entry| !targets_removed(&entry.target));
                    }
                    slot.fanout.retain(|_, entries| !entries.is_empty());
                }
                if chain.slots.len() == before {
                    return Err(format!("graph-node-process-remove: no slot {}", id.0));
                }
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-enable",
        "(graph-node-process-enable sequencer node-index instance-id enabled)",
        "Enable or bypass one slot in the node's patch.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-enable", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-enable")?;
            let enabled = !matches!(args.get(3), Some(EValue::Bool(false)) | Some(EValue::Nil) | None);
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let slot = chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == id)
                    .ok_or_else(|| format!("graph-node-process-enable: no slot {}", id.0))?;
                slot.enabled = enabled;
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-move",
        "(graph-node-process-move sequencer node-index instance-id delta)",
        "Move one slot earlier (negative) or later (positive) in the node's patch. Order is run order.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-move", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-move")?;
            let delta = args.get(3).and_then(graph_number).unwrap_or(0.0) as i64;
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let from = chain
                    .slots
                    .iter()
                    .position(|slot| slot.instance_id == id)
                    .ok_or_else(|| format!("graph-node-process-move: no slot {}", id.0))?;
                let to = (from as i64 + delta).clamp(0, chain.slots.len() as i64 - 1) as usize;
                let slot = chain.slots.remove(from);
                chain.slots.insert(to, slot);
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-inlet",
        "(graph-node-process-inlet sequencer node-index instance-id inlet value)",
        "Set a scalar inlet on one slot of the node's patch. On a node every inlet is a scalar (no lanes); a wired inlet reads its wire instead.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-inlet", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-inlet")?;
            let inlet = args
                .get(3)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-inlet expects an inlet name".to_string())?;
            let value = args
                .get(4)
                .ok_or_else(|| "graph-node-process-inlet expects a value".to_string())?;
            let literal = crate::process::ProcessLiteral::from_value(value)?;
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let slot = chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == id)
                    .ok_or_else(|| format!("graph-node-process-inlet: no slot {}", id.0))?;
                slot.inlets.insert(inlet.clone(), literal);
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-wire",
        "(graph-node-process-wire sequencer node-index from-id port to-id inlet)",
        "Wire a connectable port of one slot into an inlet of another slot in the same node patch (the primary binding; a wire pointing up the chain lands next fire, like the track patch bay).",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-wire", &args)?;
            let from = graph_node_process_id_arg(args.get(2), "graph-node-process-wire")?;
            let port = args
                .get(3)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-wire expects a port name".to_string())?;
            let to = graph_node_process_id_arg(args.get(4), "graph-node-process-wire")?;
            let inlet = args
                .get(5)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-wire expects an inlet name".to_string())?;
            let st_inner = Arc::clone(&st);
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let from_pos = chain.slots.iter().position(|s| s.instance_id == from)
                    .ok_or_else(|| format!("graph-node-process-wire: no source slot {}", from.0))?;
                let to_pos = chain.slots.iter().position(|s| s.instance_id == to)
                    .ok_or_else(|| format!("graph-node-process-wire: no target slot {}", to.0))?;
                // A wire pointing up the chain lands next fire, exactly as on a
                // track (the node runner defers it); only a self-wire is refused.
                if to_pos == from_pos {
                    return Err("graph-node-process-wire: a slot cannot feed itself".to_string());
                }
                let to_class = chain.slots[to_pos].class_name.clone();
                let source_def = graph_node_process_def(&st_inner, &chain.slots[from_pos].class_name)
                    .ok_or_else(|| "graph-node-process-wire: unknown source class".to_string())?;
                let port_def = source_def
                    .ports
                    .iter()
                    .find(|p| p.name == port)
                    .ok_or_else(|| format!("graph-node-process-wire: no port {port:?} on source"))?;
                if !port_def.is_connectable() {
                    return Err(format!("graph-node-process-wire: port {port:?} is not connectable"));
                }
                if let Some(target_def) = graph_node_process_def(&st_inner, &to_class) {
                    if !target_def.inlets.iter().any(|i| i.name == inlet) {
                        return Err(format!("graph-node-process-wire: no inlet {inlet:?} on target"));
                    }
                }
                let slot = &mut chain.slots[from_pos];
                slot.unbound_ports.retain(|p| p != &port);
                slot.bindings.insert(
                    port.clone(),
                    Some(crate::process::ParamTarget::ProcessInlet {
                        process: to_class,
                        inlet: inlet.clone(),
                        instance_id: Some(to),
                    }),
                );
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-unwire",
        "(graph-node-process-unwire sequencer node-index instance-id port)",
        "Drop the wire out of one port of a slot in the node's patch.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-unwire", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-unwire")?;
            let port = args
                .get(3)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-unwire expects a port name".to_string())?;
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let slot = chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == id)
                    .ok_or_else(|| format!("graph-node-process-unwire: no slot {}", id.0))?;
                slot.bindings.remove(&port);
                slot.unbound_ports.remove(&port);
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-fanout-add",
        "(graph-node-process-fanout-add sequencer node-index from-id port to-id inlet)",
        "Add a fan-out cable out of a connectable port into another slot's inlet (identity range, the value passes through); returns the new fan-out index.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-fanout-add", &args)?;
            let from = graph_node_process_id_arg(args.get(2), "graph-node-process-fanout-add")?;
            let port = args
                .get(3)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-fanout-add expects a port name".to_string())?;
            let to = graph_node_process_id_arg(args.get(4), "graph-node-process-fanout-add")?;
            let inlet = args
                .get(5)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-fanout-add expects an inlet name".to_string())?;
            let st_inner = Arc::clone(&st);
            let index = edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let from_pos = chain.slots.iter().position(|s| s.instance_id == from)
                    .ok_or_else(|| format!("graph-node-process-fanout-add: no source slot {}", from.0))?;
                let to_pos = chain.slots.iter().position(|s| s.instance_id == to)
                    .ok_or_else(|| format!("graph-node-process-fanout-add: no target slot {}", to.0))?;
                // A wire pointing up the chain lands next fire, exactly as on a
                // track (the node runner defers it); only a self-wire is refused.
                if to_pos == from_pos {
                    return Err("graph-node-process-fanout-add: a slot cannot feed itself".to_string());
                }
                let to_class = chain.slots[to_pos].class_name.clone();
                let source_def = graph_node_process_def(&st_inner, &chain.slots[from_pos].class_name)
                    .ok_or_else(|| "graph-node-process-fanout-add: unknown source class".to_string())?;
                let port_def = source_def
                    .ports
                    .iter()
                    .find(|p| p.name == port)
                    .ok_or_else(|| format!("graph-node-process-fanout-add: no port {port:?} on source"))?;
                if !port_def.is_connectable() {
                    return Err(format!("graph-node-process-fanout-add: port {port:?} is not connectable"));
                }
                let slot = &mut chain.slots[from_pos];
                let (lo, hi) = crate::process::process_slot_output_range(slot);
                let entries = slot.fanout.entry(port.clone()).or_default();
                entries.push(crate::process::ProcessPortFanout {
                    target: crate::process::ParamTarget::ProcessInlet {
                        process: to_class,
                        inlet: inlet.clone(),
                        instance_id: Some(to),
                    },
                    lo,
                    hi,
                });
                Ok(entries.len() - 1)
            })?;
            Ok(EValue::Number(index as f64))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-fanout-remove",
        "(graph-node-process-fanout-remove sequencer node-index instance-id port index)",
        "Remove one fan-out cable (by index) out of a port of a slot in the node's patch.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-fanout-remove", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-fanout-remove")?;
            let port = args
                .get(3)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-fanout-remove expects a port name".to_string())?;
            let index = parse_nonnegative_usize(
                args.get(4).ok_or_else(|| "graph-node-process-fanout-remove expects an index".to_string())?,
                "fan-out index",
            )?;
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let slot = chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == id)
                    .ok_or_else(|| format!("graph-node-process-fanout-remove: no slot {}", id.0))?;
                let entries = slot
                    .fanout
                    .get_mut(&port)
                    .ok_or_else(|| format!("graph-node-process-fanout-remove: no fan-out on {port:?}"))?;
                if index >= entries.len() {
                    return Err(format!("graph-node-process-fanout-remove: no fan-out {index}"));
                }
                entries.remove(index);
                if entries.is_empty() {
                    slot.fanout.remove(&port);
                }
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-map",
        "(graph-node-process-map sequencer node-index instance-id port step-param)",
        "Map a mappable port of a slot in the node's patch onto the fire payload: :transpose, :velocity, :duration or :delay (propagation delay in steps; the port's writes add to / set that field before emit + scatter). nil clears the mapping.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-process-map", &args)?;
            let id = graph_node_process_id_arg(args.get(2), "graph-node-process-map")?;
            let port = args
                .get(3)
                .and_then(graph_key_string)
                .ok_or_else(|| "graph-node-process-map expects a port name".to_string())?;
            let param = match args.get(4) {
                None | Some(EValue::Nil) | Some(EValue::Bool(false)) => None,
                Some(value) => {
                    let name = graph_key_string(value)
                        .ok_or_else(|| "graph-node-process-map expects :transpose, :velocity, :duration or nil".to_string())?;
                    match name.as_str() {
                        "transpose" | "velocity" | "duration" | "delay" => Some(name),
                        other => return Err(format!("graph-node-process-map: {other:?} is not a payload field")),
                    }
                }
            };
            let st_inner = Arc::clone(&st);
            edit_graph_node_process_chain(&st, &manifest, instance, |chain, _| {
                let slot = chain
                    .slots
                    .iter_mut()
                    .find(|slot| slot.instance_id == id)
                    .ok_or_else(|| format!("graph-node-process-map: no slot {}", id.0))?;
                if let Some(def) = graph_node_process_def(&st_inner, &slot.class_name) {
                    let port_def = def
                        .ports
                        .iter()
                        .find(|p| p.name == port)
                        .ok_or_else(|| format!("graph-node-process-map: no port {port:?}"))?;
                    if !port_def.is_mappable() {
                        return Err(format!("graph-node-process-map: port {port:?} is not mappable"));
                    }
                }
                slot.unbound_ports.remove(&port);
                match param {
                    Some(param) => {
                        slot.bindings
                            .insert(port.clone(), Some(crate::process::ParamTarget::StepParam { param }));
                    }
                    None => {
                        slot.bindings.remove(&port);
                    }
                }
                Ok(())
            })?;
            Ok(EValue::Bool(true))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-process-classes",
        "(graph-node-process-classes)",
        "Process classes a node patch can hold, as {:class :label :doc} maps in library order: node-flavoured labels, minus the classes that do nothing on a node fire.",
        move |_args, _ctx| {
            let published = st.published_process_authoring();
            Ok(lisp_list(
                published
                    .defs
                    .iter()
                    .filter(|def| !GRAPH_NODE_HIDDEN_PROCESS_CLASSES.contains(&def.name.as_str()))
                    .map(|def| {
                        let mut m = HashMap::new();
                        m.insert("class".to_string(), lisp_string(def.name.clone()));
                        m.insert("label".to_string(), lisp_string(graph_node_process_label(&def.name)));
                        m.insert("doc".to_string(), lisp_string(def.doc.clone().unwrap_or_default()));
                        EValue::Map(m)
                    })
                    .collect(),
            ))
        },
    );

    let st = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-node-lane-patch",
        "(graph-node-lane-patch sequencer node-index)",
        "The node's patch in the shape of one SEQ.track-lane-patch entry list (slot-index instance-id name class enabled out-ports{name ordinal port-id primary-free readers} in-ports{name ordinal lane kind writers}), with cable ids in the node namespace (1024 + node index) so it can share the lane patchbay renderer with tracks.",
        move |args, _ctx| {
            let (manifest, instance) = graph_node_process_args(&st, "graph-node-lane-patch", &args)?;
            let overrides = st.current_graph_overrides();
            let chain = overrides
                .iter()
                .find(|o| manifest.matches_overrides(o))
                .and_then(|o| {
                    o.node_intrinsics
                        .iter()
                        .find(|n| n.group == manifest.node.name && n.instance == instance)
                })
                .and_then(|n| n.process_chain.clone())
                .unwrap_or_default();
            Ok(graph_node_lane_patch_value(&st, &chain, GRAPH_NODE_LANE_PATCH_NAMESPACE_BASE + instance))
        },
    );
}
