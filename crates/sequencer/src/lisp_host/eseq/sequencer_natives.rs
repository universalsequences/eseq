/*!
The registration hub for the scheduler-side sequencing natives.

`register_sequencer_natives` (and the `_with_accumulators` variant) installs
the bulk of what a scratch-buffer or scheduler script can call: step editing
and querying (`seq-toggle-step`, `seq-step-on`/`-off`, `seq-plock-*`,
`seq-track-steps`, `seq-emit`, ...), `def-sequencer` (dispatching to
`graph_manifest` when a `def-node` sub-form makes it graph-mode),
accumulator emission (`acc-emit`, `acc-chord`, `acc-set-*`/`acc-add-*`
param natives), MIDI-FX scripting (`fx-*`, bodies in `midi_fx`), generator
tick natives (`gen-tick`, `gen-beat`, `gen-rand`, ...), and the `neural-*`
authoring natives (arg parsing in `neural_natives`). The `__register-*`
natives are the internal hooks `def-accumulator`/`def-midi-fx`/
`def-sequencer` expand into.
*/

use super::super::*;

pub const DEF_SEQUENCER_SIGNATURE: &str =
    "(def-sequencer name :resolution timebase :res timebase :tick callback :tick-source source :init callback :shape shape :energy-decay amount :reset-every duration :seed-on-reset amount :max-poly count :max-poly-selection mode :duration duration :dur duration :swing amount ...)";
pub const DEF_SEQUENCER_DOCS: &str =
    "Define a self-clocked Lisp generator with :tick, or a graph sequencer with :shape and graph forms. :resolution/:res select the timebase; :tick-source is the internal prebuilt-source path; :init is reserved for generator initialization.";
pub const DEF_SEQUENCER_KEYWORDS: &[&str] = &[
    ":resolution", ":res", ":tick", ":tick-source", ":init", ":shape", ":energy-decay", ":reset-every",
    ":seed-on-reset", ":max-poly", ":max-poly-selection", ":duration", ":dur", ":swing",
];
pub const SEQ_EMIT_SIGNATURE: &str =
    "(seq-emit :track t :at offset :vel v :note n :dur beats :speed ratio :pan amount :chop count :chord (...) :quantize grid)";
pub const SEQ_EMIT_DOCS: &str =
    "Emit an event from a generator :tick at a musical offset; the engine resolves timing to samples. Long and abbreviated parameter names are both accepted.";
pub const SEQ_EMIT_KEYWORDS: &[&str] = &[
    ":track", ":at", ":vel", ":velocity", ":note", ":transpose", ":trn", ":dur",
    ":duration", ":speed", ":spd", ":pan", ":chop", ":chp", ":chord", ":quantize", ":q",
];
pub const SEQ_EMIT_CONTROL_SIGNATURE: &str =
    "(seq-emit-control :op \"mute\"|\"solo\" :track idx | :group \"name\" :at offset-beats :dur beats)";
pub const SEQ_EMIT_CONTROL_DOCS: &str =
    "Emit a timed mixer-control hold from a generator :tick: the target track or group is \
     muted/soloed from :at for :dur beats. Targets are validated when the hold is applied; \
     an unknown track or group reports a host error and applies nothing.";
pub const SEQ_EMIT_CONTROL_KEYWORDS: &[&str] = &[":op", ":track", ":group", ":at", ":dur"];

pub(in crate::lisp_host) fn register_sequencer_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    context: SharedSequencerEvalContext,
    metadata: SharedSequencerNativeMetadata,
) {
    register_sequencer_natives_with_accumulators(
        runtime,
        state,
        context,
        metadata,
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(GeneratorChannelSnapshot::default())),
    );
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SelectedNeuralNeuron {
    pub pattern_idx: usize,
    pub network_id: u64,
    pub neuron_idx: usize,
}

pub type SharedSelectedNeuralNeurons = Arc<Mutex<BTreeSet<SelectedNeuralNeuron>>>;

pub fn register_neural_authoring_natives(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
) {
    register_neural_authoring_natives_with_selection(
        runtime,
        state,
        Arc::new(Mutex::new(BTreeSet::new())),
    );
}

