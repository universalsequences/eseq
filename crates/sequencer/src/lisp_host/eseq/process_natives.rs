/*!
Registers the live-coding natives for first-class processes and channels.

This is the native surface of the eseqlisp process layer: definition and
lifecycle (`process`, `processes`, `start`, `stop`, `connect!`, `patch`,
`inlet`/`process-inlet`), channel plumbing (`defchan`, `send`, `hear`,
`tap`), per-tick emission and event shaping inside a process body (`emit`,
`lane`/`lane!`, `ratchet!`, `veto!`, `transpose!`, `gate?`,
`current-note`, `step-length`, `timebase-beats`), field access
(`scalar-field`, `gate-field`, `pitch-field`, `field-domain`,
`field-nearest-delta`), and parameter targeting (`target-set!`,
`target-add!`). Argument parsing lives in `process_dsl_parse`; the engine
that folds these processes each block is `crate::runtime::process`.
*/

use super::super::*;

pub(in crate::lisp_host) fn publish_process_authoring(
    process_authoring: &SharedProcessAuthoring,
    publish: &Option<ProcessPublishHook>,
) {
    let Some(publish) = publish else {
        return;
    };
    if let Ok(registry) = process_authoring.lock() {
        match registry.snapshot().to_published() {
            Ok(snapshot) => publish(snapshot),
            Err(error) => eprintln!("[process] publish error: {error}"),
        }
    }
}
pub(in crate::lisp_host) fn register_process_target_hint_constructors(runtime: &mut Runtime) {
    for (name, arity) in [
        ("step-param", 1usize),
        ("param-tag", 1),
        ("instrument-param", 1),
        ("effect-param", 2),
        ("midi-fx-target", 2),
    ] {
        runtime.register_native_with_docs(
            name,
            name,
            "Construct a process target hint expression.",
            move |args, _ctx| {
                if args.len() != arity {
                    return Err(format!("{name} expects {arity} argument(s)"));
                }
                Ok(process_list(
                    std::iter::once(EValue::Symbol(name.to_string())).chain(args.into_iter()),
                ))
            },
        );
    }
    runtime.register_native_with_docs(
        "process-inlet",
        "(process-inlet :class :inlet)",
        "Construct a process-inlet target selector for a connectable process port.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("process-inlet expects a process class and inlet".to_string());
            }
            Ok(process_list(
                std::iter::once(EValue::Symbol("process-inlet".to_string()))
                    .chain(args.into_iter()),
            ))
        },
    );
}

