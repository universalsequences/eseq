use super::graph_dsl::{graph_keyword, graph_parse_swing_spec, graph_swing_value_from_args};
use super::*;

pub(crate) type SharedGraphNodeContext = Arc<Mutex<Option<GraphNodeContext>>>;

/// Per-node-event context for a graph-mode `:update`, bound while the update body
/// evaluates so the `node-*` accessors read it. Carries only musical/symbolic
/// coordinates and the node's resolved input + behavioral params/state (no samples).
pub(crate) struct GraphNodeContext {
    node_index: usize,
    input: f64,
    energy: f64,
    tick_index: u64,
    beat: f64,
    resolution: Timebase,
    delay_steps: u32,
    /// Behavioral params (`node-param`), prototype defaults + per-instance plocks.
    params: HashMap<String, f64>,
    /// Author-defined state cells (`node-state`/`node-set!`) beyond engine `energy`.
    state: HashMap<String, f64>,
    /// The payload that arrived this boundary (`node-input-event`), if any (Ext 1).
    input_event: Option<crate::graph::GraphPayload>,
    dampen_incoming: Option<f64>,
    recover_incoming: Option<f64>,
    reset_graph_state: bool,
}

pub(super) struct CompiledGraphUpdate {
    pub(super) source: String,
    pub(super) callback: EValue,
}

impl ScratchControlRuntime {
    fn graph_update_callback(&mut self, id: u64, source: &str) -> Result<EValue, String> {
        if let Some(compiled) = self.graph_updates.get(&id) {
            if compiled.source == source {
                return Ok(compiled.callback.clone());
            }
        }

        let wrapped = format!("(lambda (self) {source})");
        let callback = self
            .runtime
            .eval_str(&wrapped)
            .map_err(|e| format!("failed to compile graph update: {e:?}; source={source}"))?
            .ok_or_else(|| "graph update compilation produced no callback".to_string())?;
        match callback {
            EValue::Closure(_, _) | EValue::NativeFunction(_) => {
                self.graph_updates.insert(
                    id,
                    CompiledGraphUpdate {
                        source: source.to_string(),
                        callback: callback.clone(),
                    },
                );
                Ok(callback)
            }
            other => Err(format!(
                "graph update must compile to a callable, got {}",
                eseqlisp::vm::format_lisp_value(&other)
            )),
        }
    }

    /// Run a graph node's `:update` rule for one evaluation boundary and report
    /// whether it fired. The behavioral params (prototype defaults; per-instance plocks
    /// later) and the engine-integrated `energy` are bound via the `node-*` accessors;
    /// the truthiness of the body's result is the fire decision. With no `:update`
    /// body, falls back to the neural rule (fire when `energy >= threshold`).
    ///
    /// v1a: `energy` is engine-owned (integrated + reset by [`crate::graph::GraphRuntime`]),
    /// so the body is a pure predicate. Author state cells and emit/relay arrive in v1b.
    pub fn invoke_graph_update(
        &mut self,
        manifest: &crate::graph::GraphManifest,
        eval: &crate::graph::NodeEval,
    ) -> Result<crate::graph::NodeFire, String> {
        let params = eval.params.clone();
        let Some(source) = manifest.node.update_source.as_deref() else {
            let threshold = params.get("threshold").copied().unwrap_or(1.0);
            return Ok(crate::graph::NodeFire {
                fired: eval.energy >= threshold,
                ..crate::graph::NodeFire::default()
            });
        };
        let callback = self.graph_update_callback(manifest.id, source)?;
        {
            let mut ctx = self
                .graph_node
                .lock()
                .map_err(|_| "failed to lock graph node context".to_string())?;
            *ctx = Some(GraphNodeContext {
                node_index: eval.node_index,
                input: eval.input,
                energy: eval.energy,
                tick_index: eval.tick_index,
                beat: eval.beat,
                resolution: eval.resolution,
                delay_steps: eval.delay_steps,
                params,
                state: HashMap::new(),
                input_event: eval.input_event,
                dampen_incoming: None,
                recover_incoming: None,
                reset_graph_state: false,
            });
        }
        let result = self
            .runtime
            .invoke(callback, vec![EValue::Number(eval.node_index as f64)])
            .map_err(|e| format!("{e:?}"));
        if std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some() {
            eprintln!(
                "[graph-update] id={} node={} src={source:?} result={result:?}",
                manifest.id, eval.node_index
            );
        }
        let mut dampen_incoming = None;
        let mut recover_incoming = None;
        let mut reset_graph_state = false;
        if let Ok(mut ctx) = self.graph_node.lock() {
            if let Some(ctx) = ctx.take() {
                dampen_incoming = ctx.dampen_incoming;
                recover_incoming = ctx.recover_incoming;
                reset_graph_state = ctx.reset_graph_state;
            }
        }
        let fired = matches!(&result, Ok(Some(v)) if evalue_is_truthy(v));
        let emit = match &result {
            Ok(Some(v)) => parse_emit_spec(v),
            _ => None,
        };
        result?;
        Ok(crate::graph::NodeFire {
            fired,
            emit,
            reset_graph_state,
            dampen_incoming,
            recover_incoming,
        })
    }
}