pub fn register_neural_authoring_natives_with_selection(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    selected_neural_neurons: SharedSelectedNeuralNeurons,
) {
    let state_for_neural_list = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-list",
        "(neural-list)",
        "Return the current pattern's neural network definitions.",
        move |_args, _ctx| {
            Ok(lisp_list(
                state_for_neural_list
                    .current_neural_networks()
                    .iter()
                    .map(neural_network_to_value)
                    .collect(),
            ))
        },
    );

    let state_for_neural_create = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-create",
        "(neural-create :name \"name\" :neurons n [:enabled true] [:weights matrix])",
        "Create a neural network in the current pattern and return its structured description.",
        move |args, ctx| {
            let options = parse_neural_create_args(&args)?;
            let created = state_for_neural_create.edit_current_neural_networks(|networks| {
                let mut network = ProjectNeuralNetwork {
                    id: next_neural_network_id(networks),
                    name: options.name.clone(),
                    enabled: options.enabled,
                    num_neurons: options.num_neurons,
                    weights: options.weights.clone().unwrap_or_else(|| {
                        vec![vec![0.0; options.num_neurons]; options.num_neurons]
                    }),
                    neurons: vec![ProjectNeuron::default(); options.num_neurons],
                    ..ProjectNeuralNetwork::default()
                };
                normalize_project_neural_network_shape(&mut network)?;
                networks.push(network.clone());
                Ok(network)
            })?;
            ctx.set_status(format!("created neural network '{}'", created.name));
            Ok(neural_network_to_value(&created))
        },
    );

    let state_for_neural_describe = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-describe",
        "(neural-describe id-or-name)",
        "Return one neural network definition from the current pattern.",
        move |args, _ctx| {
            let reference = parse_neural_network_ref(
                args.first()
                    .ok_or_else(|| "neural-describe expects network id or name".to_string())?,
            )?;
            let networks = state_for_neural_describe.current_neural_networks();
            let idx = neural_network_index(&networks, &reference)?;
            Ok(neural_network_to_value(&networks[idx]))
        },
    );

    let state_for_neural_select = Arc::clone(&state);
    let selection_for_neural_select = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-select-neuron",
        "(neural-select-neuron id-or-name neuron-index)",
        "Select one neuron for UI authoring and return the full selected-neuron list.",
        move |args, ctx| {
            if args.len() != 2 {
                return Err(
                    "neural-select-neuron expects network id/name and neuron index".to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let networks = state_for_neural_select.current_neural_networks();
            let network_idx = neural_network_index(&networks, &reference)?;
            let network = &networks[network_idx];
            if neuron_idx >= network.num_neurons {
                return Err("neuron index out of range".to_string());
            }
            let pattern_idx = state_for_neural_select.current_scene_index();
            let mut selection = selection_for_neural_select.lock().unwrap();
            selection.clear();
            selection.insert(SelectedNeuralNeuron {
                pattern_idx,
                network_id: network.id,
                neuron_idx,
            });
            ctx.set_status(format!(
                "selected neural neuron {}:{}:{}",
                pattern_idx, network.id, neuron_idx
            ));
            Ok(selected_neural_neurons_to_value(&selection))
        },
    );

    let selection_for_neural_clear = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-clear-selection",
        "(neural-clear-selection)",
        "Clear selected neural neurons and return the empty selected-neuron list.",
        move |_args, _ctx| {
            let mut selection = selection_for_neural_clear.lock().unwrap();
            selection.clear();
            Ok(selected_neural_neurons_to_value(&selection))
        },
    );

    let selection_for_neural_selected = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-selected-neurons",
        "(neural-selected-neurons)",
        "Return selected neural neurons as maps containing :pattern, :network-id, and :neuron.",
        move |_args, _ctx| {
            let selection = selection_for_neural_selected.lock().unwrap();
            Ok(selected_neural_neurons_to_value(&selection))
        },
    );

    let selection_for_neural_selected_predicate = Arc::clone(&selected_neural_neurons);
    let state_for_neural_selected_predicate = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-neuron-selected?",
        "(neural-neuron-selected? network-id neuron-index)",
        "Return true when a neural network neuron is selected in the current pattern.",
        move |args, _ctx| {
            if args.len() != 2 {
                return Err(
                    "neural-neuron-selected? expects network id and neuron index".to_string(),
                );
            }
            let network_id = match &args[0] {
                EValue::Number(id) if id.is_finite() && *id >= 0.0 => *id as u64,
                _ => return Err("network id must be a non-negative number".to_string()),
            };
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let pattern_idx = state_for_neural_selected_predicate.current_scene_index();
            let selection = selection_for_neural_selected_predicate.lock().unwrap();
            Ok(EValue::Bool(selection.contains(&SelectedNeuralNeuron {
                pattern_idx,
                network_id,
                neuron_idx,
            })))
        },
    );

    let state_for_neural_delete = Arc::clone(&state);
    let selection_for_neural_delete = Arc::clone(&selected_neural_neurons);
    runtime.register_native_with_docs(
        "neural-delete",
        "(neural-delete id-or-name)",
        "Delete one neural network from the current pattern.",
        move |args, ctx| {
            let reference = parse_neural_network_ref(
                args.first()
                    .ok_or_else(|| "neural-delete expects network id or name".to_string())?,
            )?;
            let deleted = state_for_neural_delete.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                Ok(networks.remove(idx))
            })?;
            let pattern_idx = state_for_neural_delete.current_scene_index();
            selection_for_neural_delete
                .lock()
                .unwrap()
                .retain(|selected| {
                    selected.pattern_idx != pattern_idx || selected.network_id != deleted.id
                });
            ctx.set_status(format!("deleted neural network '{}'", deleted.name));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_neural_enable = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-enable",
        "(neural-enable id-or-name true)",
        "Enable or disable one neural network in the current pattern.",
        move |args, ctx| {
            if args.len() != 2 {
                return Err("neural-enable expects network id/name and bool".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let enabled = parse_bool_value(&args[1], "neural-enable")?;
            let updated = state_for_neural_enable.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                networks[idx].enabled = enabled;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "neural network '{}' enabled={}",
                updated.name, updated.enabled
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_set = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-set",
        "(neural-set id-or-name :reset-bars 4 :energy-decay 0.994 :max-poly 2)",
        "Set global neural network options.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("neural-set expects network id/name".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let edits = parse_neural_set_args(&args[1..])?;
            let updated = state_for_neural_set.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                apply_neural_set_edits(&mut networks[idx], &edits)?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!("updated neural network '{}'", updated.name));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_neuron = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-neuron",
        "(neural-neuron id-or-name index :route track :resolution :16 :threshold 0.8 :delay 2 :quantize :8 :transpose 0)",
        "Set one neuron's route, clock, threshold, delay, quantize, transpose, and dampening options.",
        move |args, ctx| {
            if args.len() < 2 {
                return Err("neural-neuron expects network id/name and neuron index".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let edits = parse_neural_neuron_args(&args[2..])?;
            let track_count = state_for_neural_neuron.active_track_count();
            let updated = state_for_neural_neuron.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                if neuron_idx >= networks[idx].num_neurons {
                    return Err("neuron index out of range".to_string());
                }
                apply_neural_neuron_edits(
                    &mut networks[idx].neurons[neuron_idx],
                    &edits,
                    track_count,
                )?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "updated neural network '{}' neuron {}",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_plock_instrument = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-plock-instrument",
        "(neural-plock-instrument id-or-name neuron track param value)",
        "Set a target-track instrument parameter p-lock for one neuron using a stored engine value.",
        move |args, ctx| {
            if args.len() != 5 {
                return Err(
                    "neural-plock-instrument expects network, neuron, track, param, value"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let param_idx = parse_nonnegative_usize(&args[3], "instrument param")?;
            let value = parse_value_arg(&args, 4, "instrument p-lock")?;
            let param_id = neural_instrument_param_id(
                &state_for_neural_plock_instrument,
                target_track,
                param_idx,
            )?;
            let updated = state_for_neural_plock_instrument.edit_current_neural_networks(
                |networks| {
                    let idx = neural_network_index(networks, &reference)?;
                    upsert_neural_instrument_plock(
                        &mut networks[idx],
                        neuron_idx,
                        target_track,
                        param_idx,
                        param_id,
                        value,
                    )?;
                    Ok(networks[idx].clone())
                },
            )?;
            ctx.set_status(format!(
                "updated neural network '{}' neuron {} instrument p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_plock_effect = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-plock-effect",
        "(neural-plock-effect id-or-name neuron track slot param value)",
        "Set a target-track audio effect parameter p-lock for one neuron using a stored engine value.",
        move |args, ctx| {
            if args.len() != 6 {
                return Err(
                    "neural-plock-effect expects network, neuron, track, slot, param, value"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let slot_idx = parse_nonnegative_usize(&args[3], "effect slot")?;
            let param_idx = parse_nonnegative_usize(&args[4], "effect param")?;
            let value = parse_value_arg(&args, 5, "effect p-lock")?;
            let param_id =
                neural_effect_param_id(&state_for_neural_plock_effect, target_track, slot_idx, param_idx)?;
            let updated = state_for_neural_plock_effect.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                upsert_neural_effect_plock(
                    &mut networks[idx],
                    neuron_idx,
                    target_track,
                    slot_idx,
                    param_idx,
                    param_id,
                    value,
                )?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "updated neural network '{}' neuron {} effect p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_clear_instrument = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-clear-instrument-plock",
        "(neural-clear-instrument-plock id-or-name neuron track param)",
        "Clear a target-track instrument parameter p-lock from one neuron.",
        move |args, ctx| {
            if args.len() != 4 {
                return Err(
                    "neural-clear-instrument-plock expects network, neuron, track, param"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let param_idx = parse_nonnegative_usize(&args[3], "instrument param")?;
            let updated =
                state_for_neural_clear_instrument.edit_current_neural_networks(|networks| {
                    let idx = neural_network_index(networks, &reference)?;
                    clear_neural_instrument_plock(
                        &mut networks[idx],
                        neuron_idx,
                        target_track,
                        param_idx,
                    )?;
                    Ok(networks[idx].clone())
                })?;
            ctx.set_status(format!(
                "cleared neural network '{}' neuron {} instrument p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_clear_effect = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-clear-effect-plock",
        "(neural-clear-effect-plock id-or-name neuron track slot param)",
        "Clear a target-track audio effect parameter p-lock from one neuron.",
        move |args, ctx| {
            if args.len() != 5 {
                return Err(
                    "neural-clear-effect-plock expects network, neuron, track, slot, param"
                        .to_string(),
                );
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let neuron_idx = parse_nonnegative_usize(&args[1], "neuron index")?;
            let target_track = parse_nonnegative_usize(&args[2], "track")?;
            let slot_idx = parse_nonnegative_usize(&args[3], "effect slot")?;
            let param_idx = parse_nonnegative_usize(&args[4], "effect param")?;
            let updated =
                state_for_neural_clear_effect.edit_current_neural_networks(|networks| {
                    let idx = neural_network_index(networks, &reference)?;
                    clear_neural_effect_plock(
                        &mut networks[idx],
                        neuron_idx,
                        target_track,
                        slot_idx,
                        param_idx,
                    )?;
                    Ok(networks[idx].clone())
                })?;
            ctx.set_status(format!(
                "cleared neural network '{}' neuron {} effect p-lock",
                updated.name, neuron_idx
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_weights = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-weights",
        "(neural-weights id-or-name '((0 1) (0 0)))",
        "Replace a neural network's full NxN weight matrix. Rows are from-neuron, columns are to-neuron.",
        move |args, ctx| {
            if args.len() != 2 {
                return Err("neural-weights expects network id/name and matrix".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let updated = state_for_neural_weights.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                networks[idx].weights =
                    parse_neural_weight_matrix(&args[1], networks[idx].num_neurons)?;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!("updated neural network '{}' weights", updated.name));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_weight = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-weight",
        "(neural-weight id-or-name :from 0 :to 1 :value 0.8)",
        "Set one matrix cell. Rows are from-neuron, columns are to-neuron.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("neural-weight expects network id/name".to_string());
            }
            let reference = parse_neural_network_ref(&args[0])?;
            let edit = parse_neural_weight_args(&args[1..])?;
            let updated = state_for_neural_weight.edit_current_neural_networks(|networks| {
                let idx = neural_network_index(networks, &reference)?;
                let n = networks[idx].num_neurons;
                if edit.from >= n || edit.to >= n {
                    return Err("neural-weight from/to index out of range".to_string());
                }
                normalize_project_neural_network_shape(&mut networks[idx])?;
                networks[idx].weights[edit.from][edit.to] = edit.value;
                Ok(networks[idx].clone())
            })?;
            ctx.set_status(format!(
                "updated neural network '{}' weight {} -> {}",
                updated.name, edit.from, edit.to
            ));
            Ok(neural_network_to_value(&updated))
        },
    );

    let state_for_neural_reset_step = Arc::clone(&state);
    runtime.register_native_with_docs(
        "neural-reset-step",
        "(neural-reset-step :track 0 :step 0 true) | (neural-reset-step track step true)",
        "Set or clear the dedicated neural reset flag for a step.",
        move |args, ctx| {
            let reset = parse_neural_reset_step_args(&args)?;
            state_for_neural_reset_step.set_neural_reset_step(
                reset.track,
                reset.step,
                reset.enabled,
            )?;
            ctx.set_status(format!(
                "track {} step {} neural-reset={}",
                reset.track, reset.step, reset.enabled
            ));
            Ok(EValue::Bool(reset.enabled))
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::lisp_host) fn register_sequencer_natives_with_accumulators(
    runtime: &mut Runtime,
    state: Arc<crate::sequencer::SequencerState>,
    context: SharedSequencerEvalContext,
    metadata: SharedSequencerNativeMetadata,
    accumulators: SharedRegisteredAccumulators,
    midi_fx: SharedRegisteredMidiFx,
    pending_midi_fx_params: SharedPendingMidiFxParams,
    midi_fx_state: SharedMidiFxState,
    accumulator_eval: SharedAccumulatorEvalContext,
    sequencers: SharedRegisteredSequencers,
    generator_tick: SharedGeneratorTickContext,
    generator_channels: SharedGeneratorChannels,
) {
    let current_track =
        |ctx: &SharedSequencerEvalContext| ctx.lock().map(|guard| guard.track).unwrap_or(0);
    let current_step =
        |ctx: &SharedSequencerEvalContext| ctx.lock().map(|guard| guard.cursor_step).unwrap_or(0);

    let _ = install_runtime_globals(runtime, &context, &metadata, &[]);

    // `def-sequencer` is a plain variadic builtin (NOT a macro): eseqlisp macros are
    // fixed-arity with no unquote-splicing, and builtins already receive variadic
    // evaluated args — so `(def-sequencer "name" :resolution :16 :tick (lambda () ...))`
    // works directly, with the :tick closure usable on whichever VM evaluates the form.
    // `__register-sequencer` is kept as the lower-level alias.
    let sequencers_for_register = Arc::clone(&sequencers);
    runtime.register_native_with_docs_and_keywords(
        "def-sequencer",
        DEF_SEQUENCER_SIGNATURE,
        DEF_SEQUENCER_DOCS,
        DEF_SEQUENCER_KEYWORDS.iter().copied(),
        move |args, _ctx| register_sequencer_impl(&args, &sequencers_for_register),
    );
    let sequencers_for_register_alias = Arc::clone(&sequencers);
    runtime.register_native_with_docs(
        "__register-sequencer",
        "(__register-sequencer name :resolution timebase :tick callback | :tick-source source)",
        "Lower-level alias for def-sequencer; :tick-source registers prebuilt scheduler source.",
        move |args, _ctx| register_sequencer_impl(&args, &sequencers_for_register_alias),
    );

    let generator_tick_for_emit = Arc::clone(&generator_tick);
    runtime.register_native_with_docs_and_keywords(
        "seq-emit",
        SEQ_EMIT_SIGNATURE,
        SEQ_EMIT_DOCS,
        SEQ_EMIT_KEYWORDS.iter().copied(),
        move |args, _ctx| {
            let mut guard = generator_tick_for_emit
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("seq-emit called outside a generator tick".to_string());
            };
            let event = build_seq_emit_event(&args, ctx)?;
            ctx.emitted.push(event);
            Ok(EValue::Bool(true))
        },
    );

    let generator_tick_for_control = Arc::clone(&generator_tick);
    runtime.register_native_with_docs_and_keywords(
        "seq-emit-control",
        SEQ_EMIT_CONTROL_SIGNATURE,
        SEQ_EMIT_CONTROL_DOCS,
        SEQ_EMIT_CONTROL_KEYWORDS.iter().copied(),
        move |args, _ctx| {
            let mut guard = generator_tick_for_control
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("seq-emit-control called outside a generator tick".to_string());
            };
            let control = build_seq_emit_control(&args)?;
            ctx.controls.push(control);
            Ok(EValue::Bool(true))
        },
    );

    let generator_tick_for_tick = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-tick",
        "(gen-tick)",
        "0-based count of this generator's boundary crossings since reset.",
        move |_args, _ctx| {
            let guard = generator_tick_for_tick
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-tick called outside a generator tick".to_string());
            };
            Ok(EValue::Number(ctx.tick_index as f64))
        },
    );

    let generator_tick_for_beat = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-beat",
        "(gen-beat)",
        "Musical position of this boundary in quarter-note beats.",
        move |_args, _ctx| {
            let guard = generator_tick_for_beat
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-beat called outside a generator tick".to_string());
            };
            Ok(EValue::Number(ctx.beat))
        },
    );

    let generator_tick_for_bar = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-bar",
        "(gen-bar)",
        "0-based bar index of this boundary (4 beats per bar).",
        move |_args, _ctx| {
            let guard = generator_tick_for_bar
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-bar called outside a generator tick".to_string());
            };
            Ok(EValue::Number((ctx.beat / 4.0).floor()))
        },
    );

    let generator_tick_for_phase = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-phase",
        "(gen-phase)",
        "Position within the current bar in beats (0..4).",
        move |_args, _ctx| {
            let guard = generator_tick_for_phase
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("gen-phase called outside a generator tick".to_string());
            };
            Ok(EValue::Number(ctx.beat.rem_euclid(4.0)))
        },
    );

    let generator_tick_for_rand = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "gen-rand",
        "(gen-rand)",
        "Deterministic pseudo-random float in [0,1), seeded per generator.",
        move |_args, _ctx| {
            let mut guard = generator_tick_for_rand
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("gen-rand called outside a generator tick".to_string());
            };
            ctx.random_state = ctx.random_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let bits = gen_splitmix64(ctx.random_state);
            Ok(EValue::Number((bits >> 11) as f64 / (1u64 << 53) as f64))
        },
    );

    let generator_tick_for_state_get = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "state-get",
        "(state-get \"key\") | (state-get \"key\" default)",
        "Read a persistent per-generator scalar state cell (0.0, or the given default, if unset).",
        move |args, _ctx| {
            let key = match args.first() {
                Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => s.clone(),
                _ => return Err("state-get expects a string key".to_string()),
            };
            let default = match args.get(1) {
                Some(EValue::Number(n)) => *n,
                _ => 0.0,
            };
            let guard = generator_tick_for_state_get
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_ref() else {
                return Err("state-get called outside a generator tick".to_string());
            };
            Ok(EValue::Number(
                ctx.state.get(&key).copied().unwrap_or(default),
            ))
        },
    );

    let generator_tick_for_state_set = Arc::clone(&generator_tick);
    runtime.register_native_with_docs(
        "state-set!",
        "(state-set! \"key\" value)",
        "Write a persistent per-generator scalar state cell; returns the value.",
        move |args, _ctx| {
            let key = match args.first() {
                Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => s.clone(),
                _ => return Err("state-set! expects a string key".to_string()),
            };
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("state-set! expects (state-set! \"key\" number)".to_string());
            };
            let mut guard = generator_tick_for_state_set
                .lock()
                .map_err(|_| "failed to lock generator tick context".to_string())?;
            let Some(ctx) = guard.as_mut() else {
                return Err("state-set! called outside a generator tick".to_string());
            };
            ctx.state.insert(key, *value);
            Ok(EValue::Number(*value))
        },
    );

    let generator_tick_for_chan_get = Arc::clone(&generator_tick);
    let generator_channels_for_chan_get = Arc::clone(&generator_channels);
    runtime.register_native_with_docs(
        "chan-get",
        "(chan-get \"name\") | (chan-get \"name\" default)",
        "Read a process channel's current value from a generator :tick (nil, or the given default, if unset). Read-only: values are a per-chunk snapshot, so a process write lands on subsequent ticks.",
        move |args, _ctx| {
            let name = match args.first() {
                Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => s.clone(),
                _ => return Err("chan-get expects a string channel name".to_string()),
            };
            {
                let guard = generator_tick_for_chan_get
                    .lock()
                    .map_err(|_| "failed to lock generator tick context".to_string())?;
                if guard.is_none() {
                    return Err("chan-get called outside a generator tick".to_string());
                }
            }
            let channels = generator_channels_for_chan_get
                .lock()
                .map_err(|_| "failed to lock generator channel snapshot".to_string())?
                .clone();
            Ok(channels
                .values
                .get(&name)
                .cloned()
                .unwrap_or_else(|| args.get(1).cloned().unwrap_or(EValue::Nil)))
        },
    );

    let generator_channels_for_chan_epoch = Arc::clone(&generator_channels);
    runtime.register_native_with_docs(
        "chan-epoch",
        "(chan-epoch)",
        "Return the current process-channel payload generation.",
        move |_args, _ctx| {
            let epoch = generator_channels_for_chan_epoch
                .lock()
                .map_err(|_| "failed to lock generator channel snapshot".to_string())?
                .payload_epoch;
            Ok(EValue::Number(epoch as f64))
        },
    );

    runtime.register_native_with_docs(
        "gen-offset",
        "(gen-offset :16 n)",
        "Beats offset = n steps at a timebase, for seq-emit :at.",
        move |args, _ctx| {
            let timebase = parse_timebase_arg(&args, 0)?;
            let Some(EValue::Number(n)) = args.get(1) else {
                return Err("gen-offset expects (gen-offset :timebase n)".to_string());
            };
            Ok(EValue::Number(
                *n * timebase.step_beats(crate::generator::GENERATOR_RESOLUTION_REF_STEPS),
            ))
        },
    );

    runtime.register_native_with_docs(
        "beats",
        "(beats :8) | (beats 0.25)",
        "Beats in one step of a timebase, or a numeric beat duration.",
        move |args, _ctx| {
            if let Some(EValue::Number(n)) = args.first() {
                return Ok(EValue::Number(n.max(0.0)));
            }
            let timebase = parse_timebase_arg(&args, 0)?;
            Ok(EValue::Number(timebase.step_beats(
                crate::generator::GENERATOR_RESOLUTION_REF_STEPS,
            )))
        },
    );

    let accumulators_for_register = Arc::clone(&accumulators);
    runtime.register_native_with_docs(
        "__register-accumulator",
        "(__register-accumulator name callback)",
        "Internal helper used by def-accumulator to register a named scheduler-side trigger mutation callback.",
        move |args, ctx| {
            let Some(name) = args.first() else {
                return Err("expected accumulator name".to_string());
            };
            let name = match name {
                EValue::String(name) => name.clone(),
                _ => return Err("expected accumulator name string".to_string()),
            };
            let Some(callback) = args.get(1) else {
                return Err("expected accumulator callback".to_string());
            };
            let callback = match callback {
                EValue::Closure(_, _) => RegisteredAccumulatorCallback::Closure(callback.clone()),
                EValue::String(source) => RegisteredAccumulatorCallback::Source(source.clone()),
                other => RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(other)),
            };
            let mut registry = accumulators_for_register
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
            ctx.set_status(format!("registered accumulator '{name}'"));
            Ok(EValue::Bool(true))
        },
    );

    let midi_fx_for_register = Arc::clone(&midi_fx);
    let pending_params_for_register = Arc::clone(&pending_midi_fx_params);
    runtime.register_native_with_docs(
        "__register-midi-fx",
        "(__register-midi-fx name callback)",
        "Internal helper used by def-midi-fx to register a named scheduler-side MIDI FX callback.",
        move |args, ctx| {
            let Some(name) = args.first() else {
                return Err("expected MIDI FX name".to_string());
            };
            let name = match name {
                EValue::String(name) => name.clone(),
                _ => return Err("expected MIDI FX name string".to_string()),
            };
            let Some(callback) = args.get(1) else {
                return Err("expected MIDI FX callback".to_string());
            };
            let callback = match callback {
                EValue::Closure(_, _) => RegisteredAccumulatorCallback::Closure(callback.clone()),
                EValue::String(source) => RegisteredAccumulatorCallback::Source(source.clone()),
                other => {
                    RegisteredAccumulatorCallback::Source(eseqlisp::vm::format_lisp_source(other))
                }
            };
            let mut params = pending_params_for_register
                .lock()
                .map_err(|_| "failed to lock pending MIDI FX params".to_string())?
                .drain(..)
                .collect::<Vec<_>>();
            ensure_enabled_param(&mut params);
            let params = params
                .into_iter()
                .enumerate()
                .map(|(idx, mut param)| {
                    param.node_param_idx = idx as u32;
                    param
                })
                .collect::<Vec<_>>();
            let mut registry = midi_fx_for_register
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?;
            if let Some(existing) = registry.iter_mut().find(|entry| entry.name == name) {
                existing.callback = callback.clone();
                existing.params = params.clone();
            } else {
                registry.push(RegisteredAccumulator {
                    name: name.clone(),
                    callback: callback.clone(),
                    params: params.clone(),
                });
            }
            ctx.set_status(format!("registered MIDI FX '{name}'"));
            Ok(EValue::Bool(true))
        },
    );

    let pending_params_for_param = Arc::clone(&pending_midi_fx_params);
    runtime.register_native_with_docs(
        "midi-fx-param",
        "(midi-fx-param \"name\" :default value :min value :max value :role symbol :enum \"a\" \"b\" ...)",
        "Declare a plockable parameter for the next def-midi-fx in a folder MIDI FX source.",
        move |args, _ctx| {
            let Some(name_value) = args.first() else {
                return Err("midi-fx-param expects a name".to_string());
            };
            let name = match name_value {
                EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => {
                    name.trim_start_matches('@').to_string()
                }
                _ => return Err("midi-fx-param name must be string/symbol/keyword".to_string()),
            };
            let param = parse_midi_fx_param_descriptor(&name, &args[1..])?;
            pending_params_for_param
                .lock()
                .map_err(|_| "failed to lock pending MIDI FX params".to_string())?
                .push(param);
            Ok(EValue::Bool(true))
        },
    );

    let state_for_reset_acc = Arc::clone(&state);
    let context_for_reset_acc = Arc::clone(&context);
    runtime.register_native_with_docs(
        "reset-acc",
        "(reset-acc) | (reset-acc track) | (reset-acc :all)",
        "Reset scheduler accumulator state for the current track, a specific 0-based track, or all tracks.",
        move |args, ctx| {
            if args.is_empty() {
                let track_idx = current_track(&context_for_reset_acc);
                state_for_reset_acc.request_accumulator_reset(track_idx);
                ctx.set_status(format!("reset accumulator for track {track_idx}"));
                return Ok(EValue::Bool(true));
            }
            match &args[0] {
                EValue::Keyword(name) if name == "all" => {
                    state_for_reset_acc.request_all_accumulator_resets();
                    ctx.set_status("reset accumulators for all tracks");
                    Ok(EValue::Bool(true))
                }
                EValue::Number(track) if *track >= 0.0 => {
                    let track_idx = *track as usize;
                    state_for_reset_acc.request_accumulator_reset(track_idx);
                    ctx.set_status(format!("reset accumulator for track {track_idx}"));
                    Ok(EValue::Bool(true))
                }
                _ => Err("reset-acc expects no args, a 0-based track index, or :all".to_string()),
            }
        },
    );

    let state_for_use_acc = Arc::clone(&state);
    let context_for_use_acc = Arc::clone(&context);
    let accumulators_for_use_acc = Arc::clone(&accumulators);
    runtime.register_native_with_docs(
        "seq-use-accumulator",
        "(seq-use-accumulator name) | (seq-use-accumulator track name)",
        "Assign a built-in or scratch accumulator to the current track, or to a specific 0-based track.",
        move |args, ctx| {
            let (track_idx, label) = match args.as_slice() {
                [EValue::String(label)] => (current_track(&context_for_use_acc), label.clone()),
                [EValue::Number(track), EValue::String(label)] if *track >= 0.0 => {
                    (*track as usize, label.clone())
                }
                _ => {
                    return Err(
                        "seq-use-accumulator expects a name string or track/name".to_string()
                    )
                }
            };
            if track_idx >= state_for_use_acc.active_track_count() {
                return Err("track out of range".to_string());
            }

            let mut names = crate::accumulator::ACCUMULATOR_REGISTRY
                .iter()
                .map(|def| def.name.to_string())
                .collect::<Vec<_>>();
            let builtin_count = names.len();
            names.extend(
                accumulators_for_use_acc
                    .lock()
                    .map_err(|_| "failed to lock accumulator registry".to_string())?
                    .iter()
                    .map(|entry| entry.name.clone()),
            );
            let Some(idx) = names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&label))
            else {
                return Err(format!("unknown accumulator '{label}'"));
            };

            let tp = &state_for_use_acc.pattern.track_params[track_idx];
            tp.set_accumulator_idx(idx);
            if idx < builtin_count {
                tp.set_script_accumulator_name(None);
                if let Some(def) = crate::accumulator::ACCUMULATOR_REGISTRY.get(idx) {
                    tp.set_accum_limit(def.default_limit);
                }
            } else {
                tp.set_script_accumulator_name(Some(names[idx].clone()));
            }
            state_for_use_acc.request_accumulator_reset(track_idx);
            state_for_use_acc.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} accumulator {}", names[idx]));
            Ok(EValue::String(names[idx].clone()))
        },
    );

    let state_for_use_midi_fx = Arc::clone(&state);
    let context_for_use_midi_fx = Arc::clone(&context);
    let midi_fx_for_use = Arc::clone(&midi_fx);
    runtime.register_native_with_docs(
        "seq-use-midi-fx",
        "(seq-use-midi-fx name...) | (seq-use-midi-fx track name...)",
        "Assign a scratch MIDI FX chain to the current track, or to a specific 0-based track.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("seq-use-midi-fx expects at least one MIDI FX name".to_string());
            }
            let (track_idx, labels_start) = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => (*track as usize, 1),
                _ => (current_track(&context_for_use_midi_fx), 0),
            };
            if track_idx >= state_for_use_midi_fx.active_track_count() {
                return Err("track out of range".to_string());
            }
            let mut chain = Vec::new();
            for arg in args.iter().skip(labels_start) {
                match arg {
                    EValue::String(label) => chain.push(label.clone()),
                    _ => return Err("seq-use-midi-fx expects string MIDI FX names".to_string()),
                }
            }
            if chain.is_empty() {
                return Err("seq-use-midi-fx expects at least one MIDI FX name".to_string());
            }
            let registry = midi_fx_for_use
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?
                .clone();
            let names = registry
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>();
            for label in &chain {
                if !names.iter().any(|name| name.eq_ignore_ascii_case(label)) {
                    return Err(format!("unknown MIDI FX '{label}'"));
                }
            }
            state_for_use_midi_fx.pattern.track_params[track_idx].set_midi_fx_chain(chain.clone());
            for slot in &state_for_use_midi_fx.pattern.midi_fx_slots[track_idx] {
                slot.clear();
            }
            for (slot_idx, label) in chain.iter().enumerate() {
                if slot_idx >= state_for_use_midi_fx.pattern.midi_fx_slots[track_idx].len() {
                    break;
                }
                if let Some(entry) = registry
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(label))
                {
                    let desc = EffectDescriptor {
                        name: entry.name.clone(),
                        params: entry.params.clone(),
                        input_channels: 0,
                        output_channels: 0,
                        instrument_modulators: Vec::new(),
                        instrument_modulation_targets: Vec::new(),
                        tensor_params: Vec::new(),
                    };
                    state_for_use_midi_fx.pattern.midi_fx_slots[track_idx][slot_idx]
                        .sync_descriptor(&desc, 0);
                }
            }
            state_for_use_midi_fx.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} MIDI FX {:?}", chain));
            Ok(lisp_list(chain.into_iter().map(EValue::String).collect()))
        },
    );

    let state_for_clear_midi_fx = Arc::clone(&state);
    let context_for_clear_midi_fx = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-clear-midi-fx",
        "(seq-clear-midi-fx) | (seq-clear-midi-fx track)",
        "Clear the MIDI FX chain for the current track or a specific 0-based track.",
        move |args, ctx| {
            let track_idx = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => *track as usize,
                None => current_track(&context_for_clear_midi_fx),
                _ => return Err("seq-clear-midi-fx expects no args or a track index".to_string()),
            };
            if track_idx >= state_for_clear_midi_fx.active_track_count() {
                return Err("track out of range".to_string());
            }
            state_for_clear_midi_fx.pattern.track_params[track_idx].set_midi_fx_chain(Vec::new());
            for slot in &state_for_clear_midi_fx.pattern.midi_fx_slots[track_idx] {
                slot.clear();
            }
            state_for_clear_midi_fx.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} MIDI FX cleared"));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_midi_fx_chain = Arc::clone(&state);
    let context_for_midi_fx_chain = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-midi-fx-chain",
        "(seq-midi-fx-chain) | (seq-midi-fx-chain track)",
        "Return the MIDI FX chain for the current track or a specific 0-based track.",
        move |args, _ctx| {
            let track_idx = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => *track as usize,
                None => current_track(&context_for_midi_fx_chain),
                _ => return Err("seq-midi-fx-chain expects no args or a track index".to_string()),
            };
            if track_idx >= state_for_midi_fx_chain.active_track_count() {
                return Err("track out of range".to_string());
            }
            Ok(lisp_list(
                state_for_midi_fx_chain.pattern.track_params[track_idx]
                    .midi_fx_chain()
                    .into_iter()
                    .map(EValue::String)
                    .collect(),
            ))
        },
    );

    register_neural_authoring_natives(runtime, Arc::clone(&state));

    let state_for_set_midi_fx_param = Arc::clone(&state);
    let context_for_set_midi_fx_param = Arc::clone(&context);
    let midi_fx_for_set_param = Arc::clone(&midi_fx);
    runtime.register_native_with_docs(
        "seq-set-midi-fx-param",
        "(seq-set-midi-fx-param slot param value) | (seq-set-midi-fx-param track slot param value)",
        "Set a MIDI FX slot default parameter on the current track or a specific track.",
        move |args, ctx| {
            let (track_idx, idx) = match args.first() {
                Some(EValue::Number(track)) if args.len() >= 4 && *track >= 0.0 => {
                    (*track as usize, 1)
                }
                _ => (current_track(&context_for_set_midi_fx_param), 0),
            };
            if track_idx >= state_for_set_midi_fx_param.active_track_count() {
                return Err("track out of range".to_string());
            }
            let Some(EValue::Number(slot)) = args.get(idx) else {
                return Err("seq-set-midi-fx-param expects numeric slot".to_string());
            };
            let slot_idx = *slot as usize;
            let Some(param_ref) = args.get(idx + 1) else {
                return Err("seq-set-midi-fx-param expects param".to_string());
            };
            let value = parse_value_arg(&args, idx + 2, "MIDI FX param")?;
            let registry = midi_fx_for_set_param
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?
                .clone();
            let param_desc = midi_fx_param_descriptor_for_slot(
                &state_for_set_midi_fx_param,
                &registry,
                track_idx,
                slot_idx,
                param_ref,
            )?;
            let param_idx = param_desc.node_param_idx as usize;
            let slot = state_for_set_midi_fx_param
                .pattern
                .midi_fx_slots
                .get(track_idx)
                .and_then(|slots| slots.get(slot_idx))
                .ok_or_else(|| "MIDI FX slot out of range".to_string())?;
            slot.defaults.set(param_idx, param_desc.clamp(value));
            state_for_set_midi_fx_param.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {track_idx} MIDI FX slot {slot_idx} param {param_idx}"
            ));
            Ok(EValue::Number(slot.defaults.get(param_idx) as f64))
        },
    );

    let state_for_plock_midi_fx = Arc::clone(&state);
    let context_for_plock_midi_fx = Arc::clone(&context);
    let midi_fx_for_plock = Arc::clone(&midi_fx);
    runtime.register_native_with_docs(
        "seq-plock-midi-fx",
        "(seq-plock-midi-fx step slot param value) | (seq-plock-midi-fx track step slot param value)",
        "Set a MIDI FX parameter p-lock on a step.",
        move |args, ctx| {
            let (track_idx, idx) = match args.first() {
                Some(EValue::Number(track)) if args.len() >= 5 && *track >= 0.0 => {
                    (*track as usize, 1)
                }
                _ => (current_track(&context_for_plock_midi_fx), 0),
            };
            if track_idx >= state_for_plock_midi_fx.active_track_count() {
                return Err("track out of range".to_string());
            }
            let Some(EValue::Number(step)) = args.get(idx) else {
                return Err("seq-plock-midi-fx expects numeric step".to_string());
            };
            let Some(EValue::Number(slot)) = args.get(idx + 1) else {
                return Err("seq-plock-midi-fx expects numeric slot".to_string());
            };
            let step_idx = (*step as usize).min(crate::sequencer::MAX_STEPS - 1);
            let slot_idx = *slot as usize;
            let Some(param_ref) = args.get(idx + 2) else {
                return Err("seq-plock-midi-fx expects param".to_string());
            };
            let value = parse_value_arg(&args, idx + 3, "MIDI FX p-lock")?;
            let registry = midi_fx_for_plock
                .lock()
                .map_err(|_| "failed to lock MIDI FX registry".to_string())?
                .clone();
            let param_desc = midi_fx_param_descriptor_for_slot(
                &state_for_plock_midi_fx,
                &registry,
                track_idx,
                slot_idx,
                param_ref,
            )?;
            let param_idx = param_desc.node_param_idx as usize;
            let slot = state_for_plock_midi_fx
                .pattern
                .midi_fx_slots
                .get(track_idx)
                .and_then(|slots| slots.get(slot_idx))
                .ok_or_else(|| "MIDI FX slot out of range".to_string())?;
            slot.set_plock(step_idx, param_idx, param_desc.clamp(value));
            state_for_plock_midi_fx.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {track_idx} step {step_idx} MIDI FX slot {slot_idx} param {param_idx}"
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_midi_fx_position = Arc::clone(&state);
    let context_for_midi_fx_position = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-midi-fx-position",
        "(seq-set-midi-fx-position :post-accumulator) | (seq-set-midi-fx-position track :post-accumulator)",
        "Set whether the track MIDI FX chain runs before or after the visible accumulator slot.",
        move |args, ctx| {
            if args.is_empty() {
                return Err("seq-set-midi-fx-position expects a position".to_string());
            }
            let (track_idx, pos_idx) = match args.first() {
                Some(EValue::Number(track)) if *track >= 0.0 => (*track as usize, 1),
                _ => (current_track(&context_for_midi_fx_position), 0),
            };
            if track_idx >= state_for_midi_fx_position.active_track_count() {
                return Err("track out of range".to_string());
            }
            let position = match args.get(pos_idx) {
                Some(EValue::Keyword(name)) | Some(EValue::String(name))
                    if name == "post-accumulator" || name == "post" =>
                {
                    crate::sequencer::MidiFxPosition::PostAccumulator
                }
                Some(EValue::Keyword(name)) | Some(EValue::String(name))
                    if name == "pre-accumulator" || name == "pre" =>
                {
                    return Err(
                        "pre-accumulator MIDI FX position is not implemented yet".to_string()
                    );
                }
                _ => {
                    return Err(
                        "seq-set-midi-fx-position expects :pre-accumulator or :post-accumulator"
                            .to_string(),
                    )
                }
            };
            state_for_midi_fx_position.pattern.track_params[track_idx]
                .set_midi_fx_position(position);
            state_for_midi_fx_position.publish_scheduler_snapshot();
            ctx.set_status(format!("track {track_idx} MIDI FX position {position:?}"));
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_suppress = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-suppress",
        "(acc-suppress)",
        "Suppress the source trigger for the current accumulator evaluation.",
        move |_args, _ctx| {
            let mut guard = acc_eval_for_suppress
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            eval.suppressed = true;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_chord = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-chord",
        "(acc-chord)",
        "Return the current trigger chord as a list of transpose values.",
        move |_args, _ctx| {
            let guard = acc_eval_for_chord
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            Ok(lisp_list(
                eval.chord
                    .iter()
                    .map(|note| EValue::Number(*note as f64))
                    .collect(),
            ))
        },
    );

    let acc_eval_for_chord_durations = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-chord-durations",
        "(acc-chord-durations)",
        "Return the current trigger chord note durations in source step units.",
        move |_args, _ctx| {
            let guard = acc_eval_for_chord_durations
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            let durations = accumulator_chord_notes(eval)
                .into_iter()
                .map(|note| EValue::Number(note.duration_steps as f64))
                .collect();
            Ok(lisp_list(durations))
        },
    );

    let acc_eval_for_arp_count = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-arp-count",
        "(acc-arp-count :16)",
        "Return the number of arp ticks needed to cover the current chord durations at a timebase.",
        move |args, _ctx| {
            let guard = acc_eval_for_arp_count
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            let timebase = parse_timebase_arg(&args, 0)?;
            let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
            Ok(EValue::Number(
                accumulator_arp_count(eval, rate_beats) as f64
            ))
        },
    );

    let acc_eval_for_arp_note = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-arp-note",
        "(acc-arp-note :16 tick)",
        "Return the chord note for an arp tick at a timebase, or nil once that note's duration has ended.",
        move |args, _ctx| {
            let guard = acc_eval_for_arp_note
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("accumulator context not active".to_string());
            };
            let timebase = parse_timebase_arg(&args, 0)?;
            let Some(EValue::Number(tick)) = args.get(1) else {
                return Err("acc-arp-note expects numeric tick".to_string());
            };
            if *tick < 0.0 {
                return Ok(EValue::Nil);
            }
            let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
            Ok(accumulator_arp_note(eval, rate_beats, *tick as usize)
                .map(|note| EValue::Number(note as f64))
                .unwrap_or(EValue::Nil))
        },
    );

    let acc_eval_for_arp_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-arp-emit",
        "(acc-arp-emit :16 tick :vel 0.8 ...)",
        "Emit one duration-aware arpeggiated note for a tick. Returns false when that note lane has ended.",
        move |args, _ctx| {
            let mut guard = acc_eval_for_arp_emit
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let timebase = parse_timebase_arg(&args, 0)?;
            let Some(EValue::Number(tick)) = args.get(1) else {
                return Err("acc-arp-emit expects numeric tick".to_string());
            };
            if *tick < 0.0 {
                return Ok(EValue::Bool(false));
            }
            let rate_beats = timebase.step_beats(eval.num_steps).max(0.0) as f32;
            let Some(note) = accumulator_arp_note(eval, rate_beats, *tick as usize) else {
                return Ok(EValue::Bool(false));
            };

            let mut resolved = eval.resolved;
            resolved.transpose = note;
            if eval.step_beats > 0.0 {
                resolved.duration = (rate_beats / eval.step_beats).max(0.0);
            }
            let mut chord = Vec::new();
            let mut chord_durations = Vec::new();
            let target_track =
                apply_acc_emit_overrides(&args, 2, &mut resolved, &mut chord, &mut chord_durations)?;
            eval.emitted.push(EmittedAccumulatorEvent {
                offset_beats: *tick as f32 * rate_beats,
                track: target_track,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose: eval.chord_step_transpose,
                effect_params: eval.effect_params.clone(),
                instrument_params: eval.instrument_params.clone(),
            });
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-emit",
        "(acc-emit offset :vel value :note transpose ...) | (acc-emit :16 value ...)",
        "Emit a derived trigger at a musical offset. Numeric offsets use the source step's timebase; an initial timebase keyword overrides that unit.",
        move |args, _ctx| {
            let mut guard = acc_eval_for_emit
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let (offset_beats, idx) = parse_acc_emit_offset(&args, eval.step_beats, eval.num_steps)?;
            let mut resolved = eval.resolved;
            let mut chord = eval.chord.clone();
            let mut chord_durations = eval.chord_durations.clone();
            let chord_step_transpose = eval.chord_step_transpose;
            let target_track =
                apply_acc_emit_overrides(&args, idx, &mut resolved, &mut chord, &mut chord_durations)?;
            eval.emitted.push(EmittedAccumulatorEvent {
                offset_beats,
                track: target_track,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose,
                effect_params: eval.effect_params.clone(),
                instrument_params: eval.instrument_params.clone(),
            });
            Ok(EValue::Bool(true))
        },
    );

    let fx_eval_for_suppress = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-suppress",
        "(fx-suppress)",
        "Suppress the input event for the current MIDI FX evaluation.",
        move |_args, _ctx| eval_suppress_current_event(&fx_eval_for_suppress, "MIDI FX"),
    );

    let fx_eval_for_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-emit",
        "(fx-emit offset :vel value :note transpose ...) | (fx-emit :16 value ...)",
        "Emit a derived MIDI FX event at a musical offset.",
        move |args, _ctx| eval_emit_current_event(&fx_eval_for_emit, &args, "MIDI FX"),
    );

    let fx_eval_for_arp_count = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-count",
        "(fx-arp-count :16)",
        "Return the number of arp ticks for the current MIDI FX note spans at a timebase.",
        move |args, _ctx| eval_arp_count_current_event(&fx_eval_for_arp_count, &args, "MIDI FX"),
    );

    let fx_eval_for_arp_note = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-note",
        "(fx-arp-note :16 tick)",
        "Return the note for an arp tick at a timebase, or nil once that note lane has ended.",
        move |args, _ctx| eval_arp_note_current_event(&fx_eval_for_arp_note, &args, "MIDI FX"),
    );

    let fx_eval_for_arp_emit = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-emit",
        "(fx-arp-emit :16 tick :vel 0.8 ...)",
        "Emit one duration-aware arpeggiated MIDI FX event for a tick.",
        move |args, _ctx| eval_arp_emit_current_event(&fx_eval_for_arp_emit, &args, "MIDI FX"),
    );

    let fx_eval_for_time = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-time",
        "(fx-time :16) | (fx-time :8t units)",
        "Return a duration in beats for a MIDI FX timebase and optional unit count.",
        move |args, _ctx| eval_fx_time(&fx_eval_for_time, &args),
    );

    let fx_eval_for_source_time = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-source-time",
        "(fx-source-time) | (fx-source-time units)",
        "Return a duration in beats for the current source step and optional unit count.",
        move |args, _ctx| eval_fx_source_time(&fx_eval_for_source_time, &args),
    );

    let fx_eval_for_phase_time = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-phase-time",
        "(fx-phase-time)",
        "Return the current MIDI FX scheduling phase in beats. Live quantized triggers advance this across repeated invocations.",
        move |_args, _ctx| eval_fx_phase_time(&fx_eval_for_phase_time),
    );

    let fx_eval_for_phase_tick = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-phase-tick",
        "(fx-phase-tick :16)",
        "Return the current MIDI FX scheduling phase as a tick index at the given timebase.",
        move |args, _ctx| eval_fx_phase_tick(&fx_eval_for_phase_tick, &args),
    );

    let fx_eval_for_param = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-param",
        "(fx-param \"name\") | (fx-param index)",
        "Read the current MIDI FX slot parameter value, resolving the current step's p-lock over the slot default.",
        move |args, _ctx| eval_midi_fx_param(&fx_eval_for_param, &args),
    );

    let fx_eval_for_track = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-track",
        "(fx-track)",
        "Return the zero-based source track for the current MIDI FX event.",
        move |_args, _ctx| eval_midi_fx_track(&fx_eval_for_track),
    );

    let fx_eval_for_velocity = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-velocity",
        "(fx-velocity)",
        "Return the resolved velocity for the current MIDI FX event.",
        move |_args, _ctx| eval_midi_fx_velocity(&fx_eval_for_velocity),
    );

    let fx_eval_for_arp_emit_directed = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-arp-emit-directed",
        "(fx-arp-emit-directed rate tick direction :vel 0.8 ...)",
        "Emit one arpeggiated MIDI FX note with direction 0=up, 1=down, 2=up-down, 3=random.",
        move |args, _ctx| {
            eval_arp_emit_directed_current_event(&fx_eval_for_arp_emit_directed, &args, "MIDI FX")
        },
    );

    let fx_eval_for_note_count = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note-count",
        "(fx-note-count)",
        "Return the number of notes available to the current MIDI FX event.",
        move |_args, _ctx| {
            let guard = fx_eval_for_note_count
                .lock()
                .map_err(|_| "failed to lock MIDI FX eval context".to_string())?;
            let Some(eval) = guard.as_ref() else {
                return Err("MIDI FX context not active".to_string());
            };
            let count = eval
                .note_spans
                .as_ref()
                .map(|spans| spans.len())
                .unwrap_or_else(|| accumulator_chord_notes(eval).len());
            Ok(EValue::Number(count as f64))
        },
    );

    let fx_eval_for_note = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note",
        "(fx-note index)",
        "Return the transpose value for a note in the current MIDI FX event.",
        move |args, _ctx| eval_note_span_field(&fx_eval_for_note, &args, FxNoteField::Transpose),
    );

    let fx_eval_for_note_start = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note-start",
        "(fx-note-start index)",
        "Return a note start time in beats relative to the current MIDI FX event.",
        move |args, _ctx| eval_note_span_field(&fx_eval_for_note_start, &args, FxNoteField::Start),
    );

    let fx_eval_for_note_end = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-note-end",
        "(fx-note-end index)",
        "Return a note end time in beats relative to the current MIDI FX event.",
        move |args, _ctx| eval_note_span_field(&fx_eval_for_note_end, &args, FxNoteField::End),
    );

    let fx_eval_for_notes = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "fx-notes",
        "(fx-notes)",
        "Return all notes for the current MIDI FX event as maps with :note, :start, and :end fields.",
        move |_args, _ctx| eval_note_spans_as_list(&fx_eval_for_notes),
    );

    let fx_eval_for_state_get = Arc::clone(&accumulator_eval);
    let fx_state_for_get = Arc::clone(&midi_fx_state);
    runtime.register_native_with_docs(
        "fx-state-get",
        "(fx-state-get key) | (fx-state-get key default)",
        "Read persistent per-track/per-MIDI-FX state for the current MIDI FX callback.",
        move |args, _ctx| eval_midi_fx_state_get(&fx_eval_for_state_get, &fx_state_for_get, &args),
    );

    let fx_eval_for_state_set = Arc::clone(&accumulator_eval);
    let fx_state_for_set = Arc::clone(&midi_fx_state);
    runtime.register_native_with_docs(
        "fx-state-set",
        "(fx-state-set key value)",
        "Write persistent per-track/per-MIDI-FX state for the current MIDI FX callback.",
        move |args, _ctx| eval_midi_fx_state_set(&fx_eval_for_state_set, &fx_state_for_set, &args),
    );

    let acc_eval_for_set_step = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-set-step-param",
        "(acc-set-step-param :param value)",
        "Set a resolved step parameter for the current accumulator trigger only.",
        move |args, _ctx| {
            let param = parse_step_param_arg(&args, 0)?;
            let value = parse_value_arg(&args, 1, "step param")?;
            let mut guard = acc_eval_for_set_step
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            apply_step_param_set(&mut eval.resolved, param, value);
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_step = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-add-step-param",
        "(acc-add-step-param :param delta)",
        "Add a delta to a resolved step parameter for the current accumulator trigger only.",
        move |args, _ctx| {
            let param = parse_step_param_arg(&args, 0)?;
            let delta = parse_value_arg(&args, 1, "step param delta")?;
            let mut guard = acc_eval_for_add_step
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            apply_step_param_add(&mut eval.resolved, param, delta);
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_scale_step = Arc::clone(&accumulator_eval);
    runtime.register_native_with_docs(
        "acc-scale-step-param",
        "(acc-scale-step-param :param factor)",
        "Scale a resolved step parameter for the current accumulator trigger only.",
        move |args, _ctx| {
            let param = parse_step_param_arg(&args, 0)?;
            let factor = parse_value_arg(&args, 1, "step param factor")?;
            let mut guard = acc_eval_for_scale_step
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            apply_step_param_scale(&mut eval.resolved, param, factor);
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect = Arc::clone(&metadata);
    let context_for_acc_effect = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-effect-param",
        "(acc-set-effect-param ref normalized) | (acc-set-effect-param slot param-index normalized)",
        "Set an effect parameter for the current accumulator trigger using a normalized 0.0..1.0 value.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let normalized = parse_normalized_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_normalized(eval, slot_idx, param_idx, normalized, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_effect = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_effect = Arc::clone(&metadata);
    let context_for_add_acc_effect = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-effect-param",
        "(acc-add-effect-param ref normalized-delta) | (acc-add-effect-param slot param-index normalized-delta)",
        "Add a normalized delta to the current resolved effect parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let normalized_delta = parse_value_arg(&args, value_idx, "effect param delta")?;
            let mut guard = acc_eval_for_add_effect
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_effect);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_add_acc_effect,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            add_effect_param_normalized(
                eval,
                slot_idx,
                param_idx,
                normalized_delta,
                &param_desc,
            )?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect_raw = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect_raw = Arc::clone(&metadata);
    let context_for_acc_effect_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-effect-param-raw",
        "(acc-set-effect-param-raw ref value) | (acc-set-effect-param-raw slot param-index value)",
        "Set an effect parameter for the current accumulator trigger using a raw stored value.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let value = parse_value_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect_raw);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect_raw,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_raw(eval, slot_idx, param_idx, value, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_effect_raw = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_effect_raw = Arc::clone(&metadata);
    let context_for_add_acc_effect_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-effect-param-raw",
        "(acc-add-effect-param-raw ref delta) | (acc-add-effect-param-raw slot param-index delta)",
        "Add a raw delta to the current resolved effect parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let delta = parse_value_arg(&args, value_idx, "effect param delta")?;
            let mut guard = acc_eval_for_add_effect_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_effect_raw);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_add_acc_effect_raw,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            add_effect_param_raw(eval, slot_idx, param_idx, delta, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect_alias = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect_alias = Arc::clone(&metadata);
    let context_for_acc_effect_alias = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-plock-effect",
        "(acc-plock-effect ref normalized) | (acc-plock-effect slot param-index normalized)",
        "Alias for acc-set-effect-param using normalized values.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let normalized = parse_normalized_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect_alias
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect_alias);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect_alias,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_normalized(eval, slot_idx, param_idx, normalized, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_effect_alias_raw = Arc::clone(&accumulator_eval);
    let metadata_for_acc_effect_alias_raw = Arc::clone(&metadata);
    let context_for_acc_effect_alias_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-plock-effect-raw",
        "(acc-plock-effect-raw ref value) | (acc-plock-effect-raw slot param-index value)",
        "Alias for acc-set-effect-param-raw.",
        move |args, _ctx| {
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 0)?;
            let value = parse_value_arg(&args, value_idx, "effect param")?;
            let mut guard = acc_eval_for_effect_alias_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_effect_alias_raw);
            let param_desc = accumulator_effect_param_desc(
                &metadata_for_acc_effect_alias_raw,
                track_idx,
                slot_idx,
                param_idx,
            )?;
            set_effect_param_raw(eval, slot_idx, param_idx, value, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_instrument = Arc::clone(&accumulator_eval);
    let metadata_for_acc_instrument = Arc::clone(&metadata);
    let context_for_acc_instrument = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-instrument-param",
        "(acc-set-instrument-param ref normalized) | (acc-set-instrument-param param-index normalized)",
        "Set an instrument parameter for the current accumulator trigger using a normalized 0.0..1.0 value.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let normalized = parse_normalized_arg(&args, value_idx, "instrument param")?;
            let mut guard = acc_eval_for_instrument
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_instrument);
            let param_desc =
                accumulator_instrument_param_desc(&metadata_for_acc_instrument, track_idx, param_idx)?;
            set_instrument_param_normalized(eval, param_idx, normalized, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_instrument = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_instrument = Arc::clone(&metadata);
    let context_for_add_acc_instrument = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-instrument-param",
        "(acc-add-instrument-param ref normalized-delta) | (acc-add-instrument-param param-index normalized-delta)",
        "Add a normalized delta to the current resolved instrument parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let normalized_delta = parse_value_arg(&args, value_idx, "instrument param delta")?;
            let mut guard = acc_eval_for_add_instrument
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_instrument);
            let param_desc = accumulator_instrument_param_desc(
                &metadata_for_add_acc_instrument,
                track_idx,
                param_idx,
            )?;
            add_instrument_param_normalized(eval, param_idx, normalized_delta, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_instrument_raw = Arc::clone(&accumulator_eval);
    let metadata_for_acc_instrument_raw = Arc::clone(&metadata);
    let context_for_acc_instrument_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-set-instrument-param-raw",
        "(acc-set-instrument-param-raw ref value) | (acc-set-instrument-param-raw param-index value)",
        "Set an instrument parameter for the current accumulator trigger using a raw stored value.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let value = parse_value_arg(&args, value_idx, "instrument param")?;
            let mut guard = acc_eval_for_instrument_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_acc_instrument_raw);
            let param_desc = accumulator_instrument_param_desc(
                &metadata_for_acc_instrument_raw,
                track_idx,
                param_idx,
            )?;
            set_instrument_param_raw(eval, param_idx, value, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let acc_eval_for_add_instrument_raw = Arc::clone(&accumulator_eval);
    let metadata_for_add_acc_instrument_raw = Arc::clone(&metadata);
    let context_for_add_acc_instrument_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "acc-add-instrument-param-raw",
        "(acc-add-instrument-param-raw ref delta) | (acc-add-instrument-param-raw param-index delta)",
        "Add a raw delta to the current resolved instrument parameter for this accumulator trigger.",
        move |args, _ctx| {
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 0)?;
            let delta = parse_value_arg(&args, value_idx, "instrument param delta")?;
            let mut guard = acc_eval_for_add_instrument_raw
                .lock()
                .map_err(|_| "failed to lock accumulator eval context".to_string())?;
            let Some(eval) = guard.as_mut() else {
                return Err("accumulator context not active".to_string());
            };
            let track_idx = current_track(&context_for_add_acc_instrument_raw);
            let param_desc = accumulator_instrument_param_desc(
                &metadata_for_add_acc_instrument_raw,
                track_idx,
                param_idx,
            )?;
            add_instrument_param_raw(eval, param_idx, delta, &param_desc)?;
            Ok(EValue::Bool(true))
        },
    );

    let context_for_track = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-current-track",
        "(seq-current-track)",
        "Return the current 0-based track index for the scratch context.",
        move |_args, _ctx| Ok(EValue::Number(current_track(&context_for_track) as f64)),
    );

    let state_for_set_track = Arc::clone(&state);
    let context_for_set_track = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-current-track",
        "(seq-set-current-track track)",
        "Set the current 0-based track index for subsequent scratch operations.",
        move |args, ctx| {
            let Some(EValue::Number(track)) = args.first() else {
                return Err("expected 0-based track index".to_string());
            };
            let track = *track as isize;
            if track < 0 {
                return Err("track indices must be >= 0".to_string());
            }
            let track_count = state_for_set_track.active_track_count() as isize;
            if track >= track_count {
                return Err(format!("track out of range (0..{})", track_count - 1));
            }
            let track_idx = track as usize;
            if let Ok(mut eval_ctx) = context_for_set_track.lock() {
                eval_ctx.track = track_idx;
            }
            ctx.set_status(format!("current track {}", track));
            Ok(EValue::Number(track as f64))
        },
    );

    let state_for_host_set_track = Arc::clone(&state);
    let context_for_host_set_track = Arc::clone(&context);
    runtime.register_native("__host-set-current-track", move |args, _ctx| {
        let Some(EValue::Number(track)) = args.first() else {
            return Err("expected 0-based track index".to_string());
        };
        let track = *track as isize;
        if track < 0 {
            return Err("track indices must be >= 0".to_string());
        }
        let track_count = state_for_host_set_track.active_track_count() as isize;
        if track >= track_count {
            return Err(format!("track out of range (0..{})", track_count - 1));
        }
        if let Ok(mut eval_ctx) = context_for_host_set_track.lock() {
            eval_ctx.track = track as usize;
        }
        Ok(EValue::Number(track as f64))
    });
    runtime.document_symbol(
        "__host-set-current-track",
        "(__host-set-current-track track)",
        "Internal host hook that updates the scratch evaluation context's current track.",
    );

    let context_for_step = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-current-step",
        "(seq-current-step)",
        "Return the current 0-based step index for the scratch context.",
        move |_args, _ctx| Ok(EValue::Number(current_step(&context_for_step) as f64)),
    );

    let context_for_host_set_step = Arc::clone(&context);
    runtime.register_native("__host-set-current-step", move |args, _ctx| {
        let Some(EValue::Number(step)) = args.first() else {
            return Err("expected 0-based step index".to_string());
        };
        let step = *step as isize;
        if step < 0 {
            return Err("step indices must be >= 0".to_string());
        }
        if let Ok(mut eval_ctx) = context_for_host_set_step.lock() {
            eval_ctx.cursor_step = step as usize;
        }
        Ok(EValue::Number(step as f64))
    });
    runtime.document_symbol(
        "__host-set-current-step",
        "(__host-set-current-step step)",
        "Internal host hook that updates the scratch evaluation context's current step.",
    );

    let state_for_steps = Arc::clone(&state);
    let context_for_steps = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-num-steps",
        "(seq-num-steps)",
        "Return the number of steps in the current track.",
        move |_args, _ctx| {
            let track = current_track(&context_for_steps);
            Ok(EValue::Number(
                state_for_steps.pattern.track_params[track].get_num_steps() as f64,
            ))
        },
    );

    let state_for_toggle = Arc::clone(&state);
    let context_for_toggle = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-toggle-step",
        "(seq-toggle-step step)",
        "Toggle the active state of a 0-based step in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_toggle);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_toggle.toggle_step_and_clear_plocks(track_idx, step_idx);
            let active = state_for_toggle.pattern.patterns[track_idx].is_active(step_idx);
            ctx.set_status(format!(
                "track {} step {} {}",
                track_idx,
                step_idx,
                if active { "on" } else { "off" }
            ));
            Ok(EValue::Bool(active))
        },
    );

    let state_for_step_on = Arc::clone(&state);
    let context_for_step_on = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-step-on",
        "(seq-step-on step)",
        "Ensure a 0-based step is active in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_step_on);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_step_on.pattern.patterns[track_idx].set_step_active(step_idx, true);
            ctx.set_status(format!("track {} step {} on", track_idx, step_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_step_off = Arc::clone(&state);
    let context_for_step_off = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-step-off",
        "(seq-step-off step)",
        "Ensure a 0-based step is inactive in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_step_off);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_step_off.clear_step_payload(track_idx, step_idx);
            ctx.set_status(format!("track {} step {} off", track_idx, step_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_clear_step = Arc::clone(&state);
    let context_for_clear_step = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-clear-step",
        "(seq-clear-step step)",
        "Clear all payload data for a 0-based step in the current track.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_clear_step);
            let step_idx = parse_step_arg(&args, 0)?;
            state_for_clear_step.clear_step_payload(track_idx, step_idx);
            ctx.set_status(format!("track {} step {} cleared", track_idx, step_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_clear_track = Arc::clone(&state);
    let context_for_clear_track = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-clear-track",
        "(seq-clear-track)",
        "Clear all step payloads in the current track.",
        move |_args, ctx| {
            let track_idx = current_track(&context_for_clear_track);
            let num_steps = state_for_clear_track.pattern.track_params[track_idx].get_num_steps();
            for step in 0..num_steps {
                state_for_clear_track.clear_step_payload(track_idx, step);
            }
            ctx.set_status(format!("track {} cleared", track_idx));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_velocity = Arc::clone(&state);
    let context_for_velocity = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-velocity",
        "(seq-set-velocity step value)",
        "Set the velocity parameter for a 0-based step.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_velocity);
            let step_idx = parse_step_arg(&args, 0)?;
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("expected velocity value".to_string());
            };
            state_for_velocity.set_step_param(
                track_idx,
                step_idx,
                StepParam::Velocity,
                *value as f32,
            );
            ctx.set_status(format!(
                "track {} step {} velocity {}",
                track_idx, step_idx, value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_transpose = Arc::clone(&state);
    let context_for_transpose = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-set-transpose",
        "(seq-set-transpose step value)",
        "Set the transpose parameter for a 0-based step.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_transpose);
            let step_idx = parse_step_arg(&args, 0)?;
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("expected transpose value".to_string());
            };
            state_for_transpose.set_step_param(
                track_idx,
                step_idx,
                StepParam::Transpose,
                *value as f32,
            );
            ctx.set_status(format!(
                "track {} step {} transpose {}",
                track_idx, step_idx, value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_adjust = Arc::clone(&state);
    let context_for_adjust = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-adjust-transpose",
        "(seq-adjust-transpose step delta)",
        "Adjust the transpose parameter for a 0-based step by a delta.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_adjust);
            let step_idx = parse_step_arg(&args, 0)?;
            let Some(EValue::Number(value)) = args.get(1) else {
                return Err("expected transpose delta".to_string());
            };
            state_for_adjust.adjust_step_param(
                track_idx,
                step_idx,
                StepParam::Transpose,
                *value as f32,
            );
            ctx.set_status(format!(
                "track {} step {} transpose adjusted by {}",
                track_idx, step_idx, value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_step = Arc::clone(&state);
    let context_for_step_native = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-step",
        "(seq-step step)",
        "Return a map snapshot for a 0-based step in the current track.",
        move |args, _ctx| {
            let track_idx = current_track(&context_for_step_native);
            let step_idx = parse_step_arg(&args, 0)?;
            Ok(step_snapshot_to_value(
                step_idx,
                state_for_step.capture_step_snapshot(track_idx, step_idx),
            ))
        },
    );

    let state_for_track_steps = Arc::clone(&state);
    let context_for_track_steps = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-track-steps",
        "(seq-track-steps)",
        "Return a list of step snapshot maps for the current track.",
        move |_args, _ctx| {
            let track_idx = current_track(&context_for_track_steps);
            let num_steps = state_for_track_steps.pattern.track_params[track_idx].get_num_steps();
            let mut steps = Vec::with_capacity(num_steps);
            for step_idx in 0..num_steps {
                steps.push(step_snapshot_to_value(
                    step_idx,
                    state_for_track_steps.capture_step_snapshot(track_idx, step_idx),
                ));
            }
            Ok(lisp_list(steps))
        },
    );

    let state_for_rotate = Arc::clone(&state);
    let context_for_rotate = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-rotate-track",
        "(seq-rotate-track amount)",
        "Rotate the current track by the given step amount.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_rotate);
            let Some(EValue::Number(direction)) = args.first() else {
                return Err("expected rotation direction".to_string());
            };
            let num_steps = state_for_rotate.pattern.track_params[track_idx].get_num_steps();
            let steps: Vec<usize> = (0..num_steps).collect();
            state_for_rotate.rotate_steps(track_idx, &steps, *direction as isize);
            ctx.set_status(format!(
                "track {} rotated by {}",
                track_idx, *direction as isize
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_step_plock = Arc::clone(&state);
    let context_for_step_plock = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-step",
        "(seq-plock-step step :param value)",
        "Parameter-lock a step parameter using a keyword name.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_step_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let param = parse_step_param_arg(&args, 1)?;
            let value = parse_value_arg(&args, 2, "step param")?;
            state_for_step_plock.set_step_param(track_idx, step_idx, param, value);
            ctx.set_status(format!(
                "track {} step {} {} {}",
                track_idx,
                step_idx,
                param.short_label(),
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_timebase_plock = Arc::clone(&state);
    let context_for_timebase_plock = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-timebase",
        "(seq-plock-timebase step :timebase)",
        "Set a timebase override for a 0-based step.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_timebase_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let timebase = parse_timebase_arg(&args, 1)?;
            state_for_timebase_plock.pattern.timebase_plocks[track_idx].set(step_idx, timebase);
            state_for_timebase_plock.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} timebase {}",
                track_idx,
                step_idx,
                timebase.label()
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_effect_plock = Arc::clone(&state);
    let context_for_effect_plock = Arc::clone(&context);
    let metadata_for_effect_plock = Arc::clone(&metadata);
    let context_for_effect_param_name = Arc::clone(&context);
    let metadata_for_effect_name = Arc::clone(&metadata);
    runtime.register_native_with_docs("seq-effect-param-name", "(seq-effect-param-name slot param-index)", "Return the parameter name for a 0-based effect slot and 0-based parameter index on the current track.", move |args, _ctx| {
        let track_idx = current_track(&context_for_effect_param_name);
        let slot_idx = parse_slot_arg(&args, 0)?;
        let param_idx = parse_param_index_arg(&args, 1)?;
        let name = metadata_for_effect_name
            .lock()
            .ok()
            .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
            .as_ref()
            .and_then(|slots| slots.get(slot_idx))
            .and_then(|desc| desc.params.get(param_idx))
            .map(|param| param.name.clone())
            .ok_or_else(|| "effect parameter out of range".to_string())?;
        Ok(EValue::String(name))
    });

    let context_for_effect_param_names = Arc::clone(&context);
    let metadata_for_effect_names = Arc::clone(&metadata);
    runtime.register_native_with_docs(
        "seq-effect-param-names",
        "(seq-effect-param-names slot)",
        "Return a list of parameter names for a 0-based effect slot on the current track.",
        move |args, _ctx| {
            let track_idx = current_track(&context_for_effect_param_names);
            let slot_idx = parse_slot_arg(&args, 0)?;
            let params = metadata_for_effect_names
                .lock()
                .ok()
                .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|slots| slots.get(slot_idx))
                .map(|desc| {
                    desc.params
                        .iter()
                        .map(|param| EValue::String(param.name.clone()))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| "effect slot out of range".to_string())?;
            Ok(lisp_list(params))
        },
    );

    runtime.register_native_with_docs(
        "seq-plock-effect",
        "(seq-plock-effect step ref normalized) | (seq-plock-effect step slot param-index normalized)",
        "Set an effect parameter lock for a 0-based step using a normalized 0.0..1.0 value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_effect_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 1)?;
            let normalized = parse_normalized_arg(&args, value_idx, "effect p-lock")?;
            let Some(slot) = state_for_effect_plock.pattern.effect_chains[track_idx].get(slot_idx)
            else {
                return Err("effect slot out of range".to_string());
            };
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("effect param index out of range".to_string());
            }
            let param_desc = metadata_for_effect_plock
                .lock()
                .ok()
                .and_then(|metadata| metadata.effect_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|slots| slots.get(slot_idx))
                .and_then(|desc| desc.params.get(param_idx))
                .cloned()
                .ok_or_else(|| "effect descriptor missing for parameter".to_string())?;
            let value = param_desc.denormalize(normalized);
            slot.set_plock(step_idx, param_idx, value);
            state_for_effect_plock.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} effect {} param {} {}",
                track_idx,
                step_idx,
                slot_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_effect_plock_raw = Arc::clone(&state);
    let context_for_effect_plock_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-effect-raw",
        "(seq-plock-effect-raw step ref value) | (seq-plock-effect-raw step slot param-index value)",
        "Set an effect parameter lock for a 0-based step using the stored engine value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_effect_plock_raw);
            let step_idx = parse_step_arg(&args, 0)?;
            let (slot_idx, param_idx, value_idx) = parse_effect_param_target_arg(&args, 1)?;
            let value = parse_value_arg(&args, value_idx, "effect p-lock")?;
            let Some(slot) =
                state_for_effect_plock_raw.pattern.effect_chains[track_idx].get(slot_idx)
            else {
                return Err("effect slot out of range".to_string());
            };
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("effect param index out of range".to_string());
            }
            slot.set_plock(step_idx, param_idx, value);
            state_for_effect_plock_raw.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} effect {} param {} {}",
                track_idx,
                step_idx,
                slot_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_instrument_plock = Arc::clone(&state);
    let context_for_instrument_plock = Arc::clone(&context);
    let metadata_for_instrument_plock = Arc::clone(&metadata);
    let context_for_instrument_param_name = Arc::clone(&context);
    let metadata_for_instrument_name = Arc::clone(&metadata);
    runtime.register_native_with_docs(
        "seq-instrument-param-name",
        "(seq-instrument-param-name param-index)",
        "Return the parameter name for a 0-based instrument parameter index on the current track.",
        move |args, _ctx| {
            let track_idx = current_track(&context_for_instrument_param_name);
            let param_idx = parse_param_index_arg(&args, 0)?;
            let name = metadata_for_instrument_name
                .lock()
                .ok()
                .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|desc| desc.params.get(param_idx))
                .map(|param| param.name.clone())
                .ok_or_else(|| "instrument parameter out of range".to_string())?;
            Ok(EValue::String(name))
        },
    );

    let context_for_instrument_param_names = Arc::clone(&context);
    let metadata_for_instrument_names = Arc::clone(&metadata);
    runtime.register_native_with_docs(
        "seq-instrument-param-names",
        "(seq-instrument-param-names)",
        "Return a list of parameter names for the current track's instrument.",
        move |_args, _ctx| {
            let track_idx = current_track(&context_for_instrument_param_names);
            let params = metadata_for_instrument_names
                .lock()
                .ok()
                .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
                .as_ref()
                .map(|desc| {
                    desc.params
                        .iter()
                        .filter(|param| !param.name.eq_ignore_ascii_case("enabled"))
                        .map(|param| EValue::String(param.name.clone()))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| "instrument descriptor missing".to_string())?;
            Ok(lisp_list(params))
        },
    );

    runtime.register_native_with_docs(
        "seq-plock-instrument",
        "(seq-plock-instrument step ref normalized) | (seq-plock-instrument step param-index normalized)",
        "Set an instrument parameter lock for a 0-based step using a normalized 0.0..1.0 value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_instrument_plock);
            let step_idx = parse_step_arg(&args, 0)?;
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 1)?;
            let normalized = parse_normalized_arg(&args, value_idx, "instrument p-lock")?;
            let slot = &state_for_instrument_plock.pattern.instrument_slots[track_idx];
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("instrument param index out of range".to_string());
            }
            let param_desc = metadata_for_instrument_plock
                .lock()
                .ok()
                .and_then(|metadata| metadata.instrument_descriptors.get(track_idx).cloned())
                .as_ref()
                .and_then(|desc| desc.params.get(param_idx))
                .cloned()
                .ok_or_else(|| "instrument descriptor missing for parameter".to_string())?;
            let value = param_desc.denormalize(normalized);
            slot.set_plock(step_idx, param_idx, value);
            state_for_instrument_plock.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} instrument param {} {}",
                track_idx,
                step_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );

    let state_for_instrument_plock_raw = Arc::clone(&state);
    let context_for_instrument_plock_raw = Arc::clone(&context);
    runtime.register_native_with_docs(
        "seq-plock-instrument-raw",
        "(seq-plock-instrument-raw step ref value) | (seq-plock-instrument-raw step param-index value)",
        "Set an instrument parameter lock for a 0-based step using the stored engine value.",
        move |args, ctx| {
            let track_idx = current_track(&context_for_instrument_plock_raw);
            let step_idx = parse_step_arg(&args, 0)?;
            let (param_idx, value_idx) = parse_instrument_param_target_arg(&args, 1)?;
            let value = parse_value_arg(&args, value_idx, "instrument p-lock")?;
            let slot = &state_for_instrument_plock_raw.pattern.instrument_slots[track_idx];
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            if param_idx >= num_params {
                return Err("instrument param index out of range".to_string());
            }
            slot.set_plock(step_idx, param_idx, value);
            state_for_instrument_plock_raw.publish_scheduler_snapshot();
            ctx.set_status(format!(
                "track {} step {} instrument param {} {}",
                track_idx,
                step_idx,
                param_idx,
                value
            ));
            Ok(EValue::Bool(true))
        },
    );
}


pub(in crate::lisp_host) fn register_sequencer_impl(
    args: &[EValue],
    sequencers: &SharedRegisteredSequencers,
) -> Result<EValue, String> {
    let name = match args.first() {
        Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => {
            s.trim_start_matches('@').to_string()
        }
        _ => return Err("def-sequencer expects a name".to_string()),
    };
    let mut resolution = Timebase::Sixteenth;
    let mut tick: Option<EValue> = None;
    let mut tick_source: Option<String> = None;
    let mut idx = 1;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("def-sequencer expects keyword/value pairs".to_string()),
        };
        idx += 1;
        if args.get(idx).is_none() {
            return Err(format!("def-sequencer missing value for :{key}"));
        }
        match key.as_str() {
            "resolution" | "res" => resolution = parse_timebase_arg(args, idx)?,
            "tick" => tick = Some(args[idx].clone()),
            "tick-source" => match &args[idx] {
                EValue::String(source) => tick_source = Some(source.clone()),
                _ => return Err("def-sequencer :tick-source expects a string".to_string()),
            },
            "init" => { /* reserved for future one-time init */ }
            _ => return Err(format!("def-sequencer unknown key :{key}")),
        }
        idx += 1;
    }
    if tick.is_some() && tick_source.is_some() {
        return Err("def-sequencer accepts only one of :tick and :tick-source".to_string());
    }
    // `def-sequencer` auto-quotes :tick, so it arrives as list *data* — store it as
    // re-evaluable source (run once per boundary). The low-level `__register-sequencer`
    // also accepts already-built source for authoring surfaces that transform quoted data.
    let tick = if let Some(source) = tick_source {
        RegisteredAccumulatorCallback::Source(source)
    } else {
        match tick.ok_or_else(|| "def-sequencer requires :tick".to_string())? {
            value @ EValue::List(_) => RegisteredAccumulatorCallback::Source(
                eseqlisp::vm::format_lisp_source(&value),
            ),
            closure => RegisteredAccumulatorCallback::Closure(closure),
        }
    };
    let id = stable_sequencer_id(&name);
    let entry = RegisteredSequencer {
        id,
        name,
        resolution,
        tick,
    };
    let mut registry = sequencers
        .lock()
        .map_err(|_| "failed to lock sequencer registry".to_string())?;
    if let Some(existing) = registry.iter_mut().find(|e| e.id == entry.id) {
        *existing = entry;
    } else {
        registry.push(entry);
    }
    Ok(EValue::Number(id as f64))
}

/// Serialize an auto-quoted `:tick` body (list data) back to re-evaluable lisp
/// source, for shipping a UI-authored `def-sequencer` to the scheduler VM.
pub fn sequencer_tick_source(value: &EValue) -> String {
    eseqlisp::vm::format_lisp_source(value)
}

/// Parse a `def-sequencer` `:resolution` value (timebase keyword/number) to its
/// `Timebase` index, defaulting to sixteenth.
pub fn sequencer_resolution_index(value: &EValue) -> u8 {
    parse_timebase_arg(std::slice::from_ref(value), 0).unwrap_or(Timebase::Sixteenth) as u8
}

pub fn stable_sequencer_id(name: &str) -> u64 {
    // FNV-1a over the name; stable across processes so hot-reload matches by id.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    if hash == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        hash
    }
}

pub fn published_sequencer_from_def_args(args: &[EValue]) -> Result<PublishedSequencer, String> {
    let name = match args.first() {
        Some(EValue::String(s) | EValue::Symbol(s) | EValue::Keyword(s)) => {
            s.trim_start_matches('@').to_string()
        }
        _ => return Err("def-sequencer expects a name".to_string()),
    };
    if graph_mode_present(args) {
        let manifest = parse_graph_manifest(args)?;
        return Ok(PublishedSequencer {
            id: manifest.id,
            name,
            resolution: Timebase::Sixteenth as u8,
            tick_source: String::new(),
            graph: Some(manifest),
        });
    }

    let mut resolution: u8 = Timebase::Sixteenth as u8;
    let mut tick: Option<String> = None;
    let mut prebuilt_tick_source: Option<String> = None;
    let mut idx = 1;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("def-sequencer expects keyword/value pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("def-sequencer missing value for :{key}"));
        };
        match key.as_str() {
            "resolution" | "res" => resolution = sequencer_resolution_index(value),
            "tick" => tick = Some(sequencer_tick_source(value)),
            "tick-source" => match value {
                EValue::String(source) => prebuilt_tick_source = Some(source.clone()),
                _ => return Err("def-sequencer :tick-source expects a string".to_string()),
            },
            "init" => { /* reserved for future one-time init */ }
            _ => return Err(format!("def-sequencer unknown key :{key}")),
        }
        idx += 1;
    }
    if tick.is_some() && prebuilt_tick_source.is_some() {
        return Err("def-sequencer accepts only one of :tick and :tick-source".to_string());
    }
    let tick_source = tick
        .or(prebuilt_tick_source)
        .ok_or_else(|| "def-sequencer requires :tick".to_string())?;
    Ok(PublishedSequencer {
        id: stable_sequencer_id(&name),
        name,
        resolution,
        tick_source,
        graph: None,
    })
}

pub(in crate::lisp_host) fn gen_splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// Parse `seq-emit-control` args into an [`EmittedMixerControl`]. Shape-level
/// validation only: op and exactly one target are required here; target
/// *existence* is checked app-side at apply time so unknown targets fail with
/// a visible host error instead of silently erroring the whole tick
/// (docs/jaki-mixer-control-routes-spec.md §4).
pub(in crate::lisp_host) fn build_seq_emit_control(
    args: &[EValue],
) -> Result<crate::mixer_control::EmittedMixerControl, String> {
    use crate::mixer_control::{EmittedMixerControl, MixerControlOp, MixerControlTarget};
    let mut op: Option<MixerControlOp> = None;
    let mut target: Option<MixerControlTarget> = None;
    let mut offset_beats: f32 = 0.0;
    let mut duration_beats: Option<f32> = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("seq-emit-control expects keyword/value pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("seq-emit-control missing value for :{key}"));
        };
        match key.as_str() {
            "op" => {
                let name = match value {
                    EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                        k.trim_start_matches(':').to_ascii_lowercase()
                    }
                    _ => {
                        return Err(
                            "seq-emit-control :op expects \"mute\" or \"solo\"".to_string()
                        )
                    }
                };
                op = Some(match name.as_str() {
                    "mute" => MixerControlOp::Mute,
                    "solo" => MixerControlOp::Solo,
                    other => {
                        return Err(format!(
                            "seq-emit-control :op expects \"mute\" or \"solo\", got \"{other}\""
                        ))
                    }
                });
            }
            "track" => {
                if target.is_some() {
                    return Err(
                        "seq-emit-control expects exactly one of :track or :group".to_string()
                    );
                }
                let track = acc_emit_number(value, "track")?;
                if track < 0.0 {
                    return Err("seq-emit-control :track must be >= 0".to_string());
                }
                target = Some(MixerControlTarget::Track(track as usize));
            }
            "group" => {
                if target.is_some() {
                    return Err(
                        "seq-emit-control expects exactly one of :track or :group".to_string()
                    );
                }
                let name = match value {
                    EValue::String(name) | EValue::Symbol(name) => name.clone(),
                    _ => {
                        return Err("seq-emit-control :group expects a group name".to_string())
                    }
                };
                target = Some(MixerControlTarget::Group(name));
            }
            "at" => {
                offset_beats = acc_emit_number(value, "at")?.max(0.0);
            }
            "dur" | "duration" => {
                duration_beats = Some(acc_emit_number(value, "dur")?);
            }
            other => return Err(format!("seq-emit-control unknown key :{other}")),
        }
        idx += 1;
    }
    let Some(op) = op else {
        return Err("seq-emit-control requires :op".to_string());
    };
    let Some(target) = target else {
        return Err("seq-emit-control requires :track or :group".to_string());
    };
    let Some(duration_beats) = duration_beats else {
        return Err("seq-emit-control requires :dur".to_string());
    };
    if duration_beats <= 0.0 {
        return Err("seq-emit-control :dur must be > 0".to_string());
    }
    Ok(EmittedMixerControl {
        op,
        target,
        offset_beats,
        duration_beats,
    })
}