pub(in crate::lisp_host) fn register_process_natives(
    runtime: &mut Runtime,
    process_authoring: SharedProcessAuthoring,
    process_eval: SharedProcessEvalContext,
    publish: Option<ProcessPublishHook>,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    register_execution_natives: bool,
) {
    if let Some(state) = process_chain_state.as_ref() {
        register_graph_homeostat_natives(
            runtime,
            Arc::clone(state),
            Arc::clone(&process_eval),
        );
    }
    let process_authoring_for_inline_metadata = Arc::clone(&process_authoring);
    runtime.set_inline_widget_metadata_resolver(Rc::new(move |callee, inlet| {
        let registry = process_authoring_for_inline_metadata.lock().ok()?;
        let inlet = registry
            .defs
            .iter()
            .find(|definition| definition.name == callee)?
            .inlets
            .iter()
            .find(|definition| definition.name == inlet)?;
        let step = matches!(
            inlet.kind,
            crate::process::ProcessInletKind::Int
                | crate::process::ProcessInletKind::Gate
                | crate::process::ProcessInletKind::Track
        )
        .then_some(1.0);
        Some(eseqlisp::vm::InlineWidgetMetadata {
            min: inlet.min.map(f64::from),
            max: inlet.max.map(f64::from),
            step,
        })
    }));
    let process_authoring_for_hook = Arc::clone(&process_authoring);
    let publish_for_hook = publish.clone();
    runtime.add_global_store_hook(Rc::new(move |name, value| {
        let EValue::HostHandle { kind, id, .. } = value else {
            return;
        };
        let handle_id = crate::process::AuthoredHandleId(*id);
        if let Ok(mut registry) = process_authoring_for_hook.lock() {
            match kind.as_str() {
                "process" => registry.name_instance(handle_id, name),
                "channel" => registry.name_channel(handle_id, name),
                _ => {}
            }
        }
        publish_process_authoring(&process_authoring_for_hook, &publish_for_hook);
    }));

    register_process_target_hint_constructors(runtime);

    let process_authoring_for_inlet = Arc::clone(&process_authoring);
    runtime.register_native_with_docs(
        "inlet",
        "(inlet process :inlet)",
        "Construct an instance-specific process-inlet target selector for connect!.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("inlet expects a process handle and inlet name".to_string());
            }
            let Some(EValue::HostHandle { kind, id, .. }) = args.first() else {
                return Err("inlet expects a process handle".to_string());
            };
            if kind != "process" {
                return Err("inlet expects a process handle".to_string());
            }
            let inlet = process_symbol_name(
                args.get(1)
                    .ok_or_else(|| "inlet expects an inlet name".to_string())?,
            )?;
            let registry = process_authoring_for_inlet
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let instance = registry
                .instances
                .iter()
                .find(|entry| entry.handle_id.0 == *id)
                .ok_or_else(|| "unknown process handle".to_string())?;
            let def = registry
                .defs
                .iter()
                .find(|def| def.name == instance.class_name)
                .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
            if !def.inlets.iter().any(|entry| entry.name == inlet) {
                return Err(format!(
                    "process '{}' has no inlet '{}'",
                    instance.class_name, inlet
                ));
            }
            Ok(process_list([
                EValue::Symbol(PROCESS_INLET_INSTANCE_TARGET_TAG.to_string()),
                EValue::Number(*id as f64),
                EValue::Symbol(instance.class_name.clone()),
                EValue::Symbol(inlet),
            ]))
        },
    );

    let process_authoring_for_def = Arc::clone(&process_authoring);
    let publish_for_def = publish.clone();
    let chain_state_for_def = process_chain_state.clone();
    runtime.register_vm_native_with_docs_and_keywords(
        "def-process",
        "(def-process name :doc text :in (...) :out (...) :state (...) :every duration :seed policy :target target :targets (...) :listen (...) :phase value :init body :run body)",
        "Define a scheduler-side musical process class. Event handlers use dynamic :on-<listen-name> keys and therefore are not a fixed completion set.",
        [
            ":doc", ":in", ":out", ":state", ":every", ":seed", ":target", ":targets",
            ":listen", ":phase", ":init", ":run",
        ],
        move |args, vm| match register_process_def(
            args,
            vm,
            &process_authoring_for_def,
            chain_state_for_def.clone(),
            publish_for_def.clone(),
        ) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[process] def-process error: {error}");
                EValue::Bool(false)
            }
        },
    );

    let process_authoring_for_def_acc = Arc::clone(&process_authoring);
    let publish_for_def_acc = publish.clone();
    let chain_state_for_def_acc = process_chain_state.clone();
    runtime.register_vm_native_with_docs(
        "def-accumulator",
        "(def-accumulator name :target (step-param :transpose) :amount (...) :reset :lane :range (lo hi) :mode :wrap)",
        "Define a replay-safe lane-folding step process accumulator.",
        move |args, vm| match register_process_accumulator_def(
            args,
            vm,
            &process_authoring_for_def_acc,
            chain_state_for_def_acc.clone(),
            publish_for_def_acc.clone(),
        ) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[process] def-accumulator error: {error}");
                EValue::Bool(false)
            }
        },
    );

    let process_authoring_for_defchan = Arc::clone(&process_authoring);
    let publish_for_defchan = publish.clone();
    runtime.register_native_with_docs(
        "defchan",
        "(defchan name [initial])",
        "Declare a late-bound musical value/message channel.",
        move |args, _ctx| {
            let name = process_symbol_name(
                args.first()
                    .ok_or_else(|| "defchan expects a channel name".to_string())?,
            )?;
            let mut registry = process_authoring_for_defchan
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            let handle_id = crate::process::AuthoredHandleId(registry.next_id());
            let initial = args.get(1).cloned();
            registry.channels.push(crate::process::AuthoredChannel {
                handle_id,
                name: Some(name.clone()),
                initial,
                message_only: args.len() == 1,
            });
            registry.channel_handles.insert(handle_id.0, name);
            let handle =
                process_channel_handle(Arc::clone(&process_authoring_for_defchan), handle_id);
            drop(registry);
            publish_process_authoring(&process_authoring_for_defchan, &publish_for_defchan);
            Ok(handle)
        },
    );

    let process_authoring_for_start = Arc::clone(&process_authoring);
    let publish_for_start = publish.clone();
    runtime.register_native_with_docs(
        "start",
        "(start process)",
        "Start a process instance.",
        move |args, _ctx| {
            set_process_running(&process_authoring_for_start, args.first(), true)?;
            publish_process_authoring(&process_authoring_for_start, &publish_for_start);
            Ok(EValue::Bool(true))
        },
    );

    let process_authoring_for_stop = Arc::clone(&process_authoring);
    let publish_for_stop = publish.clone();
    runtime.register_native_with_docs(
        "stop",
        "(stop process)",
        "Stop a process instance.",
        move |args, _ctx| {
            set_process_running(&process_authoring_for_stop, args.first(), false)?;
            publish_process_authoring(&process_authoring_for_stop, &publish_for_stop);
            Ok(EValue::Bool(true))
        },
    );

    let process_authoring_for_every = Arc::clone(&process_authoring);
    let publish_for_every = publish.clone();
    let chain_state_for_every = process_chain_state.clone();
    runtime.register_native_with_docs(
        "every",
        "(every time body...)",
        "Create and start an anonymous process that runs on a quantized musical interval.",
        move |args, _ctx| {
            let interval = args
                .first()
                .ok_or_else(|| "every expects a time expression".to_string())
                .and_then(parse_process_time_expr)?;
            let run_source = process_body_source(
                args.get(1..)
                    .ok_or_else(|| "every expects a body".to_string())?,
            )?;
            let handle = construct_process_instance(
                &process_authoring_for_every,
                "__anonymous_every",
                Vec::new(),
                true,
                true,
                Some(interval),
                Some(run_source),
                false,
                chain_state_for_every.clone(),
                publish_for_every.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_every, &publish_for_every);
            Ok(handle)
        },
    );

    let process_authoring_for_after = Arc::clone(&process_authoring);
    let publish_for_after = publish.clone();
    let chain_state_for_after = process_chain_state.clone();
    runtime.register_native_with_docs(
        "after",
        "(after time body...)",
        "Create and start an anonymous one-shot process that runs after a musical delay.",
        move |args, _ctx| {
            let delay = args
                .first()
                .ok_or_else(|| "after expects a time expression".to_string())
                .and_then(parse_process_time_expr)?;
            let run_source = process_body_source(
                args.get(1..)
                    .ok_or_else(|| "after expects a body".to_string())?,
            )?;
            let handle = construct_process_instance(
                &process_authoring_for_after,
                "__anonymous_after",
                Vec::new(),
                true,
                true,
                Some(delay),
                Some(run_source),
                true,
                chain_state_for_after.clone(),
                publish_for_after.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_after, &publish_for_after);
            Ok(handle)
        },
    );

    let process_authoring_for_on = Arc::clone(&process_authoring);
    let publish_for_on = publish.clone();
    runtime.register_native_with_docs(
        "on",
        "(on source callback)",
        "Create and start an anonymous process that runs when an event source fires.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("on expects source and callback".to_string());
            }
            let source = parse_process_event_source_ref(&process_authoring_for_on, &args[0])?;
            let handle = construct_anonymous_listener_process(
                &process_authoring_for_on,
                "on",
                source,
                &args[1],
                publish_for_on.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_on, &publish_for_on);
            Ok(handle)
        },
    );

    let process_authoring_for_patch = Arc::clone(&process_authoring);
    let publish_for_patch = publish.clone();
    runtime.register_native_with_docs(
        "patch",
        "(patch source target)",
        "Connect a process outlet/channel source to a process inlet/channel target.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("patch expects source and target".to_string());
            }
            let source = parse_process_source_ref(&process_authoring_for_patch, &args[0])?;
            let target = parse_process_target_ref(&process_authoring_for_patch, &args[1])?;
            process_authoring_for_patch
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?
                .patches
                .push(crate::process::AuthoredPatch { source, target });
            publish_process_authoring(&process_authoring_for_patch, &publish_for_patch);
            Ok(EValue::Bool(true))
        },
    );

    let process_authoring_for_tap = Arc::clone(&process_authoring);
    let publish_for_tap = publish.clone();
    runtime.register_native_with_docs(
        "tap",
        "(tap source callback)",
        "Create and start an anonymous process that runs whenever a source publishes.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("tap expects source and callback".to_string());
            }
            let source = parse_process_event_source_ref(&process_authoring_for_tap, &args[0])?;
            let handle = construct_anonymous_listener_process(
                &process_authoring_for_tap,
                "tap",
                source,
                &args[1],
                publish_for_tap.clone(),
            )?;
            publish_process_authoring(&process_authoring_for_tap, &publish_for_tap);
            Ok(handle)
        },
    );

    let process_authoring_for_send = Arc::clone(&process_authoring);
    let process_eval_for_send = Arc::clone(&process_eval);
    let publish_for_send = publish.clone();
    runtime.register_native_with_docs(
        "send",
        "(send channel value)",
        "Publish a value/message to a process channel.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("send expects channel and value".to_string());
            }
            let channel = parse_channel_name(&process_authoring_for_send, &args[0])?;
            let value = args[1].clone();
            let mut eval = process_eval_for_send
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            if let Some(ctx) = eval.as_mut() {
                ensure_process_run_scope(ctx, "send")?;
                // Runtime propagation is performed by the scheduler after the run
                // result returns.
                ctx.outputs.push(crate::process::ProcessOutput {
                    name: format!("__chan:{channel}"),
                    value: value.clone(),
                });
            } else {
                drop(eval);
                let mut registry = process_authoring_for_send
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                if let Some(authored) = registry
                    .channels
                    .iter_mut()
                    .rev()
                    .find(|authored| authored.name.as_deref() == Some(channel.as_str()))
                {
                    if !authored.message_only {
                        authored.initial = Some(value.clone());
                    }
                }
                drop(registry);
                publish_process_authoring(&process_authoring_for_send, &publish_for_send);
            }
            Ok(value)
        },
    );

    if !register_execution_natives {
        let process_authoring_for_ps = Arc::clone(&process_authoring);
        runtime.register_native_with_docs(
            "ps",
            "(ps)",
            "Return authored process/channel status.",
            move |_args, _ctx| {
                let registry = process_authoring_for_ps
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                Ok(process_status_value(&registry))
            },
        );
        return;
    }

    let process_eval_for_in = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "in",
        "(in :name)",
        "Read a process inlet.",
        move |args, _ctx| {
            let key = process_key_arg(args.first(), "in")?;
            let guard = process_eval_for_in
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("in called outside process execution".to_string());
            };
            Ok(ctx.inlets.get(&key).cloned().unwrap_or(EValue::Nil))
        },
    );

    let process_eval_for_out = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "out",
        "(out :name value)",
        "Publish a process outlet value.",
        move |args, _ctx| {
            let key = process_key_arg(args.first(), "out")?;
            let value = args.get(1).cloned().unwrap_or(EValue::Nil);
            let mut guard = process_eval_for_out
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("out called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "out")?;
            ctx.outputs.push(crate::process::ProcessOutput {
                name: key,
                value: value.clone(),
            });
            Ok(value)
        },
    );

    let process_eval_for_state_get = Arc::clone(&process_eval);
    runtime.register_native("__process-state-get", move |args, _ctx| {
        let key = process_key_arg(args.first(), "__process-state-get")?;
        let guard = process_eval_for_state_get
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?;
        let Some(ctx) = guard.as_ref() else {
            return Err("__process-state-get called outside process execution".to_string());
        };
        Ok(ctx.state.get(&key).cloned().unwrap_or(EValue::Nil))
    });

    let process_eval_for_state_set = Arc::clone(&process_eval);
    runtime.register_native("__process-state-set!", move |args, _ctx| {
        let key = process_key_arg(args.first(), "__process-state-set!")?;
        let value = args.get(1).cloned().unwrap_or(EValue::Nil);
        let mut guard = process_eval_for_state_set
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?;
        let Some(ctx) = guard.as_mut() else {
            return Err("__process-state-set! called outside process execution".to_string());
        };
        ensure_process_run_scope(ctx, "__process-state-set!")?;
        ctx.state.insert(key, value.clone());
        Ok(value)
    });

    let process_eval_for_event = Arc::clone(&process_eval);
    runtime.register_native("__process-event-value", move |_args, _ctx| {
        let guard = process_eval_for_event
            .lock()
            .map_err(|_| "failed to lock process eval context".to_string())?;
        let Some(ctx) = guard.as_ref() else {
            return Err("__process-event-value called outside process execution".to_string());
        };
        Ok(ctx.event.clone().unwrap_or(EValue::Nil))
    });

    let process_eval_for_transpose = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "transpose!",
        "(transpose! semitones)",
        "Set scheduler global transpose for future note-ons.",
        move |args, _ctx| {
            let Some(EValue::Number(value)) = args.first() else {
                return Err("transpose! expects a number".to_string());
            };
            let mut guard = process_eval_for_transpose
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("transpose! called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "transpose!")?;
            ctx.transpose = Some(*value as f32);
            Ok(EValue::Number(*value))
        },
    );

    let process_eval_for_veto = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "veto!",
        "(veto!)",
        "Suppress the scheduler-owned base event for this step while allowing later processes to run.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("veto! expects no arguments".to_string());
            }
            let mut guard = process_eval_for_veto
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("veto! called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "veto!")?;
            if ctx.step_context.is_none() {
                return Err("veto! requires a scheduler step event context".to_string());
            }
            ctx.commands
                .push(crate::process::ProcessRunCommand::VetoBaseEvent);
            Ok(EValue::Bool(true))
        },
    );

    let process_eval_for_step_length = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "step-length",
        "(step-length)",
        "Return the current scheduler grid step length in beats.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("step-length expects no arguments".to_string());
            }
            let guard = process_eval_for_step_length
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("step-length called outside process execution".to_string());
            };
            let Some(step_context) = ctx.step_context.as_ref() else {
                return Err("step-length requires a scheduler step event context".to_string());
            };
            Ok(EValue::Number(step_context.step_beats as f64))
        },
    );

    runtime.register_native_with_docs(
        "timebase-beats",
        "(timebase-beats index-or-name)",
        "Convert a sequencer timebase index/name to quarter-note beats.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("timebase-beats expects one timebase".to_string());
            }
            let timebase = match &args[0] {
                EValue::Number(index) => {
                    if !index.is_finite()
                        || *index < 0.0
                        || index.fract() != 0.0
                        || (*index as usize) >= Timebase::COUNT
                    {
                        return Err(format!(
                            "timebase-beats index must be an integer from 0 to {}",
                            Timebase::COUNT - 1
                        ));
                    }
                    Timebase::from_index(*index as u32)
                }
                value => parse_timebase_arg(std::slice::from_ref(value), 0)?,
            };
            Ok(EValue::Number(timebase.step_beats(16)))
        },
    );

    // The UI runtime already owns `track` as a widget/SDF combinator. Process
    // bodies execute in the scheduler scratch VM, where the name is free; do
    // not replace an established host meaning while registering authoring
    // natives in the UI VM.
    if runtime.global_value("bars").is_none() {
        runtime.register_native_with_docs(
            "bars",
            "(bars n)",
            "Convert a 4/4 bar count to quarter-note beats.",
            move |args, _ctx| {
                if args.len() != 1 {
                    return Err("bars expects one number".to_string());
                }
                let bars = process_number_arg(args.first(), "bars")?;
                if !bars.is_finite() || bars < 0.0 {
                    return Err("bars expects a finite non-negative number".to_string());
                }
                Ok(EValue::Number(bars * 4.0))
            },
        );
    }
    if runtime.global_value("track").is_none() {
        runtime.register_native_with_docs(
            "track",
            "(track index :param [:steps-ago n | :trigs-ago n])",
            "Construct a previous-tick resolved track-parameter read source.",
            move |args, _ctx| {
                if args.len() != 2 && args.len() != 4 {
                    return Err(
                        "track expects index, param, and optional :steps-ago/:trigs-ago count"
                            .to_string(),
                    );
                }
                let track = process_number_arg(args.first(), "track")?;
                if !track.is_finite() || track < 0.0 || track.fract() != 0.0 {
                    return Err("track index must be a non-negative integer".to_string());
                }
                let param = process_symbol_name(&args[1])?;
                if param == "fire-count" {
                    if args.len() != 4 || process_symbol_name(&args[2])? != "window" {
                        return Err(
                            "track :fire-count expects :window and a beat duration".to_string()
                        );
                    }
                    let window = process_number_arg(args.get(3), "track")?;
                    if !window.is_finite() || window < 0.0 {
                        return Err(
                            "track :fire-count window must be finite and non-negative".to_string()
                        );
                    }
                    return Ok(process_map([
                        ("kind", EValue::Keyword("track-fire-count".to_string())),
                        ("track", EValue::Number(track)),
                        ("window", EValue::Number(window)),
                    ]));
                }
                let mut fields = vec![
                    ("kind", EValue::Keyword("track-read".to_string())),
                    ("track", EValue::Number(track)),
                    ("param", EValue::Keyword(param)),
                ];
                if args.len() == 4 {
                    let mode = process_symbol_name(&args[2])?;
                    if mode != "steps-ago" && mode != "trigs-ago" {
                        return Err(
                            "track read history mode must be :steps-ago or :trigs-ago".to_string()
                        );
                    }
                    let ago = process_number_arg(args.get(3), "track")?;
                    if !ago.is_finite() || ago < 0.0 || ago.fract() != 0.0 {
                        return Err(
                            "track read history count must be a non-negative integer".to_string()
                        );
                    }
                    if ago as usize >= crate::process::PROCESS_READ_HISTORY_DEPTH {
                        return Err(format!(
                            "track read history count must be less than {}",
                            crate::process::PROCESS_READ_HISTORY_DEPTH
                        ));
                    }
                    fields.push(("mode", EValue::Keyword(mode)));
                    fields.push(("ago", EValue::Number(ago)));
                }
                Ok(process_map(fields))
            },
        );
    }

    runtime.register_native_with_docs(
        "pitch-field",
        "(pitch-field pitches [:root pitch] [:weight 0..1])",
        "Construct a typed pitch-set suggestion field.",
        move |args, _ctx| {
            let Some(EValue::List(pitches)) = args.first() else {
                return Err("pitch-field expects a non-empty pitch list".to_string());
            };
            if pitches.is_empty() {
                return Err("pitch-field expects a non-empty pitch list".to_string());
            }
            let mut pitch_values = Vec::with_capacity(pitches.len());
            for pitch in pitches {
                let pitch = pitch.borrow();
                let pitch = process_number_arg(Some(&pitch), "pitch-field")?;
                if !pitch.is_finite() {
                    return Err("pitch-field pitches must be finite".to_string());
                }
                pitch_values.push(EValue::Number(pitch));
            }
            if (args.len() - 1) % 2 != 0 {
                return Err("pitch-field options must be keyword/value pairs".to_string());
            }
            let mut root = EValue::Nil;
            let mut weight = 1.0;
            let mut index = 1;
            while index < args.len() {
                match process_symbol_name(&args[index])?.as_str() {
                    "root" => {
                        let value = process_number_arg(args.get(index + 1), "pitch-field :root")?;
                        if !value.is_finite() {
                            return Err("pitch-field root must be finite".to_string());
                        }
                        root = EValue::Number(value);
                    }
                    "weight" => {
                        weight = process_number_arg(args.get(index + 1), "pitch-field :weight")?;
                        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
                            return Err("pitch-field weight must be between 0 and 1".to_string());
                        }
                    }
                    option => return Err(format!("pitch-field unknown option :{option}")),
                }
                index += 2;
            }
            Ok(process_map([
                ("field-domain", EValue::Keyword("pitch-field".to_string())),
                ("pitches", process_list(pitch_values)),
                ("root", root),
                ("weight", EValue::Number(weight)),
            ]))
        },
    );

    runtime.register_native_with_docs(
        "scalar-field",
        "(scalar-field value)",
        "Construct a typed scalar suggestion field.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("scalar-field expects one number".to_string());
            }
            process_scalar_field(process_number_arg(args.first(), "scalar-field")?)
        },
    );

    runtime.register_native_with_docs(
        "gate-field",
        "(gate-field value)",
        "Construct a typed gate suggestion field.",
        move |args, _ctx| match args.as_slice() {
            [EValue::Bool(value)] => Ok(process_gate_field(*value)),
            [EValue::Number(value)] => Ok(process_gate_field(*value > 0.5)),
            _ => Err("gate-field expects one boolean or number".to_string()),
        },
    );

    let process_eval_for_suggest = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "suggest",
        "(suggest :field value)",
        "Publish a typed field into the named channel at this process tick.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("suggest expects a field name and value".to_string());
            }
            let name = process_symbol_name(&args[0])?;
            let value = normalize_process_field(&args[1])?;
            let mut guard = process_eval_for_suggest
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("suggest called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "suggest")?;
            ctx.outputs.push(crate::process::ProcessOutput {
                name: format!("__field:{name}"),
                value: value.clone(),
            });
            Ok(value)
        },
    );

    let process_eval_for_hear = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "hear",
        "(hear :field)",
        "Read the newest typed field published strictly before this process tick.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("hear expects one field name".to_string());
            }
            let name = process_symbol_name(&args[0])?;
            let guard = process_eval_for_hear
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("hear called outside process execution".to_string());
            };
            Ok(ctx.reads.fields.get(&name).cloned().unwrap_or(EValue::Nil))
        },
    );

    runtime.register_native_with_docs(
        "field-domain",
        "(field-domain field)",
        "Return a typed field's domain keyword, or nil for no field.",
        move |args, _ctx| match args.as_slice() {
            [EValue::Nil] => Ok(EValue::Nil),
            [field] => Ok(EValue::Keyword(process_field_domain(field)?)),
            _ => Err("field-domain expects one field".to_string()),
        },
    );

    for (native, key) in [
        ("field-value", "value"),
        ("field-pitches", "pitches"),
        ("field-root", "root"),
        ("field-weight", "weight"),
    ] {
        runtime.register_native_with_docs(
            native,
            format!("({native} field)"),
            "Read a typed field component.",
            move |args, _ctx| match args.as_slice() {
                [EValue::Nil] => Ok(EValue::Nil),
                [field] => process_field_cell(field, key),
                _ => Err(format!("{native} expects one field")),
            },
        );
    }

    runtime.register_native_with_docs(
        "field-nearest-delta",
        "(field-nearest-delta pitch-field current-pitch grace)",
        "Return the shortest signed pitch-class delta toward a pitch field.",
        move |args, _ctx| {
            if args.len() != 3 || process_field_domain(&args[0])? != "pitch-field" {
                return Err(
                    "field-nearest-delta expects a pitch field, current pitch, and grace"
                        .to_string(),
                );
            }
            let current = process_number_arg(args.get(1), "field-nearest-delta")?;
            let grace = process_number_arg(args.get(2), "field-nearest-delta")?.max(0.0);
            let EValue::List(pitches) = process_field_cell(&args[0], "pitches")? else {
                return Err("pitch field pitches must be a list".to_string());
            };
            let mut best: Option<f64> = None;
            for pitch in pitches {
                let pitch = pitch.borrow();
                let pitch = process_number_arg(Some(&pitch), "field-nearest-delta")?;
                let delta = (pitch - current + 6.0).rem_euclid(12.0) - 6.0;
                if best.is_none_or(|best| delta.abs() < best.abs()) {
                    best = Some(delta);
                }
            }
            let delta = best.ok_or_else(|| "pitch field cannot be empty".to_string())?;
            Ok(EValue::Number(if delta.abs() <= grace {
                0.0
            } else {
                delta
            }))
        },
    );

    let process_eval_for_current_note = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "current-note",
        "(current-note)",
        "Return the current step event's resolved transpose before this process runs.",
        move |args, _ctx| {
            if !args.is_empty() {
                return Err("current-note expects no arguments".to_string());
            }
            let guard = process_eval_for_current_note
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(step) = guard.as_ref().and_then(|ctx| ctx.step_context.as_ref()) else {
                return Err("current-note requires a scheduler step event context".to_string());
            };
            Ok(EValue::Number(step.resolved.transpose as f64))
        },
    );

    for (native, observe) in [("observed-tracks", true), ("play-tracks", false)] {
        let process_eval_for_tracks = Arc::clone(&process_eval);
        runtime.register_native_with_docs(
            native,
            format!("({native})"),
            "Return this conductor attachment's bound track indices.",
            move |args, _ctx| {
                if !args.is_empty() {
                    return Err(format!("{native} expects no arguments"));
                }
                let guard = process_eval_for_tracks
                    .lock()
                    .map_err(|_| "failed to lock process eval context".to_string())?;
                let Some(ctx) = guard.as_ref() else {
                    return Err(format!("{native} called outside process execution"));
                };
                let tracks = if observe {
                    &ctx.conductor_observe_tracks
                } else {
                    &ctx.conductor_play_tracks
                };
                if tracks.is_empty() {
                    return Err(format!("{native} requires a conductor attachment"));
                }
                Ok(process_list(
                    tracks.iter().map(|track| EValue::Number(*track as f64)),
                ))
            },
        );
    }

    let process_authoring_for_read_source = Arc::clone(&process_authoring);
    runtime.register_native_with_docs(
        "process",
        "(process name :state-or-outlet)",
        "Construct a process state/outlet read source.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("process expects a name and state/outlet field".to_string());
            }
            let process = match &args[0] {
                EValue::HostHandle { kind, id, .. } if kind == "process" => {
                    let registry = process_authoring_for_read_source
                        .lock()
                        .map_err(|_| "failed to lock process authoring registry".to_string())?;
                    let instance = registry
                        .instances
                        .iter()
                        .find(|instance| instance.handle_id.0 == *id)
                        .ok_or_else(|| "unknown process handle in read source".to_string())?;
                    instance
                        .name
                        .clone()
                        .unwrap_or_else(|| instance.class_name.clone())
                }
                value => process_symbol_name(value)?,
            };
            Ok(process_map([
                ("kind", EValue::Keyword("process-read".to_string())),
                ("process", EValue::String(process)),
                ("field", EValue::String(process_symbol_name(&args[1])?)),
            ]))
        },
    );

    let process_eval_for_read = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "read",
        "(read (track ...)) | (read (process ...)) | (read :channel :name)",
        "Read scheduler-owned resolved track history, process state/outlets, or a channel.",
        move |args, _ctx| {
            let guard = process_eval_for_read
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("read called outside process execution".to_string());
            };
            if args.len() == 2 && process_symbol_name(&args[0]).ok().as_deref() == Some("channel") {
                let name = process_symbol_name(&args[1])?;
                return Ok(ctx
                    .reads
                    .channels
                    .get(&name)
                    .cloned()
                    .unwrap_or(EValue::Nil));
            }
            let [EValue::Map(source)] = args.as_slice() else {
                return Err("read expects a track/process source or channel name".to_string());
            };
            let string_field = |name: &str| -> Result<String, String> {
                let value = source
                    .get(name)
                    .ok_or_else(|| format!("read source missing {name}"))?
                    .borrow();
                process_symbol_name(&value)
            };
            match string_field("kind")?.as_str() {
                "track-fire-count" => {
                    let track = match source.get("track").map(|value| value.borrow()) {
                        Some(value) => process_number_arg(Some(&value), "read")? as usize,
                        None => return Err("track fire-count source missing track".to_string()),
                    };
                    let window = match source.get("window").map(|value| value.borrow()) {
                        Some(value) => process_number_arg(Some(&value), "read")?,
                        None => return Err("track fire-count source missing window".to_string()),
                    };
                    let count = ctx
                        .reads
                        .tracks
                        .get(track)
                        .map(|track| {
                            let lower = ctx.beat - window;
                            track
                                .trig_beats
                                .iter()
                                .filter(|beat| **beat > lower + 1e-9 && **beat <= ctx.beat + 1e-9)
                                .count()
                        })
                        .unwrap_or(0);
                    Ok(EValue::Number(count as f64))
                }
                "track-read" => {
                    let track = match source.get("track").map(|value| value.borrow()) {
                        Some(value) => process_number_arg(Some(&value), "read")? as usize,
                        None => return Err("track read source missing track".to_string()),
                    };
                    let param =
                        parse_step_param_arg(&[EValue::Keyword(string_field("param")?)], 0)?;
                    let Some(track) = ctx.reads.tracks.get(track) else {
                        return Ok(EValue::Number(param.default_value() as f64));
                    };
                    let mode = source
                        .get("mode")
                        .map(|_| string_field("mode"))
                        .transpose()?;
                    let ago = match source.get("ago").map(|value| value.borrow()) {
                        Some(value) => process_number_arg(Some(&value), "read")? as usize,
                        None => 0,
                    };
                    let values = match mode.as_deref() {
                        Some("steps-ago") => track.steps.get(ago).unwrap_or(&track.current),
                        Some("trigs-ago") => track.trigs.get(ago).unwrap_or(&track.current),
                        None => &track.current,
                        Some(_) => return Err("unknown track read history mode".to_string()),
                    };
                    Ok(EValue::Number(values[param.index()] as f64))
                }
                "process-read" => {
                    let process = string_field("process")?;
                    let field = string_field("field")?;
                    Ok(ctx
                        .reads
                        .process_values
                        .get(&process)
                        .and_then(|values| values.get(&field))
                        .cloned()
                        .unwrap_or(EValue::Nil))
                }
                _ => Err("unknown read source".to_string()),
            }
        },
    );

    let process_eval_for_ratchet = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "ratchet!",
        "(ratchet! :times n :mode :subdivide|:repeat :span beats :shape fn)",
        "Clone the current scheduler-owned base event into a ratchet burst.",
        move |args, _ctx| {
            let (times, mode, span_beats, shape) = parse_process_ratchet_args(&args)?;
            let mut guard = process_eval_for_ratchet
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("ratchet! called outside process execution".to_string());
            };
            ensure_process_run_scope(ctx, "ratchet!")?;
            let Some(step_context) = ctx.step_context.clone() else {
                return Err("ratchet! requires a scheduler step event context".to_string());
            };
            if times == 0 {
                return Ok(EValue::Bool(false));
            }
            ctx.commands
                .push(crate::process::ProcessRunCommand::Ratchet(
                    crate::process::ProcessRatchetRequest {
                        times,
                        mode,
                        span_beats,
                        shape,
                        shape_context: crate::process::ProcessRatchetShapeContext {
                            runtime_id: ctx.runtime_id,
                            beat: ctx.beat,
                            inlets: ctx.inlets.clone(),
                            state: ctx.state.clone(),
                            event: ctx.event.clone(),
                            step_context,
                            ports: ctx.ports.clone(),
                            random_state: ctx.random_state,
                        },
                    },
                ));
            Ok(EValue::Number(times as f64))
        },
    );

    register_process_ratchet_event_natives(runtime);

    let process_eval_for_target_add = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "target-add!",
        "(target-add! value) | (target-add! :port value)",
        "Add a value to one of this process run's typed target ports.",
        move |args, _ctx| {
            let (port, value) = process_target_write_args(&args, "target-add!")?;
            push_process_target_write(
                &process_eval_for_target_add,
                crate::process::ProcessTargetOp::Add,
                port,
                value,
            )?;
            Ok(EValue::Number(value as f64))
        },
    );

    let process_eval_for_target_set = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "target-set!",
        "(target-set! value) | (target-set! :port value)",
        "Set one of this process run's typed target ports.",
        move |args, _ctx| {
            let (port, value) = process_target_write_args(&args, "target-set!")?;
            push_process_target_write(
                &process_eval_for_target_set,
                crate::process::ProcessTargetOp::Set,
                port,
                value,
            )?;
            Ok(EValue::Number(value as f64))
        },
    );

    let process_eval_for_rand = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "rand",
        "(rand)",
        "Deterministic process-scoped pseudo-random float in [0,1).",
        move |_args, _ctx| {
            let mut guard = process_eval_for_rand
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("rand called outside process execution".to_string());
            };
            ctx.random_state = ctx.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let bits = gen_splitmix64(ctx.random_state);
            let unit = ((bits >> 11) as f64) * (1.0 / ((1u64 << 53) as f64));
            Ok(EValue::Number(unit))
        },
    );

    runtime.register_native_with_docs(
        "clip",
        "(clip value low high)",
        "Clamp value to the inclusive numeric range.",
        move |args, _ctx| {
            let value = process_number_arg(args.first(), "clip")?;
            let low = process_number_arg(args.get(1), "clip")?;
            let high = process_number_arg(args.get(2), "clip")?;
            if !value.is_finite() || !low.is_finite() || !high.is_finite() || low > high {
                return Ok(EValue::Number(f64::NAN));
            }
            Ok(EValue::Number(value.clamp(low, high)))
        },
    );

    runtime.register_native_with_docs(
        "wrap",
        "(wrap value low high)",
        "Wrap value into the half-open numeric range [low, high).",
        move |args, _ctx| {
            let value = process_number_arg(args.first(), "wrap")?;
            let low = process_number_arg(args.get(1), "wrap")?;
            let high = process_number_arg(args.get(2), "wrap")?;
            if !value.is_finite() || !low.is_finite() || !high.is_finite() || high <= low {
                return Ok(EValue::Number(f64::NAN));
            }
            let span = high - low;
            let mut wrapped = (value - low) % span;
            if wrapped < 0.0 {
                wrapped += span;
            }
            Ok(EValue::Number(low + wrapped))
        },
    );

    runtime.register_native_with_docs(
        "bounce",
        "(bounce value low high)",
        "Fold value into a ping-pong numeric range.",
        move |args, _ctx| {
            let value = process_number_arg(args.first(), "bounce")?;
            let low = process_number_arg(args.get(1), "bounce")?;
            let high = process_number_arg(args.get(2), "bounce")?;
            if !value.is_finite() || !low.is_finite() || !high.is_finite() || high <= low {
                return Ok(EValue::Number(f64::NAN));
            }
            let span = high - low;
            let period = span * 2.0;
            let mut phase = (value - low) % period;
            if phase < 0.0 {
                phase += period;
            }
            let folded = if phase <= span {
                low + phase
            } else {
                high - (phase - span)
            };
            Ok(EValue::Number(folded))
        },
    );

    runtime.register_native_with_docs(
        "gate?",
        "(gate? value)",
        "Return true when a gate-like value is active.",
        move |args, _ctx| {
            Ok(EValue::Bool(match args.first() {
                Some(EValue::Bool(value)) => *value,
                Some(EValue::Number(value)) => *value > 0.5,
                Some(EValue::Nil) | None => false,
                _ => true,
            }))
        },
    );

    let process_authoring_for_ps = Arc::clone(&process_authoring);
    runtime.register_native_with_docs(
        "ps",
        "(ps)",
        "Return authored process/channel status.",
        move |_args, _ctx| {
            let registry = process_authoring_for_ps
                .lock()
                .map_err(|_| "failed to lock process registry".to_string())?;
            Ok(process_status_value(&registry))
        },
    );
}