/// Lisp truthiness: everything is true except `false` and `nil`.
fn evalue_is_truthy(value: &EValue) -> bool {
    !matches!(value, EValue::Bool(false) | EValue::Nil)
}

/// Marker key stamped onto the Map that `(emit …)` returns, so a `:update`'s result is
/// distinguishable from any other truthy value (a plain `true`, a number, or an
/// `in-event` map) when [`ScratchControlRuntime::invoke_graph_update`] decodes it.
const EMIT_MARKER: &str = "__emit";

/// Decode the shaped event a `:update` body returned. Only a Map carrying [`EMIT_MARKER`]
/// (i.e. the value `(emit …)` produced) yields an [`crate::graph::EmitSpec`]; any other
/// truthy value means "fire with the legacy default" and returns `None`.
fn parse_emit_spec(value: &EValue) -> Option<crate::graph::EmitSpec> {
    let EValue::Map(map) = value else {
        return None;
    };
    map.get(EMIT_MARKER)?;
    let field = |key: &str| -> Option<f32> {
        map.get(key).and_then(|cell| match &*cell.borrow() {
            EValue::Number(n) => Some(*n as f32),
            _ => None,
        })
    };
    Some(crate::graph::EmitSpec {
        note: field("note"),
        velocity: field("vel"),
        duration: map.get("dur").and_then(|cell| match &*cell.borrow() {
            EValue::Number(n) => Some(crate::graph::GraphDurationSpec::Beats { value: *n }),
            value if graph_keyword(value).as_deref() == Some("seed") => {
                Some(crate::graph::GraphDurationSpec::Seed)
            }
            _ => None,
        }),
        swing: map
            .get("swing")
            .and_then(|cell| graph_parse_swing_spec(&cell.borrow()).ok()),
    })
}

pub(super) fn build_graph_emit_value(args: &[EValue]) -> Result<EValue, String> {
    let mut map: HashMap<String, std::rc::Rc<std::cell::RefCell<EValue>>> = HashMap::new();
    map.insert(
        EMIT_MARKER.to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(EValue::Bool(true))),
    );
    let mut i = 0;
    while i < args.len() {
        let key = graph_emit_key(args.get(i))
            .ok_or("emit expects keyword/value pairs, e.g. (emit :note 60 :vel 0.8)")?;
        let field = match key.as_str() {
            "note" => "note",
            "vel" | "velocity" => "vel",
            "dur" | "duration" => "dur",
            "swing" => "swing",
            other => return Err(format!("emit: unknown field :{other}")),
        };
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("emit field :{key} expects a value"))?;
        match field {
            "note" | "vel" => {
                let EValue::Number(value) = value else {
                    return Err(format!("emit field :{key} expects a number"));
                };
                map.insert(
                    field.to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(*value))),
                );
            }
            "dur" => {
                match value {
                    EValue::Number(_) => {}
                    value if graph_keyword(value).as_deref() == Some("seed") => {}
                    _ => return Err("emit field :dur expects a number or :seed".to_string()),
                }
                map.insert(
                    field.to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(value.clone())),
                );
            }
            "swing" => {
                graph_parse_swing_spec(value)
                    .map_err(|error| format!("emit field :swing {error}"))?;
                map.insert(
                    field.to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(value.clone())),
                );
            }
            _ => unreachable!(),
        }
        i += 2;
    }
    Ok(EValue::Map(map))
}