pub(in crate::lisp_host) fn build_seq_emit_event(
    args: &[EValue],
    ctx: &GeneratorTickContext,
) -> Result<EmittedAccumulatorEvent, String> {
    let mut resolved = crate::generator::default_resolved();
    let mut chord: Vec<f32> = Vec::new();
    let mut offset_beats: f32 = 0.0;
    let mut target_track: Option<usize> = None;
    let mut quantize: Option<Timebase> = None;
    let mut idx = 0;
    while idx < args.len() {
        let key = match &args[idx] {
            EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k) => {
                k.trim_start_matches(':').to_ascii_lowercase()
            }
            _ => return Err("seq-emit expects keyword/value pairs".to_string()),
        };
        idx += 1;
        let Some(value) = args.get(idx) else {
            return Err(format!("seq-emit missing value for :{key}"));
        };
        match key.as_str() {
            "at" => {
                offset_beats = match value {
                    EValue::Keyword(k) | EValue::String(k) | EValue::Symbol(k)
                        if k.eq_ignore_ascii_case("now") =>
                    {
                        0.0
                    }
                    EValue::Number(n) => *n as f32,
                    _ => return Err("seq-emit :at expects :now or a beats number".to_string()),
                };
            }
            "vel" | "velocity" => {
                resolved.velocity = acc_emit_number(value, "velocity")?.clamp(0.0, 1.0)
            }
            "note" | "transpose" | "trn" => resolved.transpose = acc_emit_number(value, "note")?,
            "dur" | "duration" => resolved.duration = acc_emit_number(value, "duration")?.max(0.0),
            "speed" | "spd" => resolved.speed = acc_emit_number(value, "speed")?.max(0.0),
            "pan" => resolved.pan = acc_emit_number(value, "pan")?.clamp(-1.0, 1.0),
            "chop" | "chp" => resolved.chop = acc_emit_number(value, "chop")?.max(1.0),
            "track" => {
                let track = acc_emit_number(value, "track")?;
                if track < 0.0 {
                    return Err("seq-emit :track must be >= 0".to_string());
                }
                target_track = Some(track as usize);
            }
            "chord" => {
                chord.clear();
                let EValue::List(items) = value else {
                    return Err("seq-emit :chord expects a list of transposes".to_string());
                };
                for item in items {
                    if let EValue::Number(n) = &*item.borrow() {
                        chord.push(*n as f32);
                    }
                }
            }
            "quantize" | "q" => {
                quantize = match value {
                    EValue::Bool(false) | EValue::Nil => None,
                    EValue::Keyword(k) | EValue::String(k) if k.eq_ignore_ascii_case("off") => None,
                    _ => Some(parse_timebase_arg(args, idx)?),
                };
            }
            _ => return Err(format!("seq-emit unknown key :{key}")),
        }
        idx += 1;
    }
    if let Some(grid) = quantize {
        let grid_beats = grid
            .step_beats(crate::generator::GENERATOR_RESOLUTION_REF_STEPS)
            .max(1e-9);
        let target = ctx.beat + offset_beats as f64;
        let position = target / grid_beats;
        let nearest = position.round();
        let snapped_units = if (position - nearest).abs() <= 1e-9 {
            nearest
        } else {
            position.ceil()
        };
        let snapped = (snapped_units * grid_beats).max(target);
        offset_beats = (snapped - ctx.beat) as f32;
    }
    Ok(EmittedAccumulatorEvent {
        offset_beats,
        track: target_track,
        resolved,
        chord,
        chord_durations: Vec::new(),
        chord_step_transpose: 0.0,
        effect_params: Vec::new(),
        instrument_params: Vec::new(),
    })
}