fn register_graph_homeostat_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    process_eval: SharedProcessEvalContext,
) {
    let state_for_nudge_param = Arc::clone(&state);
    let eval_for_nudge_param = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "graph-nudge-param!",
        "(graph-nudge-param! graph node :param delta)",
        "Add an ephemeral process delta to a graph node parameter.",
        move |args, _ctx| {
            if args.len() != 4 {
                return Err("graph-nudge-param! expects graph, node, param, and delta".to_string());
            }
            let manifest = homeostat_graph_manifest(&state_for_nudge_param, &args[0])?;
            let node = process_nonnegative_index(&args[1], "graph node")?;
            let param = process_symbol_name(&args[2])?;
            homeostat_validate_node_param(&state_for_nudge_param, &manifest, node, &param)?;
            let amount = finite_graph_delta(&args[3])?;
            dispatch_graph_command(
                &state_for_nudge_param,
                &eval_for_nudge_param,
                crate::graph::GraphControlCommand::Nudge(crate::graph::GraphNudge {
                    graph_id: manifest.id,
                    graph_name: manifest.name,
                    key: crate::graph::GraphDeltaKey::NodeParam { node, param },
                    amount,
                }),
                true,
            )?;
            Ok(EValue::Bool(true))
        },
    );

    let state_for_nudge_node = Arc::clone(&state);
    let eval_for_nudge_node = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "graph-nudge-node!",
        "(graph-nudge-node! graph node :delay delta)",
        "Add an ephemeral process delta to a graph node intrinsic.",
        move |args, _ctx| {
            if args.len() != 4 || process_symbol_name(&args[2])? != "delay" {
                return Err("graph-nudge-node! supports graph, node, :delay, and delta".to_string());
            }
            let manifest = homeostat_graph_manifest(&state_for_nudge_node, &args[0])?;
            let node = process_nonnegative_index(&args[1], "graph node")?;
            homeostat_validate_node(&state_for_nudge_node, &manifest, node)?;
            let amount = finite_graph_delta(&args[3])?;
            dispatch_graph_command(
                &state_for_nudge_node,
                &eval_for_nudge_node,
                crate::graph::GraphControlCommand::Nudge(crate::graph::GraphNudge {
                    graph_id: manifest.id,
                    graph_name: manifest.name,
                    key: crate::graph::GraphDeltaKey::NodeDelay { node },
                    amount,
                }),
                true,
            )?;
            Ok(EValue::Bool(true))
        },
    );

    let state_for_nudge_edge = Arc::clone(&state);
    let eval_for_nudge_edge = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "graph-nudge-edge!",
        "(graph-nudge-edge! graph :from n :to n :weight delta)",
        "Add an ephemeral process delta to a graph edge parameter.",
        move |args, _ctx| {
            let manifest = homeostat_graph_manifest(
                &state_for_nudge_edge,
                args.first().ok_or("graph-nudge-edge! expects a graph")?,
            )?;
            let (from, to, param, amount) = parse_homeostat_edge_args(&args[1..], true)?;
            if param != "weight" && param != "dampening" {
                return Err("graph-nudge-edge! supports :weight or :dampening".to_string());
            }
            homeostat_validate_edge(&state_for_nudge_edge, &manifest, from, to, &param)?;
            dispatch_graph_command(
                &state_for_nudge_edge,
                &eval_for_nudge_edge,
                crate::graph::GraphControlCommand::Nudge(crate::graph::GraphNudge {
                    graph_id: manifest.id,
                    graph_name: manifest.name,
                    key: crate::graph::GraphDeltaKey::EdgeParam { from, to, param },
                    amount,
                }),
                true,
            )?;
            Ok(EValue::Bool(true))
        },
    );

    let state_for_delta = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-delta",
        "(graph-delta graph node :param)",
        "Read the current ephemeral graph node-parameter delta.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err("graph-delta expects graph, node, and param".to_string());
            }
            let manifest = homeostat_graph_manifest(&state_for_delta, &args[0])?;
            let node = process_nonnegative_index(&args[1], "graph node")?;
            let param = process_symbol_name(&args[2])?;
            homeostat_validate_node_param(&state_for_delta, &manifest, node, &param)?;
            Ok(EValue::Number(
                published_graph_delta(
                    &state_for_delta,
                    manifest.id,
                    &crate::graph::GraphDeltaKey::NodeParam { node, param },
                ) as f64,
            ))
        },
    );

    let state_for_delta_edge = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-delta-edge",
        "(graph-delta-edge graph :from n :to n :weight)",
        "Read the current ephemeral graph edge-parameter delta.",
        move |args, _ctx| {
            let manifest = homeostat_graph_manifest(
                &state_for_delta_edge,
                args.first().ok_or("graph-delta-edge expects a graph")?,
            )?;
            let (from, to, param, _) = parse_homeostat_edge_args(&args[1..], false)?;
            homeostat_validate_edge(&state_for_delta_edge, &manifest, from, to, &param)?;
            Ok(EValue::Number(
                published_graph_delta(
                    &state_for_delta_edge,
                    manifest.id,
                    &crate::graph::GraphDeltaKey::EdgeParam { from, to, param },
                ) as f64,
            ))
        },
    );

    let state_for_effective = Arc::clone(&state);
    runtime.register_native_with_docs(
        "graph-effective-param",
        "(graph-effective-param graph node :param)",
        "Read a graph node parameter after authored overrides and ephemeral delta.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err(
                    "graph-effective-param expects graph, node, and param".to_string()
                );
            }
            let manifest = homeostat_graph_manifest(&state_for_effective, &args[0])?;
            let node = process_nonnegative_index(&args[1], "graph node")?;
            let param = process_symbol_name(&args[2])?;
            homeostat_validate_node_param(&state_for_effective, &manifest, node, &param)?;
            let config = homeostat_authored_config(&state_for_effective, &manifest);
            let authored = config.node_params[node].get(&param).copied().unwrap_or(0.0);
            let range = config
                .node_param_ranges
                .get(&param)
                .copied()
                .ok_or_else(|| format!("graph node param :{param} has no declared range"))?;
            let delta = published_graph_delta(
                &state_for_effective,
                manifest.id,
                &crate::graph::GraphDeltaKey::NodeParam { node, param },
            );
            Ok(EValue::Number(crate::graph::effective_delta_value(
                authored,
                delta as f64,
                range,
            )))
        },
    );

    let state_for_clear = Arc::clone(&state);
    let eval_for_clear = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "graph-clear-deltas!",
        "(graph-clear-deltas! graph)",
        "Clear all ephemeral deltas for a graph.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("graph-clear-deltas! expects one graph".to_string());
            }
            let manifest = homeostat_graph_manifest(&state_for_clear, &args[0])?;
            dispatch_graph_command(
                &state_for_clear,
                &eval_for_clear,
                crate::graph::GraphControlCommand::Clear {
                    graph_id: manifest.id,
                    graph_name: manifest.name,
                },
                true,
            )?;
            Ok(EValue::Bool(true))
        },
    );

    let state_for_leak = Arc::clone(&state);
    let eval_for_leak = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "graph-delta-leak!",
        "(graph-delta-leak! graph factor)",
        "Set the graph delta's per-beat multiplicative leak factor.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err("graph-delta-leak! expects graph and factor".to_string());
            }
            ensure_outside_process_run(&eval_for_leak, "graph-delta-leak!")?;
            let manifest = homeostat_graph_manifest(&state_for_leak, &args[0])?;
            let factor = finite_graph_delta(&args[1])?;
            if !(0.0..=1.0).contains(&factor) {
                return Err("graph delta leak factor must be between 0 and 1".to_string());
            }
            state_for_leak.push_graph_control_command(crate::graph::GraphControlCommand::SetLeak {
                graph_id: manifest.id,
                graph_name: manifest.name,
                factor,
            });
            Ok(EValue::Bool(true))
        },
    );

    let state_for_commit = Arc::clone(&state);
    let eval_for_commit = Arc::clone(&process_eval);
    runtime.register_native_with_docs(
        "graph-commit-deltas!",
        "(graph-commit-deltas! graph)",
        "Fold current ephemeral deltas into authored overrides and clear them.",
        move |args, _ctx| {
            if args.len() != 1 {
                return Err("graph-commit-deltas! expects one graph".to_string());
            }
            ensure_outside_process_run(&eval_for_commit, "graph-commit-deltas!")?;
            let manifest = homeostat_graph_manifest(&state_for_commit, &args[0])?;
            commit_published_graph_deltas(&state_for_commit, &manifest)?;
            state_for_commit.push_graph_control_command(crate::graph::GraphControlCommand::Clear {
                graph_id: manifest.id,
                graph_name: manifest.name,
            });
            Ok(EValue::Bool(true))
        },
    );
}