fn graph_emit_key(value: Option<&EValue>) -> Option<String> {
    match value {
        Some(EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k)) => {
            Some(k.trim_start_matches(':').to_string())
        }
        _ => None,
    }
}

/// Register the `node-*` accessors a graph-mode `:update` reads. They read the
/// currently-bound [`GraphNodeContext`] (set by `invoke_graph_update`) and ignore
/// their `self` argument (the context is ambient, like the `gen-*` builtins).
pub(super) fn register_graph_node_natives(
    runtime: &mut Runtime,
    graph_node: SharedGraphNodeContext,
) {
    fn ctx_key(value: Option<&EValue>) -> Option<String> {
        match value {
            Some(EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k)) => {
                Some(k.trim_start_matches(':').to_string())
            }
            _ => None,
        }
    }

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-input",
        "(node-input self)",
        "The reduced gather result arriving at this node this evaluation boundary.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-input called outside :update")?;
            Ok(EValue::Number(ctx.input))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-index",
        "(node-index self)",
        "This node's instance index within the shape.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-index called outside :update")?;
            Ok(EValue::Number(ctx.node_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-tick",
        "(node-tick self)",
        "0-based count of this node's evaluation boundaries since reset.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-tick called outside :update")?;
            Ok(EValue::Number(ctx.tick_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-param",
        "(node-param self :key)",
        "Read a behavioral param of this node (prototype default + per-instance plock).",
        move |args, _ctx| {
            let key =
                ctx_key(args.get(1).or_else(|| args.first())).ok_or("node-param expects a key")?;
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-param called outside :update")?;
            Ok(EValue::Number(ctx.params.get(&key).copied().unwrap_or(0.0)))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-state",
        "(node-state self :key)",
        "Read a runtime state cell of this node (engine `energy`, or an author cell).",
        move |args, _ctx| {
            let key =
                ctx_key(args.get(1).or_else(|| args.first())).ok_or("node-state expects a key")?;
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("node-state called outside :update")?;
            let value = if key == "energy" {
                ctx.energy
            } else {
                ctx.state.get(&key).copied().unwrap_or(0.0)
            };
            Ok(EValue::Number(value))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-input-event",
        "(node-input-event self)",
        "The payload (event) that arrived at this node this boundary, or nil (Ext 1).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_ref()
                .ok_or("node-input-event called outside :update")?;
            Ok(payload_to_event(ctx.input_event))
        },
    );

    fn event_field(args: &[EValue], key: &str) -> EValue {
        if let Some(EValue::Map(map)) = args.first() {
            if let Some(cell) = map.get(key) {
                if let EValue::Number(n) = &*cell.borrow() {
                    return EValue::Number(*n);
                }
            }
        }
        EValue::Number(0.0)
    }
    runtime.register_native_with_docs(
        "event-note",
        "(event-note ev)",
        "Read the note (transpose) field off a relayed event (Ext 1).",
        move |args, _ctx| Ok(event_field(&args, "note")),
    );
    runtime.register_native_with_docs(
        "event-vel",
        "(event-vel ev)",
        "Read the velocity field off a relayed event (Ext 1).",
        move |args, _ctx| Ok(event_field(&args, "vel")),
    );
    runtime.register_native_with_docs(
        "event-dur",
        "(event-dur ev)",
        "Read the duration-in-beats field off a relayed event.",
        move |args, _ctx| Ok(event_field(&args, "dur")),
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "node-set!",
        "(node-set! self :key value)",
        "Write an author state cell of this node (v1a: engine `energy` is engine-owned).",
        move |args, _ctx| {
            let key = ctx_key(args.get(1)).ok_or("node-set! expects a key")?;
            let value = match args.get(2) {
                Some(EValue::Number(n)) => *n,
                _ => return Err("node-set! expects (node-set! self :key number)".to_string()),
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_mut().ok_or("node-set! called outside :update")?;
            if key != "energy" {
                ctx.state.insert(key, value);
            }
            Ok(EValue::Number(value))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "dampen-incoming",
        "(dampen-incoming self amount)",
        "Request dampening for incoming edges that triggered this node if the firing commits.",
        move |args, _ctx| {
            let amount = match args.get(1).or_else(|| args.first()) {
                Some(EValue::Number(n)) => *n,
                _ => {
                    return Err("dampen-incoming expects (dampen-incoming self amount)".to_string());
                }
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_mut()
                .ok_or("dampen-incoming called outside :update")?;
            ctx.dampen_incoming = Some(amount);
            // Returns nil so a `:update` ending on this edge-effect reads as "no fire".
            Ok(EValue::Nil)
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "recover-incoming",
        "(recover-incoming self factor)",
        "Request recovery for all incoming edges if this node does not fire.",
        move |args, _ctx| {
            let factor = match args.get(1).or_else(|| args.first()) {
                Some(EValue::Number(n)) => *n,
                _ => {
                    return Err(
                        "recover-incoming expects (recover-incoming self factor)".to_string()
                    );
                }
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_mut()
                .ok_or("recover-incoming called outside :update")?;
            ctx.recover_incoming = Some(factor);
            // Returns nil so the common `(if fire? (emit …) (recover-incoming …))`
            // shape skips when the else-branch runs (the chosen no-fire form).
            Ok(EValue::Nil)
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "reset-graph-state",
        "(reset-graph-state)",
        "Request a full graph runtime-state reset if this node's firing is accepted.",
        move |_args, _ctx| {
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard
                .as_mut()
                .ok_or("reset-graph-state called outside :update")?;
            ctx.reset_graph_state = true;
            // Returns nil so callers can place it before the final `(emit …)` in a
            // `do` without changing the fire decision.
            Ok(EValue::Nil)
        },
    );

    // ── Terse, self-less surface (Ext B) ──────────────────────────────────────────
    // The node context is ambient, so these take no `self`. They read/write the same
    // bound `GraphNodeContext` as the `node-*` accessors above; the older `node-*`
    // forms remain as aliases so existing definitions keep working.

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "param",
        "(param :key)",
        "Read a behavioral param of this node (prototype default + per-instance plock).",
        move |args, _ctx| {
            let key = ctx_key(args.first()).ok_or("param expects (param :key)")?;
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("param called outside :update")?;
            Ok(EValue::Number(ctx.params.get(&key).copied().unwrap_or(0.0)))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "energy",
        "(energy)",
        "Read this node's engine-owned integrated energy.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("energy called outside :update")?;
            Ok(EValue::Number(ctx.energy))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "set-state!",
        "(set-state! :key value)",
        "Write an author state cell of this node (engine `energy` is engine-owned).",
        move |args, _ctx| {
            let key = ctx_key(args.first()).ok_or("set-state! expects a key")?;
            let value = match args.get(1) {
                Some(EValue::Number(n)) => *n,
                _ => return Err("set-state! expects (set-state! :key number)".to_string()),
            };
            let mut guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_mut().ok_or("set-state! called outside :update")?;
            if key != "energy" {
                ctx.state.insert(key, value);
            }
            Ok(EValue::Number(value))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "input",
        "(input)",
        "The reduced gather result arriving at this node this evaluation boundary.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("input called outside :update")?;
            Ok(EValue::Number(ctx.input))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "index",
        "(index)",
        "This node's instance index within the shape.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("index called outside :update")?;
            Ok(EValue::Number(ctx.node_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "step",
        "(step)",
        "0-based count of this node's evaluation boundaries since reset.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("step called outside :update")?;
            Ok(EValue::Number(ctx.tick_index as f64))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-event",
        "(in-event)",
        "The payload (event) that arrived at this node this boundary, or nil.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-event called outside :update")?;
            Ok(payload_to_event(ctx.input_event))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-note",
        "(in-note)",
        "The note of the event arriving this boundary (0 if nothing arrived).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-note called outside :update")?;
            Ok(EValue::Number(
                ctx.input_event.map(|p| p.note as f64).unwrap_or(0.0),
            ))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-vel",
        "(in-vel)",
        "The velocity of the event arriving this boundary (1.0 if nothing arrived).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-vel called outside :update")?;
            Ok(EValue::Number(
                ctx.input_event.map(|p| p.velocity as f64).unwrap_or(1.0),
            ))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "in-dur",
        "(in-dur)",
        "The duration in beats of the event arriving this boundary (one node step if nothing arrived).",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("in-dur called outside :update")?;
            let fallback = ctx
                .resolution
                .step_beats(crate::graph::GRAPH_RESOLUTION_REF_STEPS);
            Ok(EValue::Number(
                ctx.input_event
                    .map(|p| p.duration_beats as f64)
                    .unwrap_or(fallback),
            ))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "steps",
        "(steps n)",
        "Duration in beats for n steps of the current graph node's resolved resolution.",
        move |args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("steps called outside :update")?;
            let Some(EValue::Number(n)) = args.first() else {
                return Err("steps expects a numeric step count".to_string());
            };
            Ok(EValue::Number(
                n.max(0.0)
                    * ctx
                        .resolution
                        .step_beats(crate::graph::GRAPH_RESOLUTION_REF_STEPS),
            ))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "delay",
        "(delay)",
        "Duration in beats for this graph node's resolved delay.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("delay called outside :update")?;
            Ok(EValue::Number(
                ctx.delay_steps as f64
                    * ctx
                        .resolution
                        .step_beats(crate::graph::GRAPH_RESOLUTION_REF_STEPS),
            ))
        },
    );

    let gn = Arc::clone(&graph_node);
    runtime.register_native_with_docs(
        "seed",
        "(seed)",
        "Duration in beats carried by the incoming seed/payload.",
        move |_args, _ctx| {
            let guard = gn.lock().map_err(|_| "graph node context".to_string())?;
            let ctx = guard.as_ref().ok_or("seed called outside :update")?;
            Ok(EValue::Number(
                ctx.input_event
                    .map(|p| p.duration_beats as f64)
                    .unwrap_or_else(|| {
                        ctx.resolution
                            .step_beats(crate::graph::GRAPH_RESOLUTION_REF_STEPS)
                    }),
            ))
        },
    );

    runtime.register_native_with_docs(
        "swing",
        "(swing pct [:16|:8|:4|:2])",
        "Graph swing timing policy: 50 is straight, 75 is maximum delayed swing.",
        move |args, _ctx| graph_swing_value_from_args(&args),
    );

    runtime.register_native_with_docs(
        "emit",
        "(emit :note n :vel v :dur d :swing s)",
        "Fire this node with a shaped event. Each named field overrides the emitted and \
         propagated payload; unnamed fields relay the incoming event verbatim. Returning \
         it from `:update` is the fire decision (truthy).",
        move |args, _ctx| build_graph_emit_value(&args),
    );
}

/// Build the `{note, vel}` Map an `:update` sees for an arrived payload, or nil.
fn payload_to_event(payload: Option<crate::graph::GraphPayload>) -> EValue {
    match payload {
        Some(payload) => {
            let mut map = HashMap::new();
            map.insert(
                "note".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(payload.note as f64))),
            );
            map.insert(
                "vel".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(
                    payload.velocity as f64,
                ))),
            );
            map.insert(
                "dur".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(EValue::Number(
                    payload.duration_beats as f64,
                ))),
            );
            EValue::Map(map)
        }
        None => EValue::Nil,
    }
}