pub(in crate::lisp_host) fn parse_timebase_arg(args: &[EValue], idx: usize) -> Result<Timebase, String> {
    let Some(value) = args.get(idx) else {
        return Err("expected timebase".to_string());
    };
    match value {
        EValue::Number(n) if *n >= 0.0 => {
            let idx = *n as usize;
            Timebase::ALL
                .get(idx)
                .copied()
                .ok_or_else(|| "invalid timebase index".to_string())
        }
        EValue::Keyword(name) | EValue::String(name) => {
            let normalized = name.to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "whole" => Ok(Timebase::Whole),
                "2" | "half" => Ok(Timebase::Half),
                "4" | "quarter" => Ok(Timebase::Quarter),
                "8" | "eighth" => Ok(Timebase::Eighth),
                "16" | "sixteenth" => Ok(Timebase::Sixteenth),
                "32" | "thirtysecond" | "thirty-second" => Ok(Timebase::ThirtySecond),
                "64" | "sixtyfourth" | "sixty-fourth" => Ok(Timebase::SixtyFourth),
                "2t" | "halftriplet" | "half-triplet" => Ok(Timebase::HalfTriplet),
                "4t" | "quartertriplet" | "quarter-triplet" => Ok(Timebase::QuarterTriplet),
                "8t" | "eighthtriplet" | "eighth-triplet" => Ok(Timebase::EighthTriplet),
                "16t" | "sixteenthtriplet" | "sixteenth-triplet" => Ok(Timebase::SixteenthTriplet),
                "32t" | "thirtysecondtriplet" | "thirty-second-triplet" => {
                    Ok(Timebase::ThirtySecondTriplet)
                }
                "64t" | "sixtyfourthtriplet" | "sixty-fourth-triplet" => {
                    Ok(Timebase::SixtyFourthTriplet)
                }
                "prh" | "polyrhythm" => Ok(Timebase::Polyrhythm),
                _ => Err("unknown timebase".to_string()),
            }
        }
        _ => Err("expected timebase keyword/string/index".to_string()),
    }
}