fn process_nonnegative_index(value: &EValue, label: &str) -> Result<usize, String> {
    let value = process_number_arg(Some(value), label)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err(format!("{label} must be a non-negative integer"));
    }
    Ok(value as usize)
}

fn finite_graph_delta(value: &EValue) -> Result<f32, String> {
    let value = process_number_arg(Some(value), "graph delta")?;
    if !value.is_finite() || value < f32::MIN as f64 || value > f32::MAX as f64 {
        return Err("graph delta must be a finite f32 value".to_string());
    }
    Ok(value as f32)
}

fn homeostat_graph_manifest(
    state: &crate::sequencer::SequencerState,
    reference: &EValue,
) -> Result<crate::graph::GraphManifest, String> {
    let id = match reference {
        EValue::Number(value) if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 => {
            Some(*value as u64)
        }
        _ => None,
    };
    let name = process_symbol_name(reference).ok();
    state
        .published_sequencers()
        .into_iter()
        .filter_map(|published| published.graph)
        .find(|manifest| id == Some(manifest.id) || name.as_deref() == Some(&manifest.name))
        .ok_or_else(|| "graph not found".to_string())
}

fn homeostat_authored_config(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> crate::graph::GraphRuntimeConfig {
    let overrides = state
        .current_graph_overrides()
        .into_iter()
        .find(|overrides| {
            overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
        });
    manifest.runtime_config_with_overrides(overrides.as_ref())
}

fn homeostat_validate_node(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    node: usize,
) -> Result<(), String> {
    let overrides = state
        .current_graph_overrides()
        .into_iter()
        .find(|overrides| {
            overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
        });
    if node >= manifest.shape.resolved_node_count(overrides.as_ref()) {
        return Err("graph node index out of range".to_string());
    }
    Ok(())
}

fn homeostat_validate_node_param(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    node: usize,
    param: &str,
) -> Result<(), String> {
    homeostat_validate_node(state, manifest, node)?;
    if !manifest.node.params.iter().any(|spec| spec.name == param) {
        return Err(format!("graph node param :{param} has no declared range"));
    }
    Ok(())
}

fn homeostat_validate_edge(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
    from: usize,
    to: usize,
    param: &str,
) -> Result<(), String> {
    homeostat_validate_node(state, manifest, from)?;
    homeostat_validate_node(state, manifest, to)?;
    if !manifest
        .edge_sets
        .iter()
        .flat_map(|edge_set| &edge_set.params)
        .any(|spec| spec.name == param)
    {
        return Err(format!("graph edge param :{param} has no declared range"));
    }
    Ok(())
}

fn parse_homeostat_edge_args(
    args: &[EValue],
    with_amount: bool,
) -> Result<(usize, usize, String, f32), String> {
    let mut from = None;
    let mut to = None;
    let mut param = None;
    let mut amount = 0.0;
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?;
        idx += 1;
        if key == "from" || key == "to" {
            let value = args
                .get(idx)
                .ok_or_else(|| format!("graph edge :{key} expects an index"))?;
            let value = process_nonnegative_index(value, &key)?;
            if key == "from" {
                from = Some(value);
            } else {
                to = Some(value);
            }
            idx += 1;
            continue;
        }
        param = Some(key);
        if with_amount {
            amount = finite_graph_delta(
                args.get(idx)
                    .ok_or("graph edge nudge parameter expects a delta")?,
            )?;
            idx += 1;
        }
        if idx != args.len() {
            return Err("graph edge expects :from, :to, and one parameter".to_string());
        }
    }
    Ok((
        from.ok_or("graph edge requires :from")?,
        to.ok_or("graph edge requires :to")?,
        param.ok_or("graph edge requires a parameter")?,
        amount,
    ))
}

fn dispatch_graph_command(
    state: &crate::sequencer::SequencerState,
    process_eval: &SharedProcessEvalContext,
    command: crate::graph::GraphControlCommand,
    allow_in_process: bool,
) -> Result<(), String> {
    let mut guard = process_eval
        .lock()
        .map_err(|_| "failed to lock process eval context".to_string())?;
    if let Some(ctx) = guard.as_mut() {
        if ctx.scope == ProcessEvalScope::Run {
            if !allow_in_process {
                return Err("graph action is not allowed from a process run".to_string());
            }
            ctx.commands
                .push(crate::process::ProcessRunCommand::Graph(command));
            return Ok(());
        }
    }
    drop(guard);
    state.push_graph_control_command(command);
    Ok(())
}

fn ensure_outside_process_run(
    process_eval: &SharedProcessEvalContext,
    native: &str,
) -> Result<(), String> {
    let guard = process_eval
        .lock()
        .map_err(|_| "failed to lock process eval context".to_string())?;
    if guard
        .as_ref()
        .is_some_and(|ctx| ctx.scope == ProcessEvalScope::Run)
    {
        return Err(format!("{native} is not callable from a process run"));
    }
    Ok(())
}

fn published_graph_delta(
    state: &crate::sequencer::SequencerState,
    graph_id: u64,
    key: &crate::graph::GraphDeltaKey,
) -> f32 {
    state
        .graph_visualizations()
        .into_iter()
        .find(|snapshot| snapshot.id == graph_id)
        .and_then(|snapshot| {
            snapshot
                .deltas
                .into_iter()
                .find(|entry| &entry.key == key)
                .map(|entry| entry.delta)
        })
        .unwrap_or(0.0)
}

fn commit_published_graph_deltas(
    state: &crate::sequencer::SequencerState,
    manifest: &crate::graph::GraphManifest,
) -> Result<(), String> {
    let deltas = state
        .graph_visualizations()
        .into_iter()
        .find(|snapshot| snapshot.id == manifest.id)
        .map(|snapshot| snapshot.deltas)
        .unwrap_or_default();
    if deltas.is_empty() {
        return Ok(());
    }
    let authored = homeostat_authored_config(state, manifest);
    let edge_group = manifest
        .edge_sets
        .first()
        .map(crate::graph::edge_set_group_id)
        .unwrap_or_default();
    state.edit_current_graph_overrides(|graphs| {
        let graph = if let Some(index) = graphs.iter().position(|graph| {
            graph.sequencer_id == manifest.id || graph.sequencer_name == manifest.name
        }) {
            &mut graphs[index]
        } else {
            graphs.push(crate::graph::ProjectGraphOverrides {
                sequencer_id: manifest.id,
                sequencer_name: manifest.name.clone(),
                ..crate::graph::ProjectGraphOverrides::default()
            });
            graphs.last_mut().expect("graph override inserted")
        };
        for entry in &deltas {
            match &entry.key {
                crate::graph::GraphDeltaKey::NodeDelay { node } => {
                    let range = crate::graph::GraphDeltaRange {
                        min: crate::graph::GRAPH_NODE_DELAY_MIN,
                        max: crate::graph::GRAPH_NODE_DELAY_MAX,
                        is_int: true,
                    };
                    let value = crate::graph::effective_delta_value(
                        authored.nodes[*node].delay_steps as f64,
                        entry.delta as f64,
                        range,
                    ) as u32;
                    let intrinsic = graph
                        .node_intrinsics
                        .iter_mut()
                        .find(|item| {
                            item.group == manifest.node.name && item.instance == *node
                        });
                    if let Some(intrinsic) = intrinsic {
                        intrinsic.delay_steps = Some(value);
                    } else {
                        graph.node_intrinsics.push(
                            crate::graph::ProjectGraphNodeIntrinsicOverride {
                                group: manifest.node.name.clone(),
                                instance: *node,
                                resolution: None,
                                delay_steps: Some(value),
                                quantize: None,
                                route: None,
                                seed_from: None,
                                seed_on_reset: None,
                                duration: None,
                                swing: None,
                                neural_group: None,
                            },
                        );
                    }
                }
                crate::graph::GraphDeltaKey::NodeParam { node, param } => {
                    let range = authored.node_param_ranges[param];
                    let value = crate::graph::effective_delta_value(
                        authored.node_params[*node].get(param).copied().unwrap_or(0.0),
                        entry.delta as f64,
                        range,
                    );
                    if let Some(item) = graph.node_params.iter_mut().find(|item| {
                        item.group == manifest.node.name
                            && item.instance == *node
                            && item.param == *param
                    }) {
                        item.value = value;
                    } else {
                        graph.node_params.push(crate::graph::ProjectGraphNodeParamOverride {
                            group: manifest.node.name.clone(),
                            instance: *node,
                            param: param.clone(),
                            value,
                        });
                    }
                }
                crate::graph::GraphDeltaKey::EdgeParam { from, to, param } => {
                    let range = authored.edge_param_ranges[param];
                    let edge = authored
                        .edges
                        .iter()
                        .find(|edge| edge.from == *from && edge.to == *to)
                        .ok_or("graph delta edge disappeared before commit")?;
                    let base = if param == "weight" {
                        edge.weight
                    } else {
                        edge.dampening
                    };
                    let value =
                        crate::graph::effective_delta_value(base, entry.delta as f64, range);
                    if let Some(item) = graph.edge_params.iter_mut().find(|item| {
                        item.group == edge_group
                            && item.from == *from
                            && item.to == *to
                            && item.param == *param
                    }) {
                        item.value = value;
                    } else {
                        graph.edge_params.push(crate::graph::ProjectGraphEdgeParamOverride {
                            group: edge_group.clone(),
                            from: *from,
                            to: *to,
                            param: param.clone(),
                            value,
                        });
                    }
                }
            }
        }
        Ok(())
    })
}