pub(in crate::lisp_host) fn midi_fx_attr_name(value: &EValue) -> Option<String> {
    match value {
        EValue::Keyword(name) => Some(
            name.trim_start_matches('@')
                .trim_start_matches(':')
                .to_ascii_lowercase(),
        ),
        EValue::Symbol(name) | EValue::String(name)
            if name.starts_with('@') || name.starts_with(':') =>
        {
            Some(
                name.trim_start_matches('@')
                    .trim_start_matches(':')
                    .to_ascii_lowercase(),
            )
        }
        _ => None,
    }
}

pub(in crate::lisp_host) fn midi_fx_known_param_attr_name(value: &EValue) -> Option<String> {
    let name = midi_fx_attr_name(value)?;
    matches!(
        name.as_str(),
        "default" | "min" | "max" | "unit" | "role" | "tags" | "enum"
    )
    .then_some(name)
}

pub(in crate::lisp_host) fn midi_fx_attr_number(args: &[EValue], idx: usize, attr: &str) -> Result<f32, String> {
    match args.get(idx) {
        Some(EValue::Number(value)) => Ok(*value as f32),
        _ => Err(format!("midi-fx-param :{attr} expects a number")),
    }
}

pub(in crate::lisp_host) fn parse_midi_fx_param_descriptor(
    name: &str,
    args: &[EValue],
) -> Result<crate::effects::ParamDescriptor, String> {
    let mut default = 0.0_f32;
    let mut min = 0.0_f32;
    let mut max = 1.0_f32;
    let mut unit = None;
    let mut role = None;
    let mut tags = Vec::new();
    let mut labels: Option<Vec<String>> = None;
    let mut idx = 0;
    while idx < args.len() {
        let Some(attr) = midi_fx_attr_name(&args[idx]) else {
            return Err("midi-fx-param expects keyword attributes".to_string());
        };
        idx += 1;
        match attr.as_str() {
            "default" => {
                default = midi_fx_attr_number(args, idx, "default")?;
                idx += 1;
            }
            "min" => {
                min = midi_fx_attr_number(args, idx, "min")?;
                idx += 1;
            }
            "max" => {
                max = midi_fx_attr_number(args, idx, "max")?;
                idx += 1;
            }
            "unit" => {
                unit = match args.get(idx) {
                    Some(EValue::String(value))
                    | Some(EValue::Keyword(value))
                    | Some(EValue::Symbol(value)) => Some(value.clone()),
                    _ => return Err("midi-fx-param :unit expects string/symbol".to_string()),
                };
                idx += 1;
            }
            "role" => {
                role = match args.get(idx) {
                    Some(EValue::String(value))
                    | Some(EValue::Keyword(value))
                    | Some(EValue::Symbol(value)) => Some(value.clone()),
                    _ => return Err("midi-fx-param :role expects string/symbol".to_string()),
                };
                idx += 1;
            }
            "tags" => {
                while idx < args.len() && midi_fx_known_param_attr_name(&args[idx]).is_none() {
                    match &args[idx] {
                        EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                            tags.push(value.clone())
                        }
                        _ => return Err("midi-fx-param :tags values must be strings".to_string()),
                    }
                    idx += 1;
                }
            }
            "enum" => {
                let mut enum_labels = Vec::new();
                while idx < args.len() && midi_fx_known_param_attr_name(&args[idx]).is_none() {
                    match &args[idx] {
                        EValue::String(value) | EValue::Keyword(value) | EValue::Symbol(value) => {
                            enum_labels.push(value.clone())
                        }
                        _ => return Err("midi-fx-param :enum labels must be strings".to_string()),
                    }
                    idx += 1;
                }
                if enum_labels.is_empty() {
                    return Err("midi-fx-param :enum expects at least one label".to_string());
                }
                max = (enum_labels.len().saturating_sub(1)) as f32;
                labels = Some(enum_labels);
            }
            other => return Err(format!("midi-fx-param unknown attribute :{other}")),
        }
    }
    if max < min {
        std::mem::swap(&mut min, &mut max);
    }
    default = default.clamp(min, max);
    Ok(crate::effects::ParamDescriptor {
        name: name.to_string(),
        min,
        max,
        default,
        kind: labels
            .map(|labels| crate::effects::ParamKind::Enum { labels })
            .unwrap_or(crate::effects::ParamKind::Continuous { unit }),
        scaling: crate::effects::ParamScaling::Linear,
        node_param_idx: 0,
        node_param_span: 1,
        host_control: None,
        ui_metadata: crate::effects::ParamUiMetadata::with_tags(None, None, role, tags),
    })
}

pub(in crate::lisp_host) fn midi_fx_param_descriptor_for_slot(
    state: &crate::sequencer::SequencerState,
    registry: &[RegisteredAccumulator],
    track_idx: usize,
    slot_idx: usize,
    param_ref: &EValue,
) -> Result<crate::effects::ParamDescriptor, String> {
    let chain = state.pattern.track_params[track_idx].midi_fx_chain();
    let fx_name = chain
        .get(slot_idx)
        .ok_or_else(|| "MIDI FX slot out of range".to_string())?;
    let entry = registry
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(fx_name))
        .ok_or_else(|| format!("unknown MIDI FX '{fx_name}'"))?;
    match param_ref {
        EValue::Number(index) if *index >= 0.0 => entry
            .params
            .get(*index as usize)
            .cloned()
            .ok_or_else(|| "MIDI FX param index out of range".to_string()),
        EValue::String(name) | EValue::Keyword(name) | EValue::Symbol(name) => entry
            .params
            .iter()
            .find(|param| param.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| format!("unknown MIDI FX param '{name}'")),
        _ => Err("MIDI FX param must be name or index".to_string()),
    }
}