pub(in crate::lisp_host) fn register_def_accumulator_dispatch_native(
    runtime: &mut Runtime,
    accumulators: SharedRegisteredAccumulators,
    process_authoring: SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) {
    // A vm native (not a plain native) so the process-accumulator branch can
    // register the class constructor, matching def-process.
    runtime.register_vm_native_with_docs(
        "def-accumulator",
        "(def-accumulator name body) | (def-accumulator name :target (step-param :transpose) :amount (...))",
        "Define either a legacy script accumulator or a process accumulator, depending on the argument shape.",
        move |args, vm| {
            let result = def_accumulator_dispatch(
                args,
                vm,
                &accumulators,
                &process_authoring,
                process_chain_state.clone(),
                publish.clone(),
            );
            match result {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("[process] def-accumulator error: {error}");
                    EValue::Bool(false)
                }
            }
        },
    );
}

pub(in crate::lisp_host) fn def_accumulator_dispatch(
    args: Vec<EValue>,
    vm: &mut eseqlisp::vm::VM,
    accumulators: &SharedRegisteredAccumulators,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let is_legacy_script_form = args.len() == 2 && !matches!(args.get(1), Some(EValue::Keyword(_)));
    if !is_legacy_script_form {
        return register_process_accumulator_def(
            args,
            vm,
            process_authoring,
            process_chain_state,
            publish,
        );
    }
    let name = process_symbol_name(
        args.first()
            .ok_or_else(|| "expected accumulator name".to_string())?,
    )?;
    let callback = args
        .get(1)
        .ok_or_else(|| "expected accumulator callback".to_string())?;
    let callback = match callback {
        EValue::Closure(_, _) => RegisteredAccumulatorCallback::Closure(callback.clone()),
        EValue::String(source) => RegisteredAccumulatorCallback::Source(source.clone()),
        other => RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(other)),
    };
    let mut registry = accumulators
        .lock()
        .map_err(|_| "failed to lock accumulator registry".to_string())?;
    if let Some(existing) = registry.iter_mut().find(|entry| entry.name == name) {
        existing.callback = callback.clone();
    } else {
        registry.push(RegisteredAccumulator {
            name: name.clone(),
            callback: callback.clone(),
            params: Vec::new(),
        });
    }
    Ok(EValue::Bool(true))
}

pub(in crate::lisp_host) fn register_process_graph_emit_native(
    runtime: &mut Runtime,
    process_eval: SharedProcessEvalContext,
) {
    runtime.register_native_with_docs(
        "emit",
        "(emit :track n :after beats :note n :vel v :duration d) or (emit :note n :vel v :dur d)",
        "Emit a process event when called from a process body; otherwise build a graph update emit map.",
        move |args, _ctx| {
            let mut guard = process_eval
                .lock()
                .map_err(|_| "failed to lock process eval context".to_string())?;
            if let Some(ctx) = guard.as_mut() {
                ensure_process_run_scope(ctx, "emit")?;
                let event = build_process_emit_event(&args)?;
                if !ctx.conductor_play_tracks.is_empty() {
                    let Some(track) = event.track else {
                        return Err("conductor emit requires an explicit bound :track".to_string());
                    };
                    if !ctx.conductor_play_tracks.contains(&track) {
                        return Err(format!(
                            "conductor cannot emit to unbound track {track}; bound play tracks are {:?}",
                            ctx.conductor_play_tracks
                        ));
                    }
                }
                ctx.emissions.push(event);
                return Ok(EValue::Bool(true));
            }
            drop(guard);
            graph_update::build_graph_emit_value(&args)
        },
    );
}

pub fn register_published_process_authoring_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    ui_epoch: Arc<AtomicUsize>,
) -> PublishedProcessAuthoringNatives {
    let process_authoring = Arc::new(Mutex::new(ProcessAuthoringRegistry::with_handle_base(
        UI_PROCESS_HANDLE_BASE,
    )));
    let process_eval = Arc::new(Mutex::new(None));
    let state_for_publish = Arc::clone(&state);
    let publish: ProcessPublishHook = Arc::new(move |snapshot| {
        state_for_publish.publish_process_authoring(snapshot);
        ui_epoch.fetch_add(1, Ordering::Relaxed);
    });
    register_process_natives(
        runtime,
        Arc::clone(&process_authoring),
        process_eval,
        Some(Arc::clone(&publish)),
        Some(Arc::clone(&state)),
        false,
    );
    register_process_chain_natives(
        runtime,
        Arc::clone(&state),
        Arc::clone(&process_authoring),
        Some(Arc::clone(&publish)),
        true,
    );
    PublishedProcessAuthoringNatives {
        process_authoring,
        process_chain_state: state,
        publish: Some(publish),
    }
}

pub(in crate::lisp_host) const PROCESS_LANE_TAG: &str = "__process-lane";
pub(in crate::lisp_host) const PROCESS_INLET_INSTANCE_TARGET_TAG: &str = "__process-inlet-instance";

/// `(lane 0 1 0 ...)` evaluates to a tagged list; `processes`/`lane!` unpack it.
pub(in crate::lisp_host) fn process_lane_values(value: &EValue) -> Option<Result<Vec<f32>, String>> {
    let EValue::List(items) = value else {
        return None;
    };
    match items.first().map(|item| item.borrow().clone()) {
        Some(EValue::Keyword(tag)) if tag.trim_start_matches(':') == PROCESS_LANE_TAG => {}
        _ => return None,
    }
    let mut values = Vec::with_capacity(items.len().saturating_sub(1));
    for item in &items[1..] {
        match &*item.borrow() {
            EValue::Number(number) => values.push(*number as f32),
            EValue::Bool(gate) => values.push(if *gate { 1.0 } else { 0.0 }),
            other => {
                return Some(Err(format!(
                    "lane values must be numbers, got {}",
                    eseqlisp::vm::format_lisp_value(other)
                )));
            }
        }
    }
    Some(Ok(values))
}

pub(in crate::lisp_host) fn process_lane_literal(values: &[f32]) -> EValue {
    let mut items = vec![EValue::Keyword(PROCESS_LANE_TAG.to_string())];
    items.extend(values.iter().map(|value| EValue::Number(*value as f64)));
    process_list(items)
}

pub(in crate::lisp_host) fn parse_process_track_spec(value: &EValue, active_tracks: usize) -> Result<Vec<usize>, String> {
    let parse_index = |number: f64| -> Result<usize, String> {
        if number < 0.0 || number.fract() != 0.0 {
            return Err("processes expects non-negative integer track indices".to_string());
        }
        let track = number as usize;
        if track >= active_tracks {
            return Err(format!("track {track} out of range"));
        }
        Ok(track)
    };
    match value {
        EValue::Number(number) => Ok(vec![parse_index(*number)?]),
        EValue::Keyword(name) | EValue::Symbol(name) if name.trim_start_matches(':') == "all" => {
            Err(
                "(processes :track :all) is deprecated: use (processes :project ...) for a \
                 project-wide layer every track (present and future) runs, or (list 0 1 ...) \
                 to stamp independent copies on a track set"
                    .to_string(),
            )
        }
        EValue::List(items) => {
            let mut tracks = Vec::with_capacity(items.len());
            for item in items {
                match &*item.borrow() {
                    EValue::Number(number) => tracks.push(parse_index(*number)?),
                    _ => return Err("processes :track list expects track indices".to_string()),
                }
            }
            Ok(tracks)
        }
        _ => Err("processes expects :track <index | (list ...)>".to_string()),
    }
}

/// Convert an attached instance handle into a pattern-scoped chain slot:
/// scalar inlet literals into `slot.inlets`, `(lane ...)` literals into
/// `slot.lanes` (legal only on `:lane true` inlets).
pub(in crate::lisp_host) fn process_chain_slot_from_handle(
    registry: &ProcessAuthoringRegistry,
    handle_id: u64,
) -> Result<crate::process::TrackProcessSlot, String> {
    let instance = registry
        .instances
        .iter()
        .find(|entry| entry.handle_id.0 == handle_id)
        .ok_or_else(|| "unknown process handle".to_string())?;
    let def = registry
        .defs
        .iter()
        .find(|def| def.name == instance.class_name)
        .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
    let mut inlets = std::collections::BTreeMap::new();
    let mut lanes = std::collections::BTreeMap::new();
    for (name, value) in &instance.inlets {
        let crate::process::ProcessInletValue::Literal(literal) = value else {
            return Err(format!(
                "chain-attached process inlet '{name}' must be a literal (patch outlets/channels with `patch` instead)"
            ));
        };
        if let Some(lane_values) = process_lane_values(literal) {
            let lane_backed = def
                .inlets
                .iter()
                .any(|inlet| inlet.name == *name && inlet.lane);
            if !lane_backed {
                return Err(format!(
                    "inlet '{name}' of '{}' is not lane-backed (:lane true)",
                    instance.class_name
                ));
            }
            lanes.insert(
                name.clone(),
                crate::process::ProcessLane {
                    values: lane_values?,
                },
            );
        } else {
            inlets.insert(
                name.clone(),
                crate::process::ProcessLiteral::from_value(literal)?,
            );
        }
    }
    validate_process_port_bindings(def, &instance.bindings)?;
    let mut bindings: BTreeMap<String, Option<crate::process::ParamTarget>> = def
        .ports
        .iter()
        .map(|port| (port.name.clone(), None))
        .collect();
    for (port, target) in &instance.bindings {
        bindings.insert(port.clone(), target.clone());
    }
    Ok(crate::process::TrackProcessSlot {
        instance_id: crate::process::ProcessInstanceId(handle_id),
        instance_name: instance.name.clone(),
        class_name: instance.class_name.clone(),
        enabled: true,
        project_layer: false,
        inlets,
        lanes,
        bindings,
    })
}

pub(in crate::lisp_host) fn validate_process_port_bindings(
    def: &crate::process::ProcessDef,
    bindings: &BTreeMap<String, Option<crate::process::ParamTarget>>,
) -> Result<(), String> {
    for (port_name, target) in bindings {
        let Some(port) = def.ports.iter().find(|port| port.name == *port_name) else {
            return Err(format!(
                "process '{}' has no target port '{}'",
                def.name, port_name
            ));
        };
        if let Some(target) = target {
            if !port.allows_binding_target(target) {
                return Err(format!(
                    "target {} is incompatible with process '{}' port '{}'",
                    process_param_target_label_for_error(target),
                    def.name,
                    port_name
                ));
            }
        }
    }
    Ok(())
}

pub(in crate::lisp_host) fn process_param_target_label_for_error(target: &crate::process::ParamTarget) -> String {
    match target {
        crate::process::ParamTarget::StepParam { param } => format!("step-param:{param}"),
        crate::process::ParamTarget::InstrumentParam { param, .. } => {
            format!("instrument:{param}")
        }
        crate::process::ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => format!("effect{}:{effect}:{param}", slot + 1),
        crate::process::ParamTarget::MidiFxParam { slot, fx, param } => {
            format!("midi-fx{}:{fx}:{param}", slot + 1)
        }
        crate::process::ParamTarget::ProcessInlet {
            process,
            inlet,
            instance_id,
        } => instance_id
            .map(|id| format!("process-inlet:{process}#{}:{inlet}", id.0))
            .unwrap_or_else(|| format!("process-inlet:{process}:{inlet}")),
        crate::process::ParamTarget::RackSlotParam { slot, param } => {
            format!("rack{}:{param}", slot + 1)
        }
        crate::process::ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            format!("rack{}:instrument:{param}", slot + 1)
        }
        crate::process::ParamTarget::RackMacroParam { macro_id } => {
            format!("rack-macro:{}", macro_id + 1)
        }
    }
}

pub(in crate::lisp_host) fn matching_existing_process_slot<'a>(
    slot: &crate::process::TrackProcessSlot,
    existing: &'a crate::process::TrackProcessChain,
) -> Option<&'a crate::process::TrackProcessSlot> {
    existing
        .slots
        .iter()
        .find(|existing_slot| process_slots_have_same_identity(slot, existing_slot))
}

pub(in crate::lisp_host) fn process_slots_have_same_identity(
    left: &crate::process::TrackProcessSlot,
    right: &crate::process::TrackProcessSlot,
) -> bool {
    if let Some(name) = left.instance_name.as_deref() {
        return right.class_name == left.class_name && right.instance_name.as_deref() == Some(name);
    }
    right.instance_id == left.instance_id && right.class_name == left.class_name
}

pub(in crate::lisp_host) fn preserve_process_slot_state(
    defs: &[crate::process::ProcessDef],
    existing: &crate::process::TrackProcessChain,
    replacement: &mut crate::process::TrackProcessChain,
) {
    // Once a chain exists, its order is pattern-owned UI state. Scratch
    // re-evaluation reconciles the attachment set without undoing drag reorder:
    // retained instances keep their existing relative order and newly authored
    // instances are appended in declaration order. An explicitly empty
    // `processes` form still clears the chain.
    let mut pending = std::mem::take(&mut replacement.slots);
    let mut ordered = Vec::with_capacity(pending.len());
    for existing_slot in &existing.slots {
        if let Some(index) = pending
            .iter()
            .position(|slot| process_slots_have_same_identity(slot, existing_slot))
        {
            ordered.push(pending.remove(index));
        }
    }
    ordered.extend(pending);
    replacement.slots = ordered;

    for slot in &mut replacement.slots {
        let Some(existing_slot) = matching_existing_process_slot(slot, existing) else {
            continue;
        };
        slot.enabled = existing_slot.enabled;
        let Some(def) = defs.iter().find(|def| def.name == slot.class_name) else {
            continue;
        };
        for inlet in &def.inlets {
            if inlet.lane {
                if let Some(lane) = existing_slot.lanes.get(&inlet.name) {
                    slot.lanes.insert(inlet.name.clone(), lane.clone());
                }
            }
            if slot.inlets.contains_key(&inlet.name) {
                if let Some(value) = existing_slot.inlets.get(&inlet.name) {
                    slot.inlets.insert(inlet.name.clone(), value.clone());
                }
            } else if inlet.lane {
                if let Some(value) = existing_slot.inlets.get(&inlet.name) {
                    slot.inlets.insert(inlet.name.clone(), value.clone());
                }
            }
        }
        for port in &def.ports {
            let Some(Some(target)) = existing_slot.bindings.get(&port.name) else {
                continue;
            };
            if port.allows_binding_target(target) && slot.bindings.contains_key(&port.name) {
                slot.bindings
                    .insert(port.name.clone(), Some(target.clone()));
            }
        }
    }
}

pub(in crate::lisp_host) fn register_process_chain_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    process_authoring: SharedProcessAuthoring,
    publish: Option<ProcessPublishHook>,
    write_process_chain_state: bool,
) {
    runtime.register_native_with_docs(
        "lane",
        "(lane v1 v2 ...)",
        "Per-step lane literal for a lane-backed process inlet. Steps beyond the lane's length read the inlet default.",
        move |args, _ctx| {
            let mut items = vec![EValue::Keyword(PROCESS_LANE_TAG.to_string())];
            for arg in &args {
                match arg {
                    EValue::Number(_) | EValue::Bool(_) => items.push(arg.clone()),
                    other => {
                        return Err(format!(
                            "lane values must be numbers, got {}",
                            eseqlisp::vm::format_lisp_value(other)
                        ))
                    }
                }
            }
            Ok(process_list(items))
        },
    );

    let state_for_processes = Arc::clone(&state);
    let authoring_for_processes = Arc::clone(&process_authoring);
    let publish_for_processes = publish.clone();
    runtime.register_native_with_docs(
        "processes",
        "(processes :track tracks instance...) | (processes :project instance...) | (processes :observe tracks :play tracks instance)",
        "Declare a track/project step chain or one conductor instance observing and playing track sets.",
        move |args, _ctx| {
            let (EValue::Keyword(key) | EValue::Symbol(key)) = args
                .first()
                .ok_or_else(|| "processes expects :track, :project, or :observe first".to_string())?
            else {
                return Err("processes expects :track, :project, or :observe first".to_string());
            };
            if key.trim_start_matches(':') == "observe" {
                let observe_tracks = parse_process_track_spec(
                    args.get(1)
                        .ok_or_else(|| "processes :observe expects a track list".to_string())?,
                    state_for_processes.active_track_count(),
                )?;
                if observe_tracks.is_empty() {
                    return Err("processes :observe requires at least one track".to_string());
                }
                if observe_tracks.iter().copied().collect::<HashSet<_>>().len()
                    != observe_tracks.len()
                {
                    return Err("processes :observe track list contains duplicates".to_string());
                }
                if process_symbol_name(
                    args.get(2)
                        .ok_or_else(|| "processes :observe expects :play".to_string())?,
                )? != "play"
                {
                    return Err("processes :observe expects :play after observed tracks".to_string());
                }
                let play_tracks = parse_process_track_spec(
                    args.get(3)
                        .ok_or_else(|| "processes :play expects a track list".to_string())?,
                    state_for_processes.active_track_count(),
                )?;
                if play_tracks.is_empty() {
                    return Err("processes :play requires at least one track".to_string());
                }
                if play_tracks.iter().copied().collect::<HashSet<_>>().len() != play_tracks.len() {
                    return Err("processes :play track list contains duplicates".to_string());
                }
                let [EValue::HostHandle { kind, id, .. }] = args.get(4..).unwrap_or(&[]) else {
                    return Err(
                        "conductor attachment expects exactly one process instance".to_string()
                    );
                };
                if kind != "process" {
                    return Err("conductor attachment expects a process instance".to_string());
                }
                {
                    let mut registry = authoring_for_processes
                        .lock()
                        .map_err(|_| "failed to lock process registry".to_string())?;
                    if !registry.instances.iter().any(|instance| instance.handle_id.0 == *id) {
                        return Err("unknown conductor process handle".to_string());
                    }
                    registry
                        .conductors
                        .retain(|entry| entry.process_handle_id.0 != *id);
                    registry
                        .conductors
                        .push(crate::process::AuthoredConductorAttachment {
                            process_handle_id: crate::process::AuthoredHandleId(*id),
                            observe_tracks,
                            play_tracks,
                        });
                }
                publish_process_authoring(&authoring_for_processes, &publish_for_processes);
                return Ok(args[4].clone());
            }
            let (tracks, instance_args) = match key.trim_start_matches(':') {
                "track" => {
                    let tracks = parse_process_track_spec(
                        args.get(1).ok_or_else(|| {
                            "processes expects a track spec after :track".to_string()
                        })?,
                        state_for_processes.active_track_count(),
                    )?;
                    (Some(tracks), args.get(2..).unwrap_or(&[]))
                }
                "project" => (None, args.get(1..).unwrap_or(&[])),
                _ => {
                    return Err(
                        "processes expects :track, :project, or :observe first".to_string()
                    )
                }
            };
            let project_layer = tracks.is_none();
            let mut slots = Vec::new();
            let mut handles = Vec::new();
            let defs = {
                let registry = authoring_for_processes
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                for arg in instance_args {
                    let EValue::HostHandle { kind, id, .. } = arg else {
                        return Err(
                            "processes expects process instances, e.g. (transpose-climb :limit 12)"
                                .to_string(),
                        );
                    };
                    if kind != "process" {
                        return Err(format!("processes expects process instances, got {kind}"));
                    }
                    if slots.iter().any(
                        |slot: &crate::process::TrackProcessSlot| slot.instance_id.0 == *id,
                    ) {
                        return Err(
                            "the same process instance cannot appear twice in one chain; construct a second instance instead"
                                .to_string(),
                        );
                    }
                    let mut slot = process_chain_slot_from_handle(&registry, *id)?;
                    slot.project_layer = project_layer;
                    slots.push(slot);
                    handles.push(arg.clone());
                }
                registry.defs.clone()
            };
            let chain = crate::process::TrackProcessChain { slots };
            if write_process_chain_state {
                match &tracks {
                    Some(tracks) => {
                        for track in tracks {
                            let mut track_chain = chain.clone();
                            if let Some(existing) = state_for_processes.track_process_chain(*track)
                            {
                                preserve_process_slot_state(&defs, &existing, &mut track_chain);
                            }
                            if !state_for_processes.set_track_process_chain(*track, track_chain) {
                                return Err(format!("track {track} out of range"));
                            }
                        }
                    }
                    None => {
                        let mut project_chain = chain.clone();
                        let existing = state_for_processes.project_process_chain();
                        preserve_process_slot_state(&defs, &existing, &mut project_chain);
                        if !state_for_processes.set_project_process_chain(project_chain) {
                            return Err("failed to update the project process layer".to_string());
                        }
                    }
                }
                publish_process_authoring(&authoring_for_processes, &publish_for_processes);
            }
            match handles.len() {
                0 => Ok(EValue::Bool(true)),
                1 => Ok(handles.remove(0)),
                _ => Ok(process_list(handles)),
            }
        },
    );

    let state_for_lane = Arc::clone(&state);
    let authoring_for_lane = Arc::clone(&process_authoring);
    let publish_for_lane = publish.clone();
    runtime.register_native_with_docs(
        "lane!",
        "(lane! instance :inlet v1 v2 ...)",
        "Replace a lane on an attached process instance in the current pattern (every track it is attached to).",
        move |args, _ctx| {
            let Some(EValue::HostHandle { kind, id, .. }) = args.first() else {
                return Err("lane! expects a process instance handle".to_string());
            };
            if kind != "process" {
                return Err("lane! expects a process instance handle".to_string());
            }
            let inlet = process_symbol_name(
                args.get(1)
                    .ok_or_else(|| "lane! expects an :inlet name".to_string())?,
            )?;
            let mut values = Vec::with_capacity(args.len().saturating_sub(2));
            for arg in &args[2..] {
                match arg {
                    EValue::Number(number) => values.push(*number as f32),
                    EValue::Bool(gate) => values.push(if *gate { 1.0 } else { 0.0 }),
                    other => {
                        return Err(format!(
                            "lane! values must be numbers, got {}",
                            eseqlisp::vm::format_lisp_value(other)
                        ))
                    }
                }
            }
            {
                let registry = authoring_for_lane
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                let instance = registry
                    .instances
                    .iter()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                let lane_backed = registry
                    .defs
                    .iter()
                    .find(|def| def.name == instance.class_name)
                    .map(|def| {
                        def.inlets
                            .iter()
                            .any(|entry| entry.name == inlet && entry.lane)
                    })
                    .unwrap_or(false);
                if !lane_backed {
                    return Err(format!(
                        "inlet '{inlet}' of '{}' is not lane-backed (:lane true)",
                        instance.class_name
                    ));
                }
            }
            let updated = if write_process_chain_state {
                let updated = state_for_lane.set_process_lane_values(
                    crate::process::ProcessInstanceId(*id),
                    &inlet,
                    values.clone(),
                );
                if updated == 0 {
                    return Err(
                        "process instance is not attached to any track (use `processes` first)"
                            .to_string(),
                    );
                }
                updated
            } else {
                0
            };
            {
                let mut registry = authoring_for_lane
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                let instance = registry
                    .instances
                    .iter_mut()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                instance.inlets.insert(
                    inlet.clone(),
                    crate::process::ProcessInletValue::Literal(process_lane_literal(&values)),
                );
            }
            if write_process_chain_state {
                publish_process_authoring(&authoring_for_lane, &publish_for_lane);
            }
            Ok(EValue::Number(updated as f64))
        },
    );

    let state_for_connect = Arc::clone(&state);
    let authoring_for_connect = Arc::clone(&process_authoring);
    let publish_for_connect = publish;
    runtime.register_native_with_docs(
        "connect!",
        "(connect! instance :port (inlet target-instance :inlet))",
        "Connect a process output port to another process instance's inlet.",
        move |args, _ctx| {
            if args.len() != 3 {
                return Err("connect! expects a process handle, port, and inlet target".to_string());
            }
            let Some(EValue::HostHandle { kind, id, .. }) = args.first() else {
                return Err("connect! expects a process handle".to_string());
            };
            if kind != "process" {
                return Err("connect! expects a process handle".to_string());
            }
            let port = process_symbol_name(
                args.get(1)
                    .ok_or_else(|| "connect! expects a port name".to_string())?,
            )?;
            let target = parse_process_connection_target(
                args.get(2)
                    .ok_or_else(|| "connect! expects an inlet target".to_string())?,
            )?;
            {
                let mut registry = authoring_for_connect
                    .lock()
                    .map_err(|_| "failed to lock process registry".to_string())?;
                let instance = registry
                    .instances
                    .iter()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                let def = registry
                    .defs
                    .iter()
                    .find(|def| def.name == instance.class_name)
                    .ok_or_else(|| format!("unknown process class '{}'", instance.class_name))?;
                let single = BTreeMap::from([(port.clone(), Some(target.clone()))]);
                validate_process_port_bindings(def, &single)?;
                let instance = registry
                    .instances
                    .iter_mut()
                    .find(|entry| entry.handle_id.0 == *id)
                    .ok_or_else(|| "unknown process handle".to_string())?;
                instance.bindings.insert(port.clone(), Some(target.clone()));
            }
            let updated = if write_process_chain_state {
                state_for_connect.set_process_port_binding_for_instance(
                    crate::process::ProcessInstanceId(*id),
                    &port,
                    target,
                )
            } else {
                0
            };
            if write_process_chain_state {
                publish_process_authoring(&authoring_for_connect, &publish_for_connect);
            }
            Ok(EValue::Number(updated as f64))
        },
    );
}

pub(in crate::lisp_host) fn register_process_def(
    args: Vec<EValue>,
    vm: &mut eseqlisp::vm::VM,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let name = process_symbol_name(
        args.first()
            .ok_or_else(|| "def-process expects a process name".to_string())?,
    )?;
    let mut def = parse_process_def(&name, &args[1..])?;
    def.source_path = vm
        .current_source_file()
        .map(|path| path.to_string_lossy().into_owned());
    process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?
        .upsert_def(def.clone());
    publish_process_authoring(process_authoring, &publish);
    register_process_constructor_native(vm, &name, process_authoring, process_chain_state, publish);
    Ok(EValue::String(name))
}

pub(in crate::lisp_host) fn register_process_constructor_native(
    vm: &mut eseqlisp::vm::VM,
    name: &str,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) {
    let process_authoring_for_constructor = Arc::clone(process_authoring);
    let chain_state_for_constructor = process_chain_state;
    let publish_for_constructor = publish;
    let class_name = name.to_string();
    vm.register_native_with_vm(name, move |ctor_args, vm| {
        let inlet_names = ctor_args
            .iter()
            .filter_map(|arg| match arg {
                EValue::Keyword(inlet) => Some(inlet.trim_start_matches(':').to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        match construct_process_instance(
            &process_authoring_for_constructor,
            &class_name,
            ctor_args,
            false,
            false,
            None,
            None,
            false,
            chain_state_for_constructor.clone(),
            publish_for_constructor.clone(),
        ) {
            Ok(value) => {
                for inlet in inlet_names {
                    vm.attach_inline_widget_runtime_target(&class_name, &inlet, value.clone());
                }
                publish_process_authoring(
                    &process_authoring_for_constructor,
                    &publish_for_constructor,
                );
                value
            }
            Err(error) => {
                eprintln!("[process] constructor error: {error}");
                EValue::Bool(false)
            }
        }
    });
}

pub(in crate::lisp_host) fn register_process_accumulator_def(
    args: Vec<EValue>,
    vm: &mut eseqlisp::vm::VM,
    process_authoring: &SharedProcessAuthoring,
    process_chain_state: Option<Arc<crate::sequencer::SequencerState>>,
    publish: Option<ProcessPublishHook>,
) -> Result<EValue, String> {
    let name = process_symbol_name(
        args.first()
            .ok_or_else(|| "def-accumulator expects a process name".to_string())?,
    )?;
    let mut def = parse_process_accumulator_def(&name, &args[1..])?;
    def.source_path = vm
        .current_source_file()
        .map(|path| path.to_string_lossy().into_owned());
    process_authoring
        .lock()
        .map_err(|_| "failed to lock process registry".to_string())?
        .upsert_def(def);
    publish_process_authoring(process_authoring, &publish);
    register_process_constructor_native(vm, &name, process_authoring, process_chain_state, publish);
    Ok(EValue::String(name))
}

pub(in crate::lisp_host) fn parse_process_accumulator_def(
    name: &str,
    args: &[EValue],
) -> Result<crate::process::ProcessDef, String> {
    let mut target_port = None;
    let mut target_kind = None;
    let mut target_hint = None;
    let mut amount = None;
    let mut reset_lane = false;
    let mut range = None;
    let mut mode = crate::process::ProcessAccumulatorMode::Wrap;
    let mut seed_policy = crate::process::ProcessSeedPolicy::Locked;
    let mut doc = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = process_symbol_name(&args[idx])?.to_ascii_lowercase();
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("def-accumulator missing value for :{key}"));
        };
        match key.as_str() {
            "target" => {
                if target_port.is_some() {
                    return Err("def-accumulator cannot specify :target more than once".to_string());
                }
                target_port = Some(parse_process_accumulator_target(value)?);
            }
            "target-kind" => target_kind = Some(parse_process_target_kind(value)?),
            "target-hint" => target_hint = Some(parse_process_target_hint(value)?),
            "amount" => amount = Some(parse_process_accumulator_amount_inlet(value)?),
            "reset" => {
                let reset = process_symbol_name(value)?.to_ascii_lowercase();
                reset_lane = reset == "lane";
                if !reset_lane && reset != "none" {
                    return Err("def-accumulator :reset supports :lane or :none".to_string());
                }
            }
            "range" => range = Some(parse_process_accumulator_range(value)?),
            "mode" => mode = parse_process_accumulator_mode(value)?,
            "seed" => seed_policy = parse_process_seed_policy(value)?,
            "doc" => {
                if let EValue::String(value) = value {
                    doc = Some(value.clone());
                }
            }
            other => return Err(format!("def-accumulator unknown key :{other}")),
        }
        idx += 1;
    }
    let mut target_port =
        target_port.ok_or_else(|| "def-accumulator requires :target".to_string())?;
    if target_port.is_mappable() {
        if let Some(kind) = target_kind {
            if kind == crate::process::ProcessTargetKind::ProcessInlet {
                return Err(
                    "def-accumulator does not expose connectable ports; use def-process with :targets ((out :process-inlet))"
                        .to_string(),
                );
            }
            target_port.target_kind = Some(kind);
        }
        if let Some(hint) = target_hint {
            if let Some(kind) = target_port.target_kind {
                if !kind.matches_hint(&hint) {
                    return Err(format!(
                        "def-accumulator :target-hint kind {} is incompatible with :target-kind {}",
                        hint.target_kind().as_str(),
                        kind.as_str()
                    ));
                }
            }
            target_port.target = Some(hint);
        }
    } else if target_kind.is_some() || target_hint.is_some() {
        return Err(
            "def-accumulator :target-kind and :target-hint require :target :mappable".to_string(),
        );
    }
    let amount = amount.ok_or_else(|| "def-accumulator requires :amount".to_string())?;
    let mut inlets = vec![amount.clone()];
    let reset_inlet = if reset_lane {
        let inlet = crate::process::ProcessInletDef {
            name: "reset".to_string(),
            kind: crate::process::ProcessInletKind::Gate,
            min: Some(0.0),
            max: Some(1.0),
            default: EValue::Number(0.0),
            lane: true,
            doc: None,
        };
        inlets.push(inlet);
        Some("reset".to_string())
    } else {
        None
    };
    Ok(crate::process::ProcessDef {
        id: crate::process::stable_process_id(name),
        name: name.to_string(),
        source_path: None,
        doc,
        inlets,
        outlets: Vec::new(),
        state: Vec::new(),
        every: None,
        seed_policy,
        ports: vec![target_port],
        accumulator: Some(crate::process::ProcessAccumulatorSpec {
            amount_inlet: amount.name,
            reset_inlet,
            range,
            mode,
        }),
        run_source: None,
        listens: Vec::new(),
    })
}
